//! Fortress entity, job and task types with their canonical hash encodings.

use serde::{Deserialize, Serialize};
use sim_core::{
    entity::TypedId,
    jobs::JobId,
    space::Pos3,
    state::{StateHash, StateHasher},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resident {}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {}

pub type ResidentId = TypedId<Resident>;
pub type ItemId = TypedId<Item>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DigMode {
    Dig,
    StairUpDown,
    StairDown,
    StairUp,
    Ramp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BuildKind {
    Wall,
    Floor,
    StairUpDown,
    Door,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ItemKind {
    Stone,
    Ore,
    Food,
}

sim_core::state_hash_enum!(DigMode, BuildKind, ItemKind);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemLocation {
    Ground(Pos3),
    Carried(ResidentId),
}

impl StateHash for ItemLocation {
    fn state_hash(&self, hash: &mut StateHasher) {
        match self {
            Self::Ground(p) => {
                hash.write_u8(0);
                p.state_hash(hash);
            }
            Self::Carried(id) => {
                hash.write_u8(1);
                id.state_hash(hash);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Dig(DigMode),
    Build(BuildKind),
    Haul(ItemId),
}

impl StateHash for JobKind {
    fn state_hash(&self, hash: &mut StateHasher) {
        match self {
            Self::Dig(mode) => {
                hash.write_u8(0);
                mode.state_hash(hash);
            }
            Self::Build(kind) => {
                hash.write_u8(1);
                kind.state_hash(hash);
            }
            Self::Haul(item) => {
                hash.write_u8(2);
                item.state_hash(hash);
            }
        }
    }
}

/// Reserved objects. Keys order by kind, then by stable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ResKey {
    Item(ItemId),
    Slot(Pos3),
}

impl StateHash for ResKey {
    fn state_hash(&self, hash: &mut StateHasher) {
        match self {
            Self::Item(id) => {
                hash.write_u8(0);
                id.state_hash(hash);
            }
            Self::Slot(p) => {
                hash.write_u8(1);
                p.state_hash(hash);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vitals {
    pub health: u16,
    /// Consecutive ticks in deep water.
    pub submerged: u16,
}

sim_core::state_hash_struct!(Vitals { health, submerged });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    pub hunger: u16,
    pub fatigue: u16,
}

sim_core::state_hash_struct!(Needs { hunger, fatigue });

/// What a resident is doing. `stage` 0 travels to the first target, later stages
/// follow the task: haul and build pick up at stage 0 and deliver at stage 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Task {
    Idle,
    Sleep,
    Eat {
        item: ItemId,
    },
    Dig {
        job: JobId,
        site: Pos3,
    },
    Haul {
        job: JobId,
        item: ItemId,
        slot: Pos3,
        stage: u8,
    },
    Build {
        job: JobId,
        item: ItemId,
        site: Pos3,
        stage: u8,
    },
    /// Escape from water or heat to a safe tile.
    Flee {
        target: Pos3,
    },
}

impl Task {
    pub(crate) fn job(self) -> Option<JobId> {
        match self {
            Self::Dig { job, .. } | Self::Haul { job, .. } | Self::Build { job, .. } => Some(job),
            _ => None,
        }
    }

    pub(crate) fn holds(self, key: ResKey) -> bool {
        match (self, key) {
            (Self::Eat { item }, ResKey::Item(other)) => item == other,
            (Self::Haul { item, slot, .. }, key) => {
                key == ResKey::Item(item) || key == ResKey::Slot(slot)
            }
            (Self::Build { item, .. }, ResKey::Item(other)) => item == other,
            _ => false,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Sleep => "sleep",
            Self::Eat { .. } => "eat",
            Self::Dig { .. } => "dig",
            Self::Haul { .. } => "haul",
            Self::Build { .. } => "build",
            Self::Flee { .. } => "flee",
        }
    }
}

impl StateHash for Task {
    fn state_hash(&self, hash: &mut StateHasher) {
        match *self {
            Self::Idle => hash.write_u8(0),
            Self::Sleep => hash.write_u8(1),
            Self::Eat { item } => {
                hash.write_u8(2);
                item.state_hash(hash);
            }
            Self::Dig { job, site } => {
                hash.write_u8(3);
                job.state_hash(hash);
                site.state_hash(hash);
            }
            Self::Haul {
                job,
                item,
                slot,
                stage,
            } => {
                hash.write_u8(4);
                (job, item, slot, stage).state_hash(hash);
            }
            Self::Build {
                job,
                item,
                site,
                stage,
            } => {
                hash.write_u8(5);
                (job, item, site, stage).state_hash(hash);
            }
            Self::Flee { target } => {
                hash.write_u8(6);
                target.state_hash(hash);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum RouteStatus {
    None,
    Waiting,
    Moving,
}

sim_core::state_hash_enum!(RouteStatus);

/// Movement along the stored path: `next` indexes the next node, `progress`
/// accumulates speed until the step cost is paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub status: RouteStatus,
    pub len: u16,
    pub next: u16,
    pub progress: u32,
    pub failures: u8,
}

impl Route {
    pub const NONE: Self = Self {
        status: RouteStatus::None,
        len: 0,
        next: 0,
        progress: 0,
        failures: 0,
    };
}

sim_core::state_hash_struct!(Route {
    status,
    len,
    next,
    progress,
    failures
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Caravan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Death {
    Hunger,
    Drowned,
    Heat,
    Collapse,
}

/// Deferred entity operations, applied at the end of the tick in source order.
#[derive(Debug, Clone, Copy)]
pub enum Op {
    SpawnResident,
    SpawnItem {
        at: Pos3,
        kind: ItemKind,
        material: u16,
    },
    DespawnItem(ItemId),
    DespawnResident(ResidentId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FortressCommand {
    DesignateDig { from: Pos3, to: Pos3, mode: DigMode },
    DesignateStockpile { from: Pos3, to: Pos3 },
    Build { pos: Pos3, kind: BuildKind },
    CancelDesignations { from: Pos3, to: Pos3 },
    SpawnResidents { count: u32 },
}
