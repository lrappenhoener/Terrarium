//! Read-only access for clients. Nothing here mutates the simulation; a test shows
//! that querying the full view every tick leaves every hash unchanged.

use sim_core::{
    jobs::JobState,
    space::{CHUNK_X, CHUNK_Y, CHUNK_Z, ChunkId, Pos3, Tile},
};
use sim_host::Simulation;

use crate::{
    BuildKind, DigMode, FortressSim, ItemKind,
    types::{ItemLocation, JobKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentView {
    pub id: u64,
    pub pos: Pos3,
    pub action: &'static str,
    pub job: Option<u64>,
    pub carried: Option<u64>,
    pub hunger: u16,
    pub fatigue: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemView {
    pub id: u64,
    /// Ground position, or the carrier's position for carried items.
    pub pos: Pos3,
    pub kind: ItemKind,
    pub material: u16,
    pub carried_by: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOverlay {
    Dig(DigMode),
    Build(BuildKind),
    Haul,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobView {
    pub id: u64,
    pub pos: Pos3,
    pub kind: JobOverlay,
    pub claimed_by: Option<u64>,
}

/// Borrowed read-only view of a fortress.
#[derive(Clone, Copy)]
pub struct View<'a> {
    sim: &'a FortressSim,
}

impl FortressSim {
    pub fn view(&self) -> View<'_> {
        View { sim: self }
    }
}

impl View<'_> {
    pub fn tick(&self) -> u64 {
        self.sim.tick
    }

    pub fn dims(&self) -> (i32, i32, i32) {
        (
            self.sim.world.width(),
            self.sim.world.height(),
            self.sim.world.depth(),
        )
    }

    pub const fn chunk_dims() -> (i32, i32, i32) {
        (CHUNK_X, CHUNK_Y, CHUNK_Z)
    }

    pub fn chunk_count(&self) -> usize {
        self.sim.world.chunk_count()
    }

    pub fn chunk_version(&self, chunk: ChunkId) -> u32 {
        self.sim.world.chunk_version(chunk)
    }

    /// Inclusive lower and exclusive upper corner, clipped to the world.
    pub fn chunk_bounds(&self, chunk: ChunkId) -> Option<(Pos3, Pos3)> {
        self.sim.world.chunk_bounds(chunk)
    }

    pub fn chunk_of(&self, p: Pos3) -> Option<ChunkId> {
        self.sim.world.chunk_of(p)
    }

    /// Visits the tiles of a chunk in level, row, column order.
    pub fn chunk_tiles(&self, chunk: ChunkId, visit: impl FnMut(Pos3, &Tile)) {
        self.sim.world.for_each_in_chunk(chunk, visit);
    }

    pub fn tile(&self, p: Pos3) -> Option<Tile> {
        self.sim.world.get(p).copied()
    }

    pub fn material_name(&self, material: u16) -> Option<&str> {
        self.sim
            .config
            .materials
            .get(usize::from(material))
            .map(|m| m.name.as_str())
    }

    pub fn is_ore(&self, material: u16) -> bool {
        self.sim
            .config
            .materials
            .get(usize::from(material))
            .is_some_and(|m| m.ore)
    }

    pub fn material_count(&self) -> usize {
        self.sim.config.materials.len()
    }

    /// Residents in ascending ID order.
    pub fn residents(&self, out: &mut Vec<ResidentView>) {
        out.clear();
        let sim = self.sim;
        for id in sim.residents.iter_by_id() {
            let Some(slot) = sim.rslot(id) else { continue };
            let task = sim.rcols.task[slot];
            out.push(ResidentView {
                id: id.to_u64(),
                pos: sim.rcols.position[slot],
                action: task.name(),
                job: task.job(),
                carried: sim.rcols.carried[slot].map(|item| item.to_u64()),
                hunger: sim.rcols.needs[slot].hunger,
                fatigue: sim.rcols.needs[slot].fatigue,
            });
        }
    }

    /// Items in ascending ID order.
    pub fn items(&self, out: &mut Vec<ItemView>) {
        out.clear();
        let sim = self.sim;
        for id in sim.items.iter_by_id() {
            let Some(slot) = sim.islot(id) else { continue };
            let (pos, carried_by) = match sim.icols.location[slot] {
                ItemLocation::Ground(p) => (p, None),
                ItemLocation::Carried(holder) => match sim.rslot(holder) {
                    Some(rslot) => (sim.rcols.position[rslot], Some(holder.to_u64())),
                    None => continue,
                },
            };
            out.push(ItemView {
                id: id.to_u64(),
                pos,
                kind: sim.icols.kind[slot],
                material: sim.icols.material[slot],
                carried_by,
            });
        }
    }

    /// Open and claimed jobs in ascending ID order.
    pub fn jobs(&self, out: &mut Vec<JobView>) {
        out.clear();
        for job in self.sim.jobs.iter() {
            let claimed_by = match job.state {
                JobState::Open => None,
                JobState::Claimed { by, .. } => Some(by.to_u64()),
                JobState::Done => continue,
            };
            out.push(JobView {
                id: job.id,
                pos: job.pos,
                kind: match job.data {
                    JobKind::Dig(mode) => JobOverlay::Dig(mode),
                    JobKind::Build(kind) => JobOverlay::Build(kind),
                    JobKind::Haul(_) => JobOverlay::Haul,
                },
                claimed_by,
            });
        }
    }

    pub fn stockpiles(&self) -> &[Pos3] {
        &self.sim.stockpiles
    }

    /// Remaining path of a resident, starting at its next node.
    pub fn path(&self, resident: u64, out: &mut Vec<Pos3>) {
        out.clear();
        let id = crate::types::ResidentId::from_u64(resident);
        let Some(slot) = self.sim.rslot(id) else {
            return;
        };
        let next = usize::from(self.sim.rcols.route[slot].next);
        out.extend(self.sim.route_nodes(id).skip(next));
    }

    /// Region label for debug overlays only. Clients must not derive decisions from it.
    pub fn debug_region(&self, p: Pos3) -> Option<u32> {
        self.sim.connectivity.debug_region(p)
    }

    pub fn describe(&self, id: u64) -> Option<Vec<(String, String)>> {
        self.sim.describe_entity(id)
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let [open, claimed, _] = self.sim.jobs.counts();
        (
            self.sim.residents.len(),
            self.sim.items.len(),
            open + claimed,
        )
    }
}
