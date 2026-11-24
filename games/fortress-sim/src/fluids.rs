//! Water and magma with exact mass conservation.
//!
//! Each tile holds one fluid kind with up to 112 units (16 per visible level, 7 bits
//! plus a magma bit) in a separate grid, so amount changes never touch tiles.
//! The fine units let water settle into slopes of one unit per tile instead of one
//! level, so it spreads over long distances. Per tick:
//! 1. Sources and sinks (exposed aquifer faces, river head and mouth) act on their
//!    tiles regardless of sleep and wake their chunks.
//! 2. Every awake chunk proposes transfers from the levels at the start of the
//!    phase: a tile without floor above a non-solid tile *falls* as much as fits;
//!    otherwise it *spreads* to horizontal neighbors at least two levels lower,
//!    sharing the total equally and handing out the remainder in a neighbor order
//!    that rotates with `(tick + x + y) % 4`. Shallow surface water may evaporate.
//! 3. Transfers are resolved per target in `(fall before spread, source)` order up
//!    to the target's free capacity at the start of the phase; the rest stays at
//!    the source. Water reaching magma, or magma reaching water, turns the target
//!    into obsidian and consumes both amounts.
//!
//! Only crossing the impassable thresholds changes tiles (and thus topology).
//! A chunk sleeps after four unchanged updates, one full rotation period, unless
//! it holds water that may evaporate.

use sim_core::{
    entity::EntityCommand,
    rng::stateless_u64,
    schedule::staggered,
    space::{ChunkId, Pos3, Tile, TileKind},
};

use crate::{
    FortressSim, SOURCE_EVENT,
    types::{Death, ItemLocation, Op},
    world::{self, MAGMA},
};

const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
const FALL: u8 = 0;
const SPREAD: u8 = 1;
/// Units removed by one evaporation event.
const EVAPORATION: u8 = 4;

/// Fluid units per visible level; a tile holds at most seven levels.
pub(crate) const UNIT: u8 = 16;
pub(crate) const MAX_AMOUNT: u8 = 7 * UNIT;

/// Stored fluid units, 0 to `MAX_AMOUNT`.
pub(crate) const fn amount(fluid: u8) -> u8 {
    fluid & 0x7f
}

/// Visible level 0 to 7; any fluid shows at least level 1.
pub(crate) const fn level(fluid: u8) -> u8 {
    amount(fluid).div_ceil(UNIT)
}

pub(crate) const fn is_magma(fluid: u8) -> bool {
    fluid & MAGMA != 0
}

const fn kind(fluid: u8) -> usize {
    if is_magma(fluid) { 1 } else { 0 }
}

/// Conservation counters per fluid kind (0 water, 1 magma). Logical state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FluidBalance {
    pub total: [u64; 2],
    pub initial: [u64; 2],
    pub sources: [u64; 2],
    pub sinks: [u64; 2],
    pub evaporated: u64,
    pub converted: [u64; 2],
    pub boiled: u64,
}

sim_core::state_hash_struct!(FluidBalance {
    total,
    initial,
    sources,
    sinks,
    evaporated,
    converted,
    boiled
});

impl FluidBalance {
    /// The conservation law: current totals follow from the initial amounts and
    /// all counted inflows and outflows.
    pub fn expected(&self, kind: usize) -> i128 {
        let gains = i128::from(self.initial[kind]) + i128::from(self.sources[kind]);
        let mut losses = i128::from(self.sinks[kind]) + i128::from(self.converted[kind]);
        if kind == 0 {
            losses += i128::from(self.evaporated) + i128::from(self.boiled);
        }
        gains - losses
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Transfer {
    target: Pos3,
    priority: u8,
    source: Pos3,
    amount: u8,
    fluid: u8,
}

impl FortressSim {
    pub(crate) fn fluid_at(&self, p: Pos3) -> u8 {
        self.fluid.get(p).copied().unwrap_or(0)
    }

    fn set_fluid(&mut self, p: Pos3, value: u8) {
        if self.fluid.get(p) != Some(&value) {
            self.fluid.set(p, value);
            if let Some(chunk) = self.fluid.chunk_of(p) {
                self.mark_fluid_chunk(chunk);
            }
        }
    }

    fn mark_fluid_chunk(&mut self, chunk: ChunkId) {
        if let Some(mark) = self.scratch.fluid_marks.get_mut(chunk as usize) {
            if !*mark {
                self.scratch.fluid_changed.push(chunk);
            }
            *mark = true;
        }
    }

    pub(crate) fn impassable_fluid(&self, fluid: u8) -> bool {
        let p = &self.config.physics;
        if is_magma(fluid) {
            level(fluid) >= p.magma_impassable_at
        } else {
            level(fluid) >= p.water_impassable_at
        }
    }

    fn evaporates(&self, p: Pos3, fluid: u8) -> bool {
        !is_magma(fluid) && (1..=2).contains(&level(fluid)) && p.z >= self.config.ground_level - 1
    }

    /// Recounts fluid totals; used at creation and by invariants.
    pub(crate) fn count_fluids(&self) -> [u64; 2] {
        let mut totals = [0u64; 2];
        for (_, &fluid) in self.fluid.iter_logical() {
            totals[kind(fluid)] += u64::from(amount(fluid));
        }
        totals
    }

    /// Wakes every chunk that holds fluid; used after creation and loading.
    pub(crate) fn wake_fluid_chunks(&mut self) {
        for chunk in 0..self.fluid.chunk_count() as ChunkId {
            let mut any = false;
            self.fluid.for_each_in_chunk(chunk, |_, f| any |= *f != 0);
            if any {
                self.fluid_active.wake(chunk);
            }
        }
    }

    fn is_aquifer_source(&self, p: Pos3) -> bool {
        let Some(aquifer) = self.config.aquifer_material() else {
            return false;
        };
        self.world.get(p).is_some_and(|t| t.kind != TileKind::Solid)
            && [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ]
            .into_iter()
            .any(|(dx, dy, dz)| {
                self.world
                    .get(p.offset(dx, dy, dz))
                    .is_some_and(|t| t.kind == TileKind::Solid && t.material == aquifer)
            })
    }

    /// Recomputes exposed aquifer faces inside the given chunks and one tile around.
    pub(crate) fn update_aquifer_sources(&mut self, chunks: &[ChunkId]) {
        if self.config.aquifer_material().is_none() {
            return;
        }
        for &chunk in chunks {
            let Some((low, high)) = self.world.chunk_bounds(chunk) else {
                continue;
            };
            let (low, high) = (low.offset(-1, -1, -1), high.offset(1, 1, 1));
            let inside = |p: &Pos3| {
                (low.x..high.x).contains(&p.x)
                    && (low.y..high.y).contains(&p.y)
                    && (low.z..high.z).contains(&p.z)
            };
            self.aquifer_sources.retain(|p| !inside(p));
            for z in low.z..high.z {
                for y in low.y..high.y {
                    for x in low.x..high.x {
                        let p = Pos3::new(x, y, z);
                        if self.is_aquifer_source(p)
                            && self.aquifer_sources.len() < self.aquifer_sources.capacity()
                        {
                            self.aquifer_sources.push(p);
                        }
                    }
                }
            }
        }
        self.aquifer_sources.sort_unstable();
        self.aquifer_sources.dedup();
    }

    pub(crate) fn rebuild_aquifer_sources(&mut self) {
        self.aquifer_sources.clear();
        let all: Vec<ChunkId> = (0..self.world.chunk_count() as ChunkId).collect();
        self.update_aquifer_sources(&all);
    }

    fn sources_and_sinks(&mut self) {
        for i in 0..self.aquifer_sources.len() {
            let p = self.aquifer_sources[i];
            let index = self.world.index(p).unwrap_or(0) as u64;
            let fluid = self.fluid_at(p);
            if staggered(self.tick, self.config.physics.aquifer_interval, index)
                && !is_magma(fluid)
                && amount(fluid) <= MAX_AMOUNT - UNIT
            {
                self.set_fluid(p, fluid + UNIT);
                self.balance.sources[0] += u64::from(UNIT);
                self.balance.total[0] += u64::from(UNIT);
            }
        }
        if let Some((xs, bed)) = world::river(&self.config) {
            let last = self.config.world_size_xy - 1;
            for x in xs {
                let head = Pos3::new(x, 0, bed);
                let fluid = self.fluid_at(head);
                let open = self
                    .world
                    .get(head)
                    .is_some_and(|t| t.kind != TileKind::Solid);
                if open && !is_magma(fluid) && amount(fluid) < MAX_AMOUNT {
                    let added = MAX_AMOUNT - amount(fluid);
                    self.set_fluid(head, MAX_AMOUNT);
                    self.balance.sources[0] += u64::from(added);
                    self.balance.total[0] += u64::from(added);
                }
                let mouth = Pos3::new(x, last, bed);
                let fluid = self.fluid_at(mouth);
                if amount(fluid) > 0 {
                    let k = kind(fluid);
                    self.set_fluid(mouth, 0);
                    self.balance.sinks[k] += u64::from(amount(fluid));
                    self.balance.total[k] -= u64::from(amount(fluid));
                }
            }
        }
    }

    /// Proposes transfers of one tile from the phase-start levels.
    fn propose(&mut self, p: Pos3, fluid: u8, tile: Tile) {
        let units = amount(fluid);
        let below = p.offset(0, 0, -1);
        if !tile.floor()
            && let Some(&under) = self.world.get(below)
            && under.kind != TileKind::Solid
        {
            let lower = self.fluid_at(below);
            let mixed = lower != 0 && is_magma(lower) != is_magma(fluid);
            if mixed || amount(lower) < MAX_AMOUNT {
                let moved = if mixed {
                    units
                } else {
                    units.min(MAX_AMOUNT - amount(lower))
                };
                self.scratch.transfers.push(Transfer {
                    target: below,
                    priority: FALL,
                    source: p,
                    amount: moved,
                    fluid,
                });
                return;
            }
        }
        let rotation = (self.tick as i64 + i64::from(p.x) + i64::from(p.y)).rem_euclid(4) as usize;
        let mut eligible = [(Pos3::default(), 0u8, false); 4];
        let mut count = 0;
        let mut total = u32::from(units);
        for i in 0..4 {
            let (dx, dy) = DIRECTIONS[(rotation + i) % 4];
            let q = p.offset(dx, dy, 0);
            let Some(&neighbor) = self.world.get(q) else {
                continue;
            };
            if neighbor.kind == TileKind::Solid {
                continue;
            }
            let other = self.fluid_at(q);
            if other != 0 && is_magma(other) != is_magma(fluid) {
                eligible[count] = (q, amount(other), true);
                count += 1;
            } else if amount(other) + 2 <= units {
                eligible[count] = (q, amount(other), false);
                count += 1;
                total += u32::from(amount(other));
            }
        }
        if count == 0 {
            return;
        }
        // Reactions take one unit; equal sharing uses the remaining neighbors.
        let sharing = eligible[..count].iter().filter(|e| !e.2).count() as u32;
        let share = total / (1 + sharing);
        let mut remainder = total % (1 + sharing);
        let mut given = 0u8;
        for &(q, other, reacts) in &eligible[..count] {
            if given >= units {
                break;
            }
            let give = if reacts {
                1
            } else {
                let mut target = share;
                if remainder > 0 {
                    target += 1;
                    remainder -= 1;
                }
                target.saturating_sub(u32::from(other)).min(7) as u8
            };
            let give = give.min(units - given);
            if give > 0 {
                given += give;
                self.scratch.transfers.push(Transfer {
                    target: q,
                    priority: SPREAD,
                    source: p,
                    amount: give,
                    fluid,
                });
            }
        }
    }

    pub(crate) fn fluids(&mut self) {
        self.scratch.fluid_changed.clear();
        // Tiles changed earlier in this tick (digging, building) must wake fluids now,
        // exactly as if every chunk were simulated.
        for i in 0..self.world.pending_changes().len() {
            let chunk = self.world.pending_changes()[i];
            self.fluid_active.wake_around(chunk);
        }
        self.sources_and_sinks();
        // Sources act on sleeping chunks too; their chunks simulate in this tick.
        for i in 0..self.scratch.fluid_changed.len() {
            let chunk = self.scratch.fluid_changed[i];
            self.fluid_active.wake_around(chunk);
        }
        let mut active = std::mem::take(&mut self.scratch.fluid_chunks);
        active.clear();
        active.extend_from_slice(self.fluid_active.prepare());
        self.scratch.transfers.clear();
        self.scratch.evaporating.clear();
        let mut may_change = std::mem::take(&mut self.scratch.fluid_may_change);
        may_change.clear();
        let mut processed = 0u64;
        for &chunk in &active {
            let mut cells = std::mem::take(&mut self.scratch.fluid_cells);
            cells.clear();
            self.fluid.for_each_in_chunk(chunk, |p, f| {
                if *f != 0 {
                    cells.push((p, *f));
                }
            });
            processed += cells.len() as u64;
            let mut eligible = false;
            for &(p, fluid) in &cells {
                let Some(&tile) = self.world.get(p) else {
                    continue;
                };
                self.propose(p, fluid, tile);
                if self.evaporates(p, fluid) {
                    eligible = true;
                    let roll = stateless_u64(
                        self.seed,
                        self.tick,
                        self.world.index(p).unwrap_or(0) as u64,
                        0x4556,
                    );
                    if roll.is_multiple_of(self.config.physics.evaporation_chance) {
                        self.scratch.evaporating.push(p);
                    }
                }
            }
            may_change.push(eligible);
            self.scratch.fluid_cells = cells;
        }
        self.diag.fluid_tiles += processed;
        self.diag.fluid_last_tiles = processed;
        self.resolve_transfers();
        for i in 0..self.scratch.evaporating.len() {
            let p = self.scratch.evaporating[i];
            let fluid = self.fluid_at(p);
            if amount(fluid) > 0 && !is_magma(fluid) {
                let gone = amount(fluid).min(EVAPORATION);
                self.set_fluid(p, fluid - gone);
                self.balance.evaporated += u64::from(gone);
                self.balance.total[0] -= u64::from(gone);
            }
        }
        for (index, &chunk) in active.iter().enumerate() {
            let changed = self.scratch.fluid_marks[chunk as usize];
            self.fluid_active.report(chunk, changed, may_change[index]);
        }
        for i in 0..self.scratch.fluid_changed.len() {
            let chunk = self.scratch.fluid_changed[i];
            self.scratch.fluid_marks[chunk as usize] = false;
            if !active.contains(&chunk) {
                self.fluid_active.wake_around(chunk);
            }
        }
        self.scratch.fluid_chunks = active;
        self.scratch.fluid_may_change = may_change;
        self.apply_fluid_tiles();
        let changed = std::mem::take(&mut self.scratch.fluid_changed);
        self.update_heat_sources(&changed);
        self.scratch.fluid_changed = changed;
    }

    fn resolve_transfers(&mut self) {
        let mut transfers = std::mem::take(&mut self.scratch.transfers);
        transfers.sort_unstable_by_key(|t| (t.target, t.priority, t.source));
        let mut start = 0;
        // Free capacity and kind are taken from the phase-start state, before any
        // transfer is applied, so the resolution order does not matter.
        let mut grants = std::mem::take(&mut self.scratch.grants);
        grants.clear();
        while start < transfers.len() {
            let target = transfers[start].target;
            let end = start
                + transfers[start..]
                    .iter()
                    .take_while(|t| t.target == target)
                    .count();
            let existing = self.fluid_at(target);
            let first = transfers[start];
            if existing != 0 && is_magma(existing) != is_magma(first.fluid) {
                grants.push((first, first.amount, true));
            } else {
                let mut free = MAX_AMOUNT - amount(existing);
                let kind_magma = if existing != 0 {
                    is_magma(existing)
                } else {
                    is_magma(first.fluid)
                };
                for t in &transfers[start..end] {
                    if is_magma(t.fluid) != kind_magma || free == 0 {
                        continue;
                    }
                    let granted = t.amount.min(free);
                    free -= granted;
                    grants.push((*t, granted, false));
                }
            }
            start = end;
        }
        // Normal transfers first, reactions last, so a reacting target cannot hand
        // out fluid after it was converted. Amounts never exceed what is left.
        for pass in [false, true] {
            for &(t, granted, reaction) in &grants {
                if reaction != pass {
                    continue;
                }
                let source = self.fluid_at(t.source);
                let actual = granted.min(amount(source));
                let left = amount(source) - actual;
                self.set_fluid(
                    t.source,
                    if left == 0 {
                        0
                    } else {
                        (source & MAGMA) | left
                    },
                );
                self.apply_grant(t, actual, reaction);
            }
        }
        self.scratch.grants = grants;
        transfers.clear();
        self.scratch.transfers = transfers;
    }

    fn apply_grant(&mut self, t: Transfer, actual: u8, reaction: bool) {
        {
            if reaction {
                let target = self.fluid_at(t.target);
                self.balance.converted[kind(t.fluid)] += u64::from(actual);
                self.balance.total[kind(t.fluid)] -= u64::from(actual);
                self.balance.converted[kind(target)] += u64::from(amount(target));
                self.balance.total[kind(target)] -= u64::from(amount(target));
                self.set_fluid(t.target, 0);
                if let Some(obsidian) = self.config.obsidian() {
                    self.scratch
                        .fluid_tiles
                        .push((t.target, Tile::new(TileKind::Solid, false, obsidian)));
                }
            } else {
                if actual > 0 {
                    let target = self.fluid_at(t.target);
                    let value = (t.fluid & MAGMA) | (amount(target) + actual).min(MAX_AMOUNT);
                    self.set_fluid(t.target, value);
                }
            }
        }
    }

    /// Applies threshold crossings and conversions as tile changes in position order.
    fn apply_fluid_tiles(&mut self) {
        for i in 0..self.scratch.fluid_changed.len() {
            let chunk = self.scratch.fluid_changed[i];
            let mut cells = std::mem::take(&mut self.scratch.fluid_cells);
            cells.clear();
            self.fluid
                .for_each_in_chunk(chunk, |p, f| cells.push((p, *f)));
            for &(p, fluid) in &cells {
                if let Some(&tile) = self.world.get(p)
                    && tile.kind != TileKind::Solid
                {
                    let blocked = self.impassable_fluid(fluid) || self.too_hot(p);
                    if tile.impassable() != blocked {
                        self.scratch
                            .fluid_tiles
                            .push((p, tile.with_impassable(blocked)));
                    }
                }
            }
            self.scratch.fluid_cells = cells;
        }
        self.scratch.fluid_tiles.sort_unstable_by_key(|&(p, _)| p);
        self.scratch.fluid_tiles.dedup_by_key(|entry| entry.0);
        for i in 0..self.scratch.fluid_tiles.len() {
            let (p, tile) = self.scratch.fluid_tiles[i];
            if tile.kind == TileKind::Solid {
                self.entomb(p);
            }
            self.world.set(p, tile);
        }
        self.scratch.fluid_tiles.clear();
    }

    /// Items inside a tile that becomes solid are destroyed, residents die.
    pub(crate) fn entomb(&mut self, p: Pos3) {
        for slot in 0..self.icols.len() {
            if self.icols.location[slot] == ItemLocation::Ground(p)
                && let Some(id) = self.items.id(slot as u32)
            {
                self.ops.push(
                    (SOURCE_EVENT, u64::MAX),
                    id.index,
                    EntityCommand::Spawn(Op::DespawnItem(id)),
                );
                self.diag.items_destroyed += 1;
            }
        }
        for slot in 0..self.rcols.len() {
            if self.rcols.position[slot] == p
                && let Some(id) = self.residents.id(slot as u32)
            {
                self.kill(id, Death::Collapse);
            }
        }
    }
}

impl FortressSim {
    /// Removes boiled water, wakes its chunk and updates the impassable marker.
    pub(crate) fn boil(&mut self, p: Pos3, remaining: u8, gone: u8) {
        self.fluid
            .set(p, if amount(remaining) == 0 { 0 } else { remaining });
        self.balance.boiled += u64::from(gone);
        self.balance.total[0] -= u64::from(gone);
        if let Some(chunk) = self.fluid.chunk_of(p) {
            self.fluid_active.wake_around(chunk);
        }
        if let Some(&tile) = self.world.get(p) {
            let blocked = self.impassable_fluid(remaining) || self.too_hot(p);
            if tile.kind != TileKind::Solid && tile.impassable() != blocked {
                self.world.set(p, tile.with_impassable(blocked));
            }
        }
    }
}
