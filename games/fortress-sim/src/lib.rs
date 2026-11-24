#![deny(clippy::float_arithmetic)]

//! Integer fortress prototype: residents dig into a mountain, haul and build
//! across stacked levels.

mod collapse;
mod config;
pub mod content;
mod diagnostics;
mod fluids;
mod render;
mod snapshot;
mod temperature;
mod types;
pub mod view;
mod world;

pub use config::{FortressConfig, Pathfinding, PhysicsConfig};
pub use content::GameContent;
pub use sim_core::space::{Pos3, Tile, TileKind};
pub use types::{BuildKind, DigMode, FortressCommand, ItemKind};

use sim_core::{
    entity::{EntityCommand, EntityCommandBuffer, EntityTable, SlotPermutation},
    index::CategoryIndex,
    jobs::{Assigner, JobBoard, JobId, JobState, Reservations, job_distance},
    path::{
        AStar, Connectivity, HierarchicalPathfinder, PathRequestQueue, PathResult, QueueStats,
        SearchOutcome,
    },
    physics::{ActiveChunkSet, ChunkDoubleBuffer},
    rng::{below, stateless_u64},
    schedule::{EventQueue, staggered},
    space::{ChunkId, ChunkedGrid3, standable, walk_neighbors},
    support::SupportChecker,
    topology::LayeredWalkTopology,
};
use sim_host::{NoopProbe, PhaseProbe, SimError, SimMetrics, Simulation};

use types::{
    Death, Event, Item, ItemId, ItemLocation, JobKind, Needs, Op, ResKey, Resident, ResidentId,
    Route, RouteStatus, Task, Vitals,
};

const DIG_PRIORITY: i32 = 5;
const BUILD_PRIORITY: i32 = 4;
const HAUL_PRIORITY: i32 = 2;
const FOOD_PRIORITY: i32 = 10;
const CLEAR_PRIORITY: i32 = 6;
const MAX_ROUTE_FAILURES: u8 = 3;
pub(crate) const HEALTH: u16 = 100;
/// Diagnostic threshold for long paths, in nodes.
const LONG_PATH: usize = 100;
pub(crate) const STOCKPILE_CAPACITY: usize = 1 << 16;
const SOURCE_COMMAND: u8 = 0;
const SOURCE_RESIDENT: u8 = 1;
pub(crate) const SOURCE_EVENT: u8 = 2;

sim_core::entity_columns! {
    struct ResidentColumns, row ResidentRow {
        position: Pos3,
        needs: Needs,
        vitals: Vitals,
        task: Task,
        carried: Option<ItemId>,
        route: Route,
        work: u32,
    }
}

sim_core::entity_columns! {
    struct ItemColumns, row ItemRow {
        location: ItemLocation,
        kind: ItemKind,
        material: u16,
        haul: Option<JobId>,
    }
}

/// Logical history counters. They do not influence behavior but are saved and hashed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Counters {
    pub dug: u64,
    pub built: u64,
    pub hauled: u64,
    pub eaten: u64,
    pub caravans: u64,
    pub deaths: [u64; 4],
    pub invalid_designations: u64,
    pub aborted_tasks: u64,
    pub completed_jobs: u64,
    pub collapses: u64,
    pub collapsed_tiles: u64,
}

sim_core::state_hash_struct!(Counters {
    dug,
    built,
    hauled,
    eaten,
    caravans,
    deaths,
    invalid_designations,
    aborted_tasks,
    completed_jobs,
    collapses,
    collapsed_tiles
});

/// Diagnostics excluded from hashes and snapshots.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Diagnostics {
    pub path: QueueStats,
    pub path_ticks: u64,
    pub unreachable_prechecks: u64,
    pub route_failures: u64,
    pub blocked_steps: u64,
    pub build_waits: u64,
    pub changed_chunks: u64,
    pub last_changed_chunks: u64,
    pub lowest_level: i32,
    pub fluid_tiles: u64,
    pub fluid_last_tiles: u64,
    pub items_destroyed: u64,
    pub heat_tiles: u64,
    pub heat_last_chunks: u64,
    pub heat_last_tiles: u64,
    pub support_last_units: u64,
    pub heat_pool_exhausted: u64,
    pub path_cost: u64,
    pub long_paths: u64,
    pub long_path_nodes: u64,
}

/// Path search in the configured mode. The hierarchical mode falls back to A*
/// when its abstract graph finds no path, for example across ramp-only or
/// diagonal-only chunk crossings. Both modes depend only on the tiles.
pub(crate) struct Pathfinder {
    astar: AStar<Pos3>,
    hierarchical: Option<Box<HierarchicalPathfinder>>,
    pub fallbacks: u64,
}

impl Pathfinder {
    fn new(kind: Pathfinding, world: &ChunkedGrid3<Tile>, limit: u32) -> Self {
        Self {
            astar: AStar::with_capacity(world.len(), limit as usize * 8 + 64),
            hierarchical: (kind == Pathfinding::Hierarchical)
                .then(|| Box::new(HierarchicalPathfinder::new(world, limit))),
            fallbacks: 0,
        }
    }

    pub(crate) fn hpa_stats(&self) -> Option<sim_core::path::HpaStats> {
        self.hierarchical.as_ref().map(|hpa| hpa.stats)
    }

    fn search(
        &mut self,
        world: &ChunkedGrid3<Tile>,
        start: Pos3,
        goal: Pos3,
        limit: u32,
        path: &mut Vec<Pos3>,
    ) -> SearchOutcome {
        let topology = LayeredWalkTopology::new(world);
        let Some(hpa) = &mut self.hierarchical else {
            return self.astar.search(&topology, start, goal, limit, path);
        };
        let outcome = hpa.search(world, start, goal, limit, path);
        if outcome.result != PathResult::Unreachable {
            return outcome;
        }
        self.fallbacks += 1;
        let rest = limit.saturating_sub(outcome.expansions).max(1);
        let fallback = self.astar.search(&topology, start, goal, rest, path);
        SearchOutcome {
            result: fallback.result,
            expansions: outcome.expansions.saturating_add(fallback.expansions),
        }
    }
}

type OpSource = (u8, u64);

pub struct FortressSim {
    config: FortressConfig,
    content: GameContent,
    seed: u64,
    tick: u64,
    world: ChunkedGrid3<Tile>,
    connectivity: Connectivity,
    residents: EntityTable<Resident>,
    rcols: ResidentColumns,
    paths: Vec<u32>,
    items: EntityTable<Item>,
    icols: ItemColumns,
    fluid: ChunkedGrid3<u8>,
    fluid_active: ActiveChunkSet,
    balance: fluids::FluidBalance,
    aquifer_sources: Vec<Pos3>,
    heat: ChunkDoubleBuffer<i32>,
    heat_balance: temperature::HeatBalance,
    heat_sources: Vec<Pos3>,
    support: SupportChecker,
    item_index: CategoryIndex<ItemId>,
    jobs: JobBoard<JobKind, ResidentId>,
    reservations: Reservations<ResKey, ResidentId>,
    queue: PathRequestQueue<ResidentId, Pos3>,
    pathfinder: Pathfinder,
    stockpiles: Vec<Pos3>,
    events: EventQueue<Event>,
    ops: EntityCommandBuffer<Op, OpSource>,
    counters: Counters,
    diag: Diagnostics,
    scratch: Scratch,
    load_warnings: Vec<String>,
}

/// Reused buffers. Their contents never survive a phase.
struct Scratch {
    chunks: Vec<ChunkId>,
    fluid_marks: Vec<bool>,
    fluid_changed: Vec<ChunkId>,
    fluid_chunks: Vec<ChunkId>,
    fluid_may_change: Vec<bool>,
    fluid_cells: Vec<(Pos3, u8)>,
    transfers: Vec<fluids::Transfer>,
    grants: Vec<(fluids::Transfer, u8, bool)>,
    evaporating: Vec<Pos3>,
    fluid_tiles: Vec<(Pos3, Tile)>,
    dead: Vec<ResidentId>,
    heat_was_active: Vec<bool>,
    heat_chunks: Vec<ChunkId>,
    hot: Vec<Pos3>,
    unsupported: Vec<Pos3>,
    fall_paths: Vec<(Pos3, Pos3, u16)>,
    ids: Vec<ResidentId>,
    workers: Vec<(ResidentId, Pos3)>,
    assignments: Vec<(ResidentId, JobId)>,
    assigner: Assigner,
    pending: Vec<(ResidentId, Task)>,
    outcomes: Vec<(ResidentId, ResKey, bool)>,
    stock_counts: Vec<u16>,
    tile_changes: Vec<(Pos3, Tile)>,
    occupied: Vec<Pos3>,
    path: Vec<Pos3>,
    failed: Vec<(ResidentId, bool)>,
    expired: Vec<(ResKey, ResidentId)>,
    abort: Vec<ResidentId>,
    job_positions: Vec<Pos3>,
    neighbors: Vec<(Pos3, u32)>,
    order: Vec<usize>,
    permutation: SlotPermutation,
}

impl Scratch {
    fn new(config: &FortressConfig) -> Self {
        let residents = config.max_residents as usize;
        let items = config.max_items as usize;
        let k = config.job_candidates as usize;
        let tiles = (config.world_size_xy * config.world_size_xy * config.world_depth) as usize;
        let chunk_count = tiles / sim_core::physics::CHUNK_VOLUME + 64;
        Self {
            chunks: Vec::with_capacity(4096),
            fluid_marks: vec![false; chunk_count * 2],
            fluid_changed: Vec::with_capacity(chunk_count * 2),
            fluid_chunks: Vec::with_capacity(chunk_count * 2),
            fluid_may_change: Vec::with_capacity(chunk_count * 2),
            fluid_cells: Vec::with_capacity(sim_core::physics::CHUNK_VOLUME),
            transfers: Vec::with_capacity(tiles / 4 + 4096),
            grants: Vec::with_capacity(tiles / 4 + 4096),
            evaporating: Vec::with_capacity(4096),
            fluid_tiles: Vec::with_capacity(tiles / 8 + 4096),
            dead: Vec::with_capacity(residents),
            heat_was_active: Vec::with_capacity(chunk_count * 2),
            heat_chunks: Vec::with_capacity(chunk_count * 2),
            hot: Vec::with_capacity(1 << 16),
            unsupported: Vec::with_capacity(1 << 16),
            fall_paths: Vec::with_capacity(1 << 16),
            ids: Vec::with_capacity(residents),
            workers: Vec::with_capacity(residents),
            assignments: Vec::with_capacity(residents),
            assigner: Assigner::with_capacity(residents, k),
            pending: Vec::with_capacity(residents),
            outcomes: Vec::with_capacity(residents * 2),
            stock_counts: Vec::with_capacity(STOCKPILE_CAPACITY),
            tile_changes: Vec::with_capacity(residents),
            occupied: Vec::with_capacity(residents + items),
            path: Vec::with_capacity(config.max_path_nodes as usize * 4),
            failed: Vec::with_capacity(residents),
            expired: Vec::with_capacity(residents * 2),
            abort: Vec::with_capacity(residents * 2),
            job_positions: Vec::with_capacity(config.max_jobs as usize),
            neighbors: Vec::with_capacity(32),
            order: Vec::with_capacity(residents.max(items)),
            permutation: SlotPermutation::with_capacity(residents.max(items)),
        }
    }
}

/// Counts down a work duration that starts when the counter is zero.
fn tick_work(work: &mut u32, duration: u32) -> bool {
    if *work == 0 {
        *work = duration.max(1);
    }
    *work -= 1;
    *work == 0
}

fn dig_result(tile: Tile, mode: DigMode) -> Tile {
    let kind = match mode {
        DigMode::Dig => return sim_core::space::dig(tile),
        DigMode::StairUpDown => TileKind::StairUpDown,
        DigMode::StairDown => TileKind::StairDown,
        DigMode::StairUp => TileKind::StairUp,
        DigMode::Ramp => TileKind::Ramp,
    };
    Tile::new(kind, false, tile.material)
}

fn build_result(kind: BuildKind, material: u16) -> Tile {
    match kind {
        BuildKind::Wall => Tile::new(TileKind::Solid, false, material),
        BuildKind::Floor => Tile::new(TileKind::Open, true, material),
        BuildKind::StairUpDown => Tile::new(TileKind::StairUpDown, false, material),
        BuildKind::Door => Tile::new(TileKind::Door, true, material),
    }
}

pub(crate) fn build_target_valid(tile: Tile, kind: BuildKind) -> bool {
    match kind {
        BuildKind::Floor => tile.kind == TileKind::Open && !tile.floor(),
        _ => tile.kind == TileKind::Open && tile.floor(),
    }
}

fn has_site(sites: &[Option<Pos3>; 6]) -> bool {
    sites.iter().any(Option::is_some)
}

/// Standable tiles from which a solid `target` can be dug in `mode`, in fixed order:
/// orthogonal neighbors, then the stair above and the stair below.
fn dig_sites(
    world: &ChunkedGrid3<Tile>,
    connectivity: &Connectivity,
    target: Pos3,
    mode: DigMode,
) -> [Option<Pos3>; 6] {
    let mut out = [None; 6];
    let Some(&tile) = world.get(target).filter(|t| t.kind == TileKind::Solid) else {
        return out;
    };
    let result = dig_result(tile, mode).kind;
    for (i, (dx, dy)) in sim_core::space::ORTHOGONAL.into_iter().enumerate() {
        let p = target.offset(dx, dy, 0);
        if connectivity.is_standable(p) {
            out[i] = Some(p);
        }
    }
    let above = target.offset(0, 0, 1);
    if climbs_up(result)
        && world.get(above).is_some_and(|t| climbs_down(t.kind))
        && connectivity.is_standable(above)
    {
        out[4] = Some(above);
    }
    let below = target.offset(0, 0, -1);
    if climbs_down(result)
        && world.get(below).is_some_and(|t| climbs_up(t.kind))
        && connectivity.is_standable(below)
    {
        out[5] = Some(below);
    }
    out
}

fn build_sites(connectivity: &Connectivity, target: Pos3) -> [Option<Pos3>; 6] {
    let mut out = [None; 6];
    for (i, (dx, dy)) in sim_core::space::ORTHOGONAL.into_iter().enumerate() {
        let p = target.offset(dx, dy, 0);
        if connectivity.is_standable(p) {
            out[i] = Some(p);
        }
    }
    out
}

const fn climbs_up(kind: TileKind) -> bool {
    matches!(kind, TileKind::StairUp | TileKind::StairUpDown)
}

const fn climbs_down(kind: TileKind) -> bool {
    matches!(kind, TileKind::StairDown | TileKind::StairUpDown)
}

fn box_range(from: Pos3, to: Pos3) -> (Pos3, Pos3) {
    (
        Pos3::new(from.x.min(to.x), from.y.min(to.y), from.z.min(to.z)),
        Pos3::new(from.x.max(to.x), from.y.max(to.y), from.z.max(to.z)),
    )
}

fn box_volume(low: Pos3, high: Pos3) -> u64 {
    (low.x.abs_diff(high.x) as u64 + 1)
        * (low.y.abs_diff(high.y) as u64 + 1)
        * (low.z.abs_diff(high.z) as u64 + 1)
}

fn for_box(low: Pos3, high: Pos3, mut visit: impl FnMut(Pos3)) {
    for z in low.z..=high.z {
        for y in low.y..=high.y {
            for x in low.x..=high.x {
                visit(Pos3::new(x, y, z));
            }
        }
    }
}

impl FortressSim {
    fn empty(
        seed: u64,
        config: FortressConfig,
        content: GameContent,
        world: ChunkedGrid3<Tile>,
    ) -> Result<Self, SimError> {
        let residents = config.max_residents;
        let items = config.max_items;
        let connectivity = Connectivity::new(&world);
        let pathfinder = Pathfinder::new(config.pathfinding, &world, config.path_expansion_limit);
        let (w, d) = (config.world_size_xy, config.world_depth);
        Ok(Self {
            seed,
            tick: 0,
            connectivity,
            pathfinder,
            residents: EntityTable::with_capacity(residents),
            rcols: ResidentColumns::with_capacity(residents as usize),
            paths: vec![0; residents as usize * config.max_path_nodes as usize],
            items: EntityTable::with_capacity(items),
            icols: ItemColumns::with_capacity(items as usize),
            fluid_active: ActiveChunkSet::new(&world, config.physics.sleep_after),
            fluid: world::generate_fluids(&config)
                .ok_or_else(|| SimError::InvalidConfig("could not reserve fluids".into()))?,
            balance: fluids::FluidBalance::default(),
            aquifer_sources: Vec::with_capacity(1 << 16),
            heat: ChunkDoubleBuffer::new(
                world.chunk_count(),
                config.physics.heat_pool_chunks as usize,
            ),
            heat_balance: temperature::HeatBalance::default(),
            heat_sources: Vec::with_capacity(1 << 16),
            support: SupportChecker::new(world.len(), 1 << 16),
            item_index: CategoryIndex::new(w, w, d, content.categories.len() as u8, items)
                .ok_or_else(|| SimError::InvalidConfig("could not create item index".into()))?,
            jobs: JobBoard::new(config.max_jobs as usize, w, w, d),
            reservations: Reservations::with_capacity(residents as usize * 2 + 16),
            queue: PathRequestQueue::with_capacity(residents as usize),
            stockpiles: Vec::with_capacity(STOCKPILE_CAPACITY),
            events: EventQueue::with_capacity(16),
            ops: EntityCommandBuffer::with_capacity(
                residents as usize * 2 + items as usize / 4 + 4096,
            ),
            counters: Counters::default(),
            diag: Diagnostics {
                lowest_level: i32::MAX,
                ..Diagnostics::default()
            },
            scratch: Scratch::new(&config),
            load_warnings: Vec::new(),
            world,
            config,
            content,
        })
    }

    /// Prepares derived physics state after creation or loading.
    fn init_physics(&mut self) {
        self.scratch.fluid_marks.clear();
        self.scratch
            .fluid_marks
            .resize(self.fluid.chunk_count(), false);
        self.fluid_active
            .set_sleeping_enabled(self.config.physics.sleeping);
        self.wake_fluid_chunks();
        self.rebuild_aquifer_sources();
        self.rebuild_heat_sources();
    }

    /// Records a death once per tick and despawns the resident at the tick end.
    pub(crate) fn kill(&mut self, id: ResidentId, cause: Death) {
        if self.scratch.dead.contains(&id) || !self.residents.is_alive(id) {
            return;
        }
        self.scratch.dead.push(id);
        let count = &mut self.counters.deaths[cause as usize];
        *count = count.saturating_add(1);
        self.ops.push(
            (SOURCE_RESIDENT, id.to_u64()),
            1,
            EntityCommand::Spawn(Op::DespawnResident(id)),
        );
    }

    /// Nearest safe tile in rings around `at`: standable, dry, not hot, and
    /// reachable; a swimming resident only considers adjacent tiles.
    fn flee_target(&self, at: Pos3) -> Option<Pos3> {
        let swimming = !self.connectivity.is_standable(at);
        let max = if swimming { 1 } else { 12 };
        for r in 1..=max {
            for dz in [0, 1, -1] {
                if swimming && dz != 0 {
                    continue;
                }
                for y in at.y - r..=at.y + r {
                    for x in at.x - r..=at.x + r {
                        if (x - at.x).abs().max((y - at.y).abs()) != r {
                            continue;
                        }
                        let p = Pos3::new(x, y, at.z + dz);
                        if self.connectivity.is_standable(p)
                            && fluids::level(self.fluid_at(p)) == 0
                            && !self.too_hot(p)
                            && (swimming || self.connectivity.reachable(at, p))
                        {
                            return Some(p);
                        }
                    }
                }
            }
        }
        None
    }

    /// Drowning, heat damage and the decision to flee.
    fn hazards(&mut self) {
        self.scratch.abort.clear();
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else { continue };
            let at = self.rcols.position[slot];
            let fluid = self.fluid_at(at);
            let deep = fluids::level(fluid) > 0 && self.impassable_fluid(fluid);
            let hot = self.too_hot(at);
            let vitals = &mut self.rcols.vitals[slot];
            vitals.submerged = if deep {
                vitals.submerged.saturating_add(1)
            } else {
                0
            };
            if hot {
                vitals.health = vitals.health.saturating_sub(1);
            }
            if vitals.submerged >= self.config.physics.drown_ticks || vitals.health == 0 {
                self.scratch.abort.push(id);
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            let cause = match self.rslot(id) {
                Some(slot) if self.rcols.vitals[slot].health == 0 => Death::Heat,
                _ => Death::Drowned,
            };
            self.kill(id, cause);
        }
        self.scratch.abort.clear();
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else { continue };
            let at = self.rcols.position[slot];
            let wet = fluids::level(self.fluid_at(at)) >= 2 || !self.connectivity.is_standable(at);
            if (wet || self.too_hot(at)) && !matches!(self.rcols.task[slot], Task::Flee { .. }) {
                self.scratch.abort.push(id);
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.abort_task(id);
            let Some(slot) = self.rslot(id) else { continue };
            let at = self.rcols.position[slot];
            if let Some(target) = self.flee_target(at) {
                if self.connectivity.is_standable(at) {
                    self.rcols.task[slot] = Task::Flee { target };
                } else {
                    // Swimming: one step to an adjacent standable tile.
                    self.rcols.position[slot] = target;
                }
            }
        }
    }

    pub(crate) fn rslot(&self, id: ResidentId) -> Option<usize> {
        self.residents.slot(id).map(|slot| slot as usize)
    }

    pub(crate) fn islot(&self, id: ItemId) -> Option<usize> {
        self.items.slot(id).map(|slot| slot as usize)
    }

    fn path_base(&self, id: ResidentId) -> usize {
        id.index as usize * self.config.max_path_nodes as usize
    }

    pub(crate) fn route_nodes(&self, id: ResidentId) -> impl Iterator<Item = Pos3> + '_ {
        let len = self
            .rslot(id)
            .map_or(0, |slot| usize::from(self.rcols.route[slot].len));
        let base = self.path_base(id);
        self.paths[base..base + len]
            .iter()
            .filter_map(|&index| self.world.pos(index as usize))
    }

    fn is_stockpile(&self, p: Pos3) -> bool {
        self.stockpiles.binary_search(&p).is_ok()
    }

    fn standing_tiles_near(&self, center: Pos3, count: usize, out: &mut Vec<Pos3>) {
        out.clear();
        let embark = self.embark();
        let size = self.config.world_size_xy;
        for radius in 0..size {
            for y in center.y - radius..=center.y + radius {
                for x in center.x - radius..=center.x + radius {
                    if (x - center.x).abs().max((y - center.y).abs()) != radius {
                        continue;
                    }
                    let p = Pos3::new(x, y, world::surface(self.seed, &self.config, x, y));
                    if self.world.get(p).is_some_and(|t| standable(*t))
                        && self.connectivity.reachable(embark, p)
                    {
                        out.push(p);
                        if out.len() >= count {
                            return;
                        }
                    }
                }
            }
        }
    }

    fn embark(&self) -> Pos3 {
        let (x, y) = (self.config.embark_x, self.config.embark_y);
        Pos3::new(x, y, world::surface(self.seed, &self.config, x, y))
    }

    fn spawn_resident(&mut self, at: Pos3) -> Option<ResidentId> {
        let id = self.residents.create()?;
        let random = |salt| stateless_u64(self.seed, self.tick, id.to_u64(), salt);
        self.rcols.push(
            at,
            Needs {
                hunger: below(random(1), u64::from(self.config.hungry_at) / 2) as u16,
                fatigue: below(random(2), u64::from(self.config.tired_at) / 2) as u16,
            },
            Vitals {
                health: HEALTH,
                submerged: 0,
            },
            Task::Idle,
            None,
            Route::NONE,
            0,
        );
        Some(id)
    }

    fn spawn_item(&mut self, at: Pos3, kind: ItemKind, material: u16) -> Option<ItemId> {
        // Stockpile slots hold one item; spawns there move like drops.
        let at = if self.is_stockpile(at) {
            self.drop_position(at)
        } else {
            at
        };
        let id = self.items.create()?;
        self.icols
            .push(ItemLocation::Ground(at), kind, material, None);
        self.index_item(id);
        self.ensure_haul(id);
        Some(id)
    }

    /// Creates a haul job for a loose item outside stockpiles.
    fn ensure_haul(&mut self, id: ItemId) {
        let Some(slot) = self.islot(id) else { return };
        let ItemLocation::Ground(at) = self.icols.location[slot] else {
            return;
        };
        if self.icols.haul[slot].is_some()
            || self.is_stockpile(at)
            || self.reservations.is_reserved(ResKey::Item(id))
            || self.stockpiles.is_empty()
        {
            return;
        }
        if let Some(job) = self.jobs.add(HAUL_PRIORITY, at, JobKind::Haul(id)) {
            self.icols.haul[slot] = Some(job);
        }
    }

    fn remove_haul(&mut self, id: ItemId) {
        if let Some(slot) = self.islot(id)
            && let Some(job) = self.icols.haul[slot].take()
        {
            if let Some(JobState::Claimed { by, .. }) = self.jobs.get(job).map(|j| j.state)
                && by != ResidentId::new(u32::MAX, u32::MAX)
            {
                self.scratch.abort.push(by);
            }
            self.jobs.remove(job);
        }
    }

    /// Abandons the current task: the job reopens, reservations and path requests
    /// are released and a carried item is dropped on the current tile.
    fn abort_task(&mut self, id: ResidentId) {
        let Some(slot) = self.rslot(id) else { return };
        let task = self.rcols.task[slot];
        if let Some(job) = task.job() {
            self.jobs.release(job);
        }
        self.reservations.release_holder(id);
        self.queue.cancel(id);
        if let Some(item) = self.rcols.carried[slot].take()
            && let Some(islot) = self.islot(item)
        {
            let at = self.drop_position(self.rcols.position[slot]);
            self.icols.location[islot] = ItemLocation::Ground(at);
            self.index_item(item);
            if let Task::Haul { job, .. } = task {
                // The haul job is bound to the item's former position.
                self.icols.haul[islot] = None;
                self.jobs.remove(job);
            }
            self.ensure_haul(item);
        }
        if !matches!(task, Task::Idle | Task::Sleep) {
            self.counters.aborted_tasks = self.counters.aborted_tasks.saturating_add(1);
        }
        self.rcols.task[slot] = Task::Idle;
        self.rcols.route[slot] = Route::NONE;
        self.rcols.work[slot] = 0;
    }

    /// Synchronizes the category index with the item's current location.
    pub(crate) fn index_item(&mut self, id: ItemId) {
        match self.islot(id) {
            Some(slot) => match self.icols.location[slot] {
                ItemLocation::Ground(p) => {
                    let category = self.content.category_of(self.icols.kind[slot].0);
                    self.item_index.insert(id.index, id, p, category);
                }
                ItemLocation::Carried(_) => {
                    self.item_index.remove(id.index);
                }
            },
            None => {
                self.item_index.remove(id.index);
            }
        }
    }

    pub(crate) fn rebuild_item_index(&mut self) {
        self.item_index.clear();
        for id in self.items.iter_by_id().collect::<Vec<_>>() {
            self.index_item(id);
        }
    }

    /// Loose, unreserved items of a category.
    fn free_items(&self, category: u8) -> u32 {
        let reserved = self
            .reservations
            .iter()
            .filter(|entry| match entry.key {
                ResKey::Item(item) => self
                    .item_index
                    .get(item.index)
                    .is_some_and(|(key, _, c)| key == item && c == category),
                ResKey::Slot(_) => false,
            })
            .count() as u32;
        self.item_index.count(category).saturating_sub(reserved)
    }

    /// Nearest loose, unreserved and reachable item by `(distance, id)`, the same
    /// minimum as an exhaustive scan.
    fn nearest_free_item(&self, at: Pos3, category: u8) -> Option<(u32, ItemId)> {
        self.item_index
            .nearest(at, category, u32::MAX, |item, p| {
                !self.reservations.is_reserved(ResKey::Item(item))
                    && self.connectivity.reachable(at, p)
            })
            .found
            .map(|(distance, item, _)| (distance, item))
    }

    fn item_ground(&self, id: ItemId) -> Option<Pos3> {
        match self.icols.location[self.islot(id)?] {
            ItemLocation::Ground(p) => Some(p),
            ItemLocation::Carried(_) => None,
        }
    }

    /// Travel target of the current task stage.
    fn goal(&self, slot: usize) -> Option<Pos3> {
        match self.rcols.task[slot] {
            Task::Idle | Task::Sleep => None,
            Task::Flee { target } => Some(target),
            Task::Eat { item } => self.item_ground(item),
            Task::Dig { site, .. } => Some(site),
            Task::Haul {
                item,
                slot: dest,
                stage,
                ..
            } => {
                if stage == 0 {
                    self.item_ground(item)
                } else {
                    Some(dest)
                }
            }
            Task::Build {
                item, site, stage, ..
            } => {
                if stage == 0 {
                    self.item_ground(item)
                } else {
                    Some(site)
                }
            }
        }
    }

    fn drop_position(&self, at: Pos3) -> Pos3 {
        let usable = |p: Pos3| {
            self.connectivity.is_standable(p)
                && match self.stockpiles.binary_search(&p) {
                    Err(_) => true,
                    Ok(_) => !self.reservations.is_reserved(ResKey::Slot(p)) && !self.items_on(p),
                }
        };
        // Rings grow until a usable tile appears; the world bounds the search.
        (0..=self.config.world_size_xy)
            .flat_map(|r: i32| (-r..=r).flat_map(move |dy| (-r..=r).map(move |dx| (r, dx, dy))))
            .filter(|&(r, dx, dy)| dx.abs().max(dy.abs()) == r)
            .map(|(_, dx, dy)| at.offset(dx, dy, 0))
            .find(|&p| usable(p))
            .unwrap_or(at)
    }

    fn items_on(&self, p: Pos3) -> bool {
        self.item_index.any_at(p)
    }

    /// Nearest tile within three steps on the same level to clear a build site.
    fn dump_near(&self, from: Pos3) -> Option<Pos3> {
        (1..=3)
            .flat_map(|r: i32| (-r..=r).flat_map(move |dy| (-r..=r).map(move |dx| (r, dx, dy))))
            .filter(|&(r, dx, dy)| dx.abs().max(dy.abs()) == r)
            .map(|(_, dx, dy)| from.offset(dx, dy, 0))
            .find(|&p| {
                self.connectivity.reachable(from, p)
                    && !self.is_stockpile(p)
                    && self.scratch.job_positions.binary_search(&p).is_err()
                    && !self.reservations.is_reserved(ResKey::Slot(p))
            })
    }

    /// Nearest work site reachable from `from`, by distance then position.
    fn best_site(&self, from: Pos3, job: JobId) -> Option<Pos3> {
        let job = self.jobs.get(job)?;
        let sites = match job.data {
            JobKind::Dig(mode) => dig_sites(&self.world, &self.connectivity, job.pos, mode),
            JobKind::Build(kind) => {
                if !build_target_valid(*self.world.get(job.pos)?, kind) {
                    return None;
                }
                build_sites(&self.connectivity, job.pos)
            }
            JobKind::Haul(_) => return None,
        };
        sites
            .into_iter()
            .flatten()
            .filter(|&site| self.connectivity.reachable(from, site))
            .min_by_key(|&site| (job_distance(from, site), site))
    }

    fn rebuild_stock(&mut self) {
        let counts = &mut self.scratch.stock_counts;
        counts.clear();
        counts.resize(self.stockpiles.len(), 0);
        for slot in 0..self.icols.len() {
            if let ItemLocation::Ground(p) = self.icols.location[slot]
                && let Ok(index) = self.stockpiles.binary_search(&p)
            {
                counts[index] = counts[index].saturating_add(1);
            }
        }
    }

    fn free_slot_near(&self, from: Pos3) -> Option<Pos3> {
        self.stockpiles
            .iter()
            .zip(&self.scratch.stock_counts)
            .filter(|&(&p, &count)| {
                count == 0
                    && !self.reservations.is_reserved(ResKey::Slot(p))
                    && self.connectivity.reachable(from, p)
            })
            .map(|(&p, _)| (job_distance(from, p), p))
            .min()
            .map(|(_, p)| p)
    }

    // ---------------------------------------------------------------- phases

    fn shuffle_slots(&mut self) {
        let scratch = &mut self.scratch;
        for table in 0..2u64 {
            let len = if table == 0 {
                self.residents.len()
            } else {
                self.items.len()
            };
            scratch.order.clear();
            scratch.order.extend(0..len);
            for end in (1..len).rev() {
                let random = stateless_u64(self.seed, self.tick, end as u64, 0x534c_4f54 + table);
                scratch
                    .order
                    .swap(end, below(random, end as u64 + 1) as usize);
            }
            if scratch.permutation.prepare(&scratch.order) {
                if table == 0 {
                    self.residents.apply_permutation(&scratch.permutation);
                    self.rcols.apply_permutation(&scratch.permutation);
                } else {
                    self.items.apply_permutation(&scratch.permutation);
                    self.icols.apply_permutation(&scratch.permutation);
                }
            }
        }
    }

    fn commands(&mut self, commands: &[FortressCommand]) {
        if commands.is_empty() {
            return;
        }
        let positions = &mut self.scratch.job_positions;
        positions.clear();
        positions.extend(
            self.jobs
                .iter()
                .filter(|job| !matches!(job.data, JobKind::Haul(_)))
                .map(|job| job.pos),
        );
        positions.sort_unstable();
        let max_jobs = u64::from(self.config.max_jobs);
        for (index, command) in commands.iter().enumerate() {
            match *command {
                FortressCommand::DesignateDig { from, to, mode } => {
                    let (low, high) = box_range(from, to);
                    if box_volume(low, high) > max_jobs {
                        self.counters.invalid_designations += 1;
                        continue;
                    }
                    for_box(low, high, |p| {
                        let valid = self.world.get(p).is_some_and(|t| t.kind == TileKind::Solid);
                        let at = self.scratch.job_positions.partition_point(|&q| q < p);
                        let free = self.scratch.job_positions.get(at) != Some(&p);
                        if valid
                            && free
                            && self.jobs.add(DIG_PRIORITY, p, JobKind::Dig(mode)).is_some()
                        {
                            self.scratch.job_positions.insert(at, p);
                            return;
                        }
                        self.counters.invalid_designations += 1;
                    });
                }
                FortressCommand::DesignateStockpile { from, to } => {
                    let (low, high) = box_range(from, to);
                    if box_volume(low, high) > STOCKPILE_CAPACITY as u64 {
                        self.counters.invalid_designations += 1;
                        continue;
                    }
                    for_box(low, high, |p| {
                        let at = self.stockpiles.partition_point(|&q| q < p);
                        if self.stockpiles.get(at) == Some(&p) {
                            return;
                        }
                        // A slot holds at most one item, so stacked tiles are not designated.
                        if self.world.get(p).is_some_and(|t| standable(*t))
                            && self.item_index.count_at(p, 2) < 2
                            && self.stockpiles.len() < STOCKPILE_CAPACITY
                        {
                            self.stockpiles.insert(at, p);
                        } else {
                            self.counters.invalid_designations += 1;
                        }
                    });
                }
                FortressCommand::Build { pos, kind } => {
                    let valid = self
                        .world
                        .get(pos)
                        .is_some_and(|t| build_target_valid(*t, kind));
                    let at = self.scratch.job_positions.partition_point(|&q| q < pos);
                    let free = self.scratch.job_positions.get(at) != Some(&pos);
                    if valid
                        && free
                        && self
                            .jobs
                            .add(BUILD_PRIORITY, pos, JobKind::Build(kind))
                            .is_some()
                    {
                        self.scratch.job_positions.insert(at, pos);
                    } else {
                        self.counters.invalid_designations += 1;
                    }
                }
                FortressCommand::CancelDesignations { from, to } => {
                    let (low, high) = box_range(from, to);
                    let inside = |p: Pos3| {
                        (low.x..=high.x).contains(&p.x)
                            && (low.y..=high.y).contains(&p.y)
                            && (low.z..=high.z).contains(&p.z)
                    };
                    self.scratch.abort.clear();
                    for job in self.jobs.iter() {
                        if inside(job.pos)
                            && !matches!(job.data, JobKind::Haul(_))
                            && let JobState::Claimed { by, .. } = job.state
                        {
                            self.scratch.abort.push(by);
                        }
                    }
                    for i in 0..self.scratch.abort.len() {
                        let id = self.scratch.abort[i];
                        self.abort_task(id);
                    }
                    self.jobs
                        .retain(|job| !(inside(job.pos) && !matches!(job.data, JobKind::Haul(_))));
                    self.stockpiles.retain(|&p| !inside(p));
                    self.scratch.job_positions.retain(|&p| !inside(p));
                }
                FortressCommand::SpawnResidents { count } => {
                    let room = self.config.max_residents as usize - self.residents.len();
                    for ordinal in 0..(count as usize).min(room) {
                        self.ops.push(
                            (SOURCE_COMMAND, index as u64),
                            ordinal as u32,
                            EntityCommand::Spawn(Op::SpawnResident),
                        );
                    }
                }
            }
        }
    }

    fn update_connectivity(&mut self) {
        self.world.take_changed_chunks(&mut self.scratch.chunks);
        for i in 0..self.scratch.chunks.len() {
            let chunk = self.scratch.chunks[i];
            self.fluid_active.wake_around(chunk);
        }
        if !self.scratch.chunks.is_empty() {
            let chunks = std::mem::take(&mut self.scratch.chunks);
            self.update_aquifer_sources(&chunks);
            self.update_heat_sources(&chunks);
            self.scratch.chunks = chunks;
        }
        let changed = self.scratch.chunks.len() as u64;
        self.diag.changed_chunks += changed;
        self.diag.last_changed_chunks = changed;
        if changed != 0 {
            self.connectivity.update(&self.world, &self.scratch.chunks);
        }
    }

    fn needs(&mut self) {
        let mut hungry_any = false;
        for slot in 0..self.rcols.len() {
            let needs = &mut self.rcols.needs[slot];
            needs.hunger = needs.hunger.saturating_add(self.config.hunger_rate);
            if self.rcols.task[slot] == Task::Sleep {
                needs.fatigue = needs.fatigue.saturating_sub(self.config.sleep_recovery);
            } else {
                needs.fatigue = needs.fatigue.saturating_add(self.config.fatigue_rate);
            }
            hungry_any |= needs.hunger >= self.config.hungry_at;
        }
        self.scratch.abort.clear();
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else { continue };
            let needs = self.rcols.needs[slot];
            let task = self.rcols.task[slot];
            if task == Task::Sleep && needs.fatigue == 0 {
                self.rcols.task[slot] = Task::Idle;
            } else if (needs.hunger >= self.config.hungry_at
                && !matches!(task, Task::Eat { .. } | Task::Idle))
                || (needs.fatigue >= self.config.tired_at
                    && !matches!(task, Task::Eat { .. } | Task::Sleep))
            {
                self.scratch.abort.push(id);
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.abort_task(id);
            if let Some(slot) = self.rslot(id)
                && self.rcols.needs[slot].hunger < self.config.hungry_at
            {
                self.rcols.task[slot] = Task::Sleep;
            }
        }
        if !hungry_any {
            return;
        }
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else { continue };
            if self.rcols.task[slot] != Task::Idle
                || self.rcols.needs[slot].hunger < self.config.hungry_at
                || !staggered(self.tick, 4, u64::from(id.index))
            {
                continue;
            }
            let at = self.rcols.position[slot];
            if let Some((distance, item)) = self.nearest_free_item(at, self.content.food) {
                self.reservations
                    .claim(ResKey::Item(item), id, FOOD_PRIORITY, distance);
            }
        }
        let mut outcomes = std::mem::take(&mut self.scratch.outcomes);
        outcomes.clear();
        self.reservations.resolve(self.tick, |claim, won| {
            outcomes.push((claim.holder, claim.key, won));
        });
        for &(holder, key, won) in &outcomes {
            if let (true, ResKey::Item(item), Some(slot)) = (won, key, self.rslot(holder)) {
                self.rcols.task[slot] = Task::Eat { item };
                self.rcols.route[slot] = Route::NONE;
                self.rcols.work[slot] = 0;
            }
        }
        self.scratch.outcomes = outcomes;
    }

    fn job_assign(&mut self) {
        self.scratch.workers.clear();
        for id in self.residents.iter_by_id() {
            if let Some(slot) = self.rslot(id)
                && self.rcols.task[slot] == Task::Idle
                && self.rcols.needs[slot].hunger < self.config.hungry_at
            {
                self.scratch.workers.push((id, self.rcols.position[slot]));
            }
        }
        if self.scratch.workers.is_empty() {
            return;
        }
        self.rebuild_stock();
        let targets = &mut self.scratch.job_positions;
        targets.clear();
        targets.extend(
            self.jobs
                .iter()
                .filter(|job| matches!(job.data, JobKind::Build(_)) && job.state != JobState::Done)
                .map(|job| job.pos),
        );
        targets.sort_unstable();
        let free_slots = self
            .scratch
            .stock_counts
            .iter()
            .zip(&self.stockpiles)
            .filter(|&(&count, &p)| count == 0 && !self.reservations.is_reserved(ResKey::Slot(p)))
            .count();
        let stones = self.free_items(self.content.stone) > 0;
        {
            let (world, connectivity, reservations, icols, items) = (
                &self.world,
                &self.connectivity,
                &self.reservations,
                &self.icols,
                &self.items,
            );
            let targets = &self.scratch.job_positions;
            self.jobs.rebuild_index(|job| match job.data {
                JobKind::Dig(mode) => has_site(&dig_sites(world, connectivity, job.pos, mode)),
                JobKind::Build(_) => stones && has_site(&build_sites(connectivity, job.pos)),
                JobKind::Haul(item) => {
                    !reservations.is_reserved(ResKey::Item(item))
                        && items.slot(item).is_some_and(|slot| {
                            icols.location[slot as usize] == ItemLocation::Ground(job.pos)
                        })
                        && (free_slots > 0 || targets.binary_search(&job.pos).is_ok())
                }
            });
        }
        let mut assigner = std::mem::take(&mut self.scratch.assigner);
        let mut assignments = std::mem::take(&mut self.scratch.assignments);
        assigner.assign(
            &self.jobs,
            &self.scratch.workers,
            self.config.job_candidates as usize,
            self.config.job_inspect_limit as usize,
            |_, at, job| match job.data {
                JobKind::Dig(_) => self.best_site(at, job.id).is_some(),
                JobKind::Build(_) => stones && self.best_site(at, job.id).is_some(),
                JobKind::Haul(_) => self.connectivity.reachable(at, job.pos),
            },
            |job, distance| {
                let clearing = matches!(job.data, JobKind::Haul(_))
                    && self.scratch.job_positions.binary_search(&job.pos).is_ok();
                let priority = if clearing {
                    CLEAR_PRIORITY
                } else {
                    job.priority
                };
                i64::from(priority) * 1_000_000 - i64::from(distance)
            },
            &mut assignments,
        );
        self.scratch.pending.clear();
        for &(worker, job_id) in &assignments {
            let (Some(slot), Some(job)) = (self.rslot(worker), self.jobs.get(job_id)) else {
                continue;
            };
            let at = self.rcols.position[slot];
            let priority = job.priority;
            let task = match job.data {
                JobKind::Dig(_) => self
                    .best_site(at, job_id)
                    .map(|site| Task::Dig { job: job_id, site }),
                JobKind::Haul(item) => self.item_ground(item).and_then(|p| {
                    let clearing = self.scratch.job_positions.binary_search(&p).is_ok();
                    let dest = self
                        .free_slot_near(p)
                        .or_else(|| clearing.then(|| self.dump_near(p)).flatten())?;
                    self.reservations.claim(
                        ResKey::Item(item),
                        worker,
                        priority,
                        job_distance(at, p),
                    );
                    self.reservations.claim(
                        ResKey::Slot(dest),
                        worker,
                        priority,
                        job_distance(p, dest),
                    );
                    Some(Task::Haul {
                        job: job_id,
                        item,
                        slot: dest,
                        stage: 0,
                    })
                }),
                JobKind::Build(_) => self.best_site(at, job_id).and_then(|site| {
                    let (distance, stone) = self.nearest_free_item(at, self.content.stone)?;
                    self.reservations
                        .claim(ResKey::Item(stone), worker, priority, distance);
                    Some(Task::Build {
                        job: job_id,
                        item: stone,
                        site,
                        stage: 0,
                    })
                }),
            };
            if let Some(task) = task {
                self.scratch.pending.push((worker, task));
            }
        }
        let mut outcomes = std::mem::take(&mut self.scratch.outcomes);
        outcomes.clear();
        self.reservations.resolve(self.tick, |claim, won| {
            outcomes.push((claim.holder, claim.key, won));
        });
        // A task starts only if all of its claims were granted.
        outcomes.sort_unstable_by_key(|&(holder, key, won)| (holder, won, key));
        for i in 0..self.scratch.pending.len() {
            let (worker, task) = self.scratch.pending[i];
            let start = outcomes.partition_point(|&(holder, _, _)| holder < worker);
            let end = outcomes.partition_point(|&(holder, _, _)| holder <= worker);
            let all_won = outcomes[start..end].iter().all(|&(_, _, won)| won);
            if all_won
                && let (Some(job), Some(slot)) = (task.job(), self.rslot(worker))
                && self.jobs.claim(job, worker, self.tick)
            {
                self.rcols.task[slot] = task;
                self.rcols.route[slot] = Route::NONE;
                self.rcols.work[slot] = 0;
                continue;
            }
            for &(_, key, won) in &outcomes[start..end] {
                if won {
                    self.reservations.release(key);
                }
            }
        }
        self.scratch.outcomes = outcomes;
        self.scratch.assignments = assignments;
        self.scratch.assigner = assigner;
    }

    fn path(&mut self) {
        self.scratch.abort.clear();
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else { continue };
            if self.rcols.route[slot].status != RouteStatus::None {
                continue;
            }
            let Some(goal) = self.goal(slot) else {
                continue;
            };
            let at = self.rcols.position[slot];
            if at == goal {
                continue;
            }
            if self.connectivity.reachable(at, goal) {
                self.queue.push(id, at, goal, self.tick);
                self.rcols.route[slot].status = RouteStatus::Waiting;
            } else {
                self.diag.unreachable_prechecks += 1;
                self.scratch.abort.push(id);
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.abort_task(id);
        }
        if self.queue.is_empty() {
            return;
        }
        let cap = self.config.max_path_nodes as usize;
        let failed = &mut self.scratch.failed;
        failed.clear();
        let world = &self.world;
        let pathfinder = &mut self.pathfinder;
        let connectivity = &self.connectivity;
        let rcols = &mut self.rcols;
        let residents = &self.residents;
        let paths = &mut self.paths;
        let diag = &mut self.diag;
        let stats = self.queue.process(
            self.tick,
            self.config.path_budget_per_tick,
            self.config.path_expansion_limit,
            |a, b| connectivity.reachable(a, b),
            &mut self.scratch.path,
            |start, goal, limit, out| pathfinder.search(world, start, goal, limit, out),
            |request, outcome, nodes| {
                let Some(slot) = residents.slot(request.key).map(|s| s as usize) else {
                    return;
                };
                let route = &mut rcols.route[slot];
                match outcome.result {
                    PathResult::Found { cost }
                        if nodes.len() <= cap && nodes.len() <= usize::from(u16::MAX) =>
                    {
                        diag.path_cost += u64::from(cost);
                        if nodes.len() >= LONG_PATH {
                            diag.long_paths += 1;
                            diag.long_path_nodes += nodes.len() as u64;
                        }
                        let base = request.key.index as usize * cap;
                        for (i, p) in nodes.iter().enumerate() {
                            paths[base + i] = world.index(*p).unwrap_or(0) as u32;
                        }
                        *route = Route {
                            status: RouteStatus::Moving,
                            len: nodes.len() as u16,
                            next: 1,
                            progress: 0,
                            failures: route.failures,
                        };
                    }
                    _ => {
                        route.status = RouteStatus::None;
                        route.failures = route.failures.saturating_add(1);
                        failed.push((request.key, route.failures >= MAX_ROUTE_FAILURES));
                    }
                }
            },
        );
        self.diag.path.add(&stats);
        self.diag.path_ticks += 1;
        self.diag.route_failures += self.scratch.failed.len() as u64;
        for i in 0..self.scratch.failed.len() {
            let (id, give_up) = self.scratch.failed[i];
            if give_up {
                self.abort_task(id);
                if let Some(slot) = self.rslot(id) {
                    self.rcols.route[slot].failures = 0;
                }
            }
        }
    }

    fn movement(&mut self) {
        let cap = self.config.max_path_nodes as usize;
        for slot in 0..self.rcols.len() {
            let route = self.rcols.route[slot];
            if route.status != RouteStatus::Moving {
                continue;
            }
            let Some(id) = self.residents.id(slot as u32) else {
                continue;
            };
            let base = id.index as usize * cap;
            let mut route = route;
            let mut at = self.rcols.position[slot];
            route.progress = route.progress.saturating_add(self.config.move_speed);
            while route.next < route.len {
                let Some(next) = self
                    .world
                    .pos(self.paths[base + usize::from(route.next)] as usize)
                else {
                    route = Route::NONE;
                    break;
                };
                self.scratch.neighbors.clear();
                walk_neighbors(&self.world, at, &mut self.scratch.neighbors);
                let Some(&(_, cost)) = self.scratch.neighbors.iter().find(|(p, _)| *p == next)
                else {
                    self.diag.blocked_steps += 1;
                    route = Route::NONE;
                    break;
                };
                if route.progress < cost {
                    break;
                }
                route.progress -= cost;
                at = next;
                route.next += 1;
            }
            if route.status == RouteStatus::Moving && route.next >= route.len {
                route = Route::NONE;
            }
            self.diag.lowest_level = self.diag.lowest_level.min(at.z);
            self.rcols.position[slot] = at;
            self.rcols.route[slot] = Route {
                failures: if route.status == RouteStatus::None && route.len == 0 {
                    0
                } else {
                    route.failures
                },
                ..route
            };
        }
    }

    fn release_and_idle(&mut self, slot: usize, id: ResidentId) {
        self.reservations.release_holder(id);
        self.rcols.task[slot] = Task::Idle;
        self.rcols.route[slot] = Route::NONE;
        self.rcols.work[slot] = 0;
    }

    fn work(&mut self) {
        self.scratch.occupied.clear();
        self.scratch
            .occupied
            .extend(self.rcols.position.iter().copied());
        for slot in 0..self.icols.len() {
            if let ItemLocation::Ground(p) = self.icols.location[slot] {
                self.scratch.occupied.push(p);
            }
        }
        self.scratch.occupied.sort_unstable();
        self.scratch.tile_changes.clear();
        self.scratch.abort.clear();
        self.scratch.ids.clear();
        self.scratch.ids.extend(self.residents.iter_by_id());
        for index in 0..self.scratch.ids.len() {
            let id = self.scratch.ids[index];
            let Some(slot) = self.rslot(id) else { continue };
            let at = self.rcols.position[slot];
            let task = self.rcols.task[slot];
            if task == Task::Idle
                || self.rcols.route[slot].status != RouteStatus::None
                || self.goal(slot).is_some_and(|goal| goal != at)
            {
                continue;
            }
            match task {
                Task::Idle | Task::Sleep => {}
                Task::Flee { .. } => self.release_and_idle(slot, id),
                Task::Eat { item } => {
                    if self.item_ground(item) != Some(at) {
                        self.scratch.abort.push(id);
                    } else if tick_work(&mut self.rcols.work[slot], self.config.eat_ticks) {
                        self.rcols.needs[slot].hunger = 0;
                        self.remove_haul(item);
                        self.ops.push(
                            (SOURCE_RESIDENT, id.to_u64()),
                            0,
                            EntityCommand::Spawn(Op::DespawnItem(item)),
                        );
                        self.counters.eaten += 1;
                        self.release_and_idle(slot, id);
                    }
                }
                Task::Dig { job, site } => {
                    let Some((target, mode)) = self.jobs.get(job).and_then(|j| match j.data {
                        JobKind::Dig(mode) => Some((j.pos, mode)),
                        _ => None,
                    }) else {
                        self.scratch.abort.push(id);
                        continue;
                    };
                    let Some(&tile) = self.world.get(target).filter(|t| t.kind == TileKind::Solid)
                    else {
                        self.scratch.abort.push(id);
                        continue;
                    };
                    debug_assert_eq!(site, at);
                    let material = self.content.material(tile.material);
                    let duration = material.map_or(1, |m| m.dig_ticks.max(1));
                    let product = material.and_then(|m| m.dig_item);
                    if tick_work(&mut self.rcols.work[slot], duration) {
                        self.scratch
                            .tile_changes
                            .push((target, dig_result(tile, mode)));
                        if let Some(kind) = product {
                            self.ops.push(
                                (SOURCE_RESIDENT, id.to_u64()),
                                0,
                                EntityCommand::Spawn(Op::SpawnItem {
                                    at: target,
                                    kind: ItemKind(kind),
                                    material: tile.material,
                                }),
                            );
                        }
                        self.jobs.complete(job);
                        self.counters.dug += 1;
                        self.counters.completed_jobs += 1;
                        self.release_and_idle(slot, id);
                    }
                }
                Task::Haul {
                    job,
                    item,
                    slot: dest,
                    stage,
                } => {
                    if stage == 0 {
                        if self.item_ground(item) != Some(at) {
                            self.scratch.abort.push(id);
                            continue;
                        }
                        self.pick_up(slot, id, item);
                        self.rcols.task[slot] = Task::Haul {
                            job,
                            item,
                            slot: dest,
                            stage: 1,
                        };
                    } else {
                        self.drop_item(slot, item, at);
                        if let Some(islot) = self.islot(item) {
                            self.icols.haul[islot] = None;
                        }
                        self.jobs.complete(job);
                        self.counters.hauled += 1;
                        self.counters.completed_jobs += 1;
                        self.release_and_idle(slot, id);
                    }
                }
                Task::Build {
                    job,
                    item,
                    site,
                    stage,
                } => {
                    if stage == 0 {
                        if self.item_ground(item) != Some(at) {
                            self.scratch.abort.push(id);
                            continue;
                        }
                        self.remove_haul(item);
                        self.pick_up(slot, id, item);
                        self.rcols.task[slot] = Task::Build {
                            job,
                            item,
                            site,
                            stage: 1,
                        };
                        continue;
                    }
                    let Some((target, kind)) = self.jobs.get(job).and_then(|j| match j.data {
                        JobKind::Build(kind) => Some((j.pos, kind)),
                        _ => None,
                    }) else {
                        self.scratch.abort.push(id);
                        continue;
                    };
                    if !self
                        .world
                        .get(target)
                        .is_some_and(|t| build_target_valid(*t, kind))
                    {
                        self.scratch.abort.push(id);
                        continue;
                    }
                    if self.rcols.work[slot] == 1
                        || tick_work(&mut self.rcols.work[slot], self.config.build_ticks)
                    {
                        if self.scratch.occupied.binary_search(&target).is_ok()
                            || self.fluid_at(target) != 0
                        {
                            // Wait deterministically until the tile is clear.
                            self.rcols.work[slot] = 1;
                            self.diag.build_waits += 1;
                            continue;
                        }
                        let material = self.islot(item).map_or(0, |s| self.icols.material[s]);
                        self.scratch
                            .tile_changes
                            .push((target, build_result(kind, material)));
                        self.rcols.carried[slot] = None;
                        self.ops.push(
                            (SOURCE_RESIDENT, id.to_u64()),
                            0,
                            EntityCommand::Spawn(Op::DespawnItem(item)),
                        );
                        self.jobs.complete(job);
                        self.counters.built += 1;
                        self.counters.completed_jobs += 1;
                        self.release_and_idle(slot, id);
                    }
                }
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.abort_task(id);
        }
        self.scratch.tile_changes.sort_unstable_by_key(|&(p, _)| p);
        for i in 0..self.scratch.tile_changes.len() {
            let (p, tile) = self.scratch.tile_changes[i];
            let old = self.world.get(p).copied().unwrap_or(tile);
            self.world.set(p, tile);
            self.note_tile_change(p, old, tile);
        }
    }

    fn pick_up(&mut self, slot: usize, id: ResidentId, item: ItemId) {
        if let Some(islot) = self.islot(item) {
            self.icols.location[islot] = ItemLocation::Carried(id);
            self.item_index.remove(item.index);
            self.rcols.carried[slot] = Some(item);
            self.rcols.route[slot] = Route::NONE;
        }
    }

    fn drop_item(&mut self, slot: usize, item: ItemId, at: Pos3) {
        if let Some(islot) = self.islot(item) {
            self.icols.location[islot] = ItemLocation::Ground(at);
            self.index_item(item);
        }
        self.rcols.carried[slot] = None;
    }

    fn lifecycle(&mut self) {
        self.scratch.abort.clear();
        for id in self.residents.iter_by_id() {
            if let Some(slot) = self.rslot(id)
                && self.rcols.needs[slot].hunger >= self.config.starve_at
            {
                self.scratch.abort.push(id);
            }
        }
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.kill(id, Death::Hunger);
        }
        self.scratch.abort.clear();
        self.reservations.expired(
            self.tick,
            self.config.reservation_timeout,
            &mut self.scratch.expired,
        );
        for i in 0..self.scratch.expired.len() {
            let (_, holder) = self.scratch.expired[i];
            self.scratch.abort.push(holder);
        }
        self.scratch.abort.sort_unstable();
        self.scratch.abort.dedup();
        for i in 0..self.scratch.abort.len() {
            let id = self.scratch.abort[i];
            self.abort_task(id);
        }
        // Remove finished jobs and designations whose target can no longer be worked.
        let world = &self.world;
        self.jobs.retain(|job| match (job.state, job.data) {
            (JobState::Done, _) => false,
            (JobState::Open, JobKind::Dig(_)) => world
                .get(job.pos)
                .is_some_and(|t| t.kind == TileKind::Solid),
            (JobState::Open, JobKind::Build(kind)) => world
                .get(job.pos)
                .is_some_and(|t| build_target_valid(*t, kind)),
            _ => true,
        });
        self.apply_ops();
        if !self.stockpiles.is_empty() {
            for slot in 0..self.icols.len() {
                if let Some(id) = self.items.id(slot as u32)
                    && self.icols.haul[slot].is_none()
                    && staggered(
                        self.tick,
                        self.config.haul_scan_interval,
                        u64::from(id.index),
                    )
                {
                    self.ensure_haul(id);
                }
            }
        }
    }

    fn apply_ops(&mut self) {
        let mut ops = std::mem::replace(&mut self.ops, EntityCommandBuffer::with_capacity(0));
        ops.apply(|op| {
            let EntityCommand::Spawn(op) = op else { return };
            match op {
                Op::SpawnResident => {
                    let mut spots = std::mem::take(&mut self.scratch.occupied);
                    self.standing_tiles_near(
                        self.embark(),
                        1 + self.residents.len() % 9,
                        &mut spots,
                    );
                    if let Some(&at) = spots.last() {
                        self.spawn_resident(at);
                    }
                    self.scratch.occupied = spots;
                }
                Op::SpawnItem { at, kind, material } => {
                    self.spawn_item(at, kind, material);
                }
                Op::DespawnItem(item) => {
                    self.remove_haul(item);
                    self.reservations.release(ResKey::Item(item));
                    if let Some(slot) = self.items.destroy(item) {
                        self.icols.swap_remove(slot);
                        self.item_index.remove(item.index);
                    }
                }
                Op::DespawnResident(id) => {
                    self.abort_task(id);
                    self.reservations.release_holder(id);
                    self.queue.cancel(id);
                    if let Some(slot) = self.residents.destroy(id) {
                        self.rcols.swap_remove(slot);
                    }
                }
            }
        });
        self.ops = ops;
        // Aborts requested by item removal.
        while let Some(id) = self.scratch.abort.pop() {
            self.abort_task(id);
        }
    }

    fn environment(&mut self) {
        while let Some(event) = self.events.pop_due(self.tick) {
            match event {
                Event::Caravan => {
                    self.counters.caravans += 1;
                    let food = (self.residents.len() as u32)
                        .saturating_mul(self.config.caravan_food_per_resident)
                        .max(self.config.caravan_food_min);
                    let edge = Pos3::new(0, self.config.embark_y, 0);
                    let mut spots = std::mem::take(&mut self.scratch.occupied);
                    self.standing_tiles_near(edge, 9, &mut spots);
                    for ordinal in 0..food {
                        if let Some(&at) = spots.get(ordinal as usize % spots.len().max(1)) {
                            self.ops.push(
                                (SOURCE_EVENT, self.counters.caravans),
                                ordinal,
                                EntityCommand::Spawn(Op::SpawnItem {
                                    at,
                                    kind: ItemKind(self.content.ration),
                                    material: self.content.ration_material,
                                }),
                            );
                        }
                    }
                    self.scratch.occupied = spots;
                    let next = self.tick.saturating_add(self.config.caravan_interval);
                    self.events.push(next, Event::Caravan);
                }
            }
        }
        self.apply_ops();
    }
}

impl Simulation for FortressSim {
    type Config = FortressConfig;
    type Command = FortressCommand;

    fn new(seed: u64, config: FortressConfig) -> Result<Self, SimError> {
        config.validate()?;
        let content = GameContent::load(&config.mods)
            .map_err(|error| SimError::InvalidConfig(error.to_string()))?;
        let world = world::generate(seed, &config, &content)?;
        let mut sim = Self::empty(seed, config, content, world)?;
        let mut spots = Vec::new();
        sim.standing_tiles_near(
            sim.embark(),
            sim.config.initial_residents as usize + 1,
            &mut spots,
        );
        if spots.is_empty() {
            return Err(SimError::InvalidConfig("no standable embark tile".into()));
        }
        for i in 0..sim.config.initial_residents as usize {
            sim.spawn_resident(spots[i % spots.len()]);
        }
        sim.standing_tiles_near(sim.embark(), 9, &mut spots);
        for i in 0..sim.config.starting_food() as usize {
            if let Some(&at) = spots.get(i % spots.len().max(1)) {
                sim.spawn_item(
                    at,
                    ItemKind(sim.content.ration),
                    sim.content.ration_material,
                );
            }
        }
        sim.events.push(sim.config.caravan_interval, Event::Caravan);
        sim.balance.initial = sim.count_fluids();
        sim.balance.total = sim.balance.initial;
        sim.init_physics();
        Ok(sim)
    }

    fn step(&mut self, commands: &[FortressCommand]) {
        self.step_profiled(commands, &mut NoopProbe);
    }

    fn step_profiled(&mut self, commands: &[FortressCommand], probe: &mut dyn PhaseProbe) {
        fn phase(probe: &mut dyn PhaseProbe, name: &'static str, work: impl FnOnce()) {
            probe.enter(name);
            work();
            probe.exit(name);
        }
        self.scratch.dead.clear();
        if self.config.debug_shuffle_slots {
            self.shuffle_slots();
        }
        phase(probe, "commands", || self.commands(commands));
        phase(probe, "connectivity", || self.update_connectivity());
        phase(probe, "needs", || {
            self.hazards();
            self.needs();
        });
        phase(probe, "job_assign", || self.job_assign());
        phase(probe, "path", || self.path());
        phase(probe, "move", || self.movement());
        phase(probe, "work", || self.work());
        phase(probe, "fluids", || self.fluids());
        phase(probe, "temperature", || self.temperature_step());
        phase(probe, "support", || self.support_step());
        phase(probe, "collapse", || self.collapse_step());
        phase(probe, "lifecycle", || self.lifecycle());
        phase(probe, "environment", || self.environment());
        self.tick = self.tick.saturating_add(1);
    }

    fn tick(&self) -> u64 {
        self.tick
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn metrics(&self) -> SimMetrics {
        self.collect_metrics()
    }

    fn state_hash(&self) -> u64 {
        self.hash_state()
    }

    fn subsystem_hashes(&self) -> Vec<(&'static str, u64)> {
        self.hashes().to_vec()
    }

    fn check_invariants(&self) -> Result<(), SimError> {
        self.validate_state()
    }

    fn snapshot_version() -> u32 {
        4
    }

    fn min_snapshot_version() -> u32 {
        4
    }

    fn load_warnings(&self) -> Vec<String> {
        self.load_warnings.clone()
    }

    fn save_snapshot(&self, out: &mut Vec<u8>) -> Result<(), SimError> {
        self.encode_snapshot(out)
    }

    fn load_snapshot(bytes: &[u8]) -> Result<Self, SimError> {
        Self::decode_snapshot(bytes)
    }

    fn content_fingerprint(&self) -> u64 {
        self.fingerprint()
    }

    fn debug_dump(&self, out: &mut String) -> Result<(), SimError> {
        self.write_debug_dump(out)
    }

    fn scale_units(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("entities", (self.residents.len() + self.items.len()) as u64),
            ("residents", self.residents.len() as u64),
            ("items", self.items.len() as u64),
            ("tiles", self.world.len() as u64),
            ("changed_chunks", self.diag.last_changed_chunks),
            ("active_fluid_tiles", self.diag.fluid_last_tiles),
            ("active_heat_tiles", self.diag.heat_last_tiles),
            ("support_checks", self.diag.support_last_units),
        ]
    }

    fn phase_dimensions(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("commands", "residents"),
            ("connectivity", "changed_chunks"),
            ("needs", "residents"),
            ("job_assign", "residents"),
            ("path", "residents"),
            ("move", "residents"),
            ("work", "residents"),
            ("lifecycle", "items"),
            ("environment", "tiles"),
            ("fluids", "active_fluid_tiles"),
            ("temperature", "active_heat_tiles"),
            ("support", "support_checks"),
            ("collapse", "support_checks"),
        ]
    }

    fn describe_entity(&self, id: u64) -> Option<Vec<(String, String)>> {
        self.describe(id)
    }
}

#[cfg(test)]
mod tests;
