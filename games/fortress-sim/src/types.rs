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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Plant {}
pub type PlantId = TypedId<Plant>;

/// Experience per skill, indexed like the content's skills.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills(pub [u32; crate::content::MAX_SKILLS]);

impl StateHash for Skills {
    fn state_hash(&self, hash: &mut StateHasher) {
        self.0.state_hash(hash);
    }
}

/// A workshop: content kind, footprint corner, construction state and current job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workshop {
    /// Stable, never reused.
    pub id: u32,
    pub kind: u16,
    pub pos: Pos3,
    pub built: bool,
    pub job: Option<JobId>,
}

sim_core::state_hash_struct!(Workshop {
    id,
    kind,
    pos,
    built,
    job
});

/// A designated farm tile with its crop, current plant and pending job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FarmPlot {
    pub pos: Pos3,
    pub crop: u16,
    pub plant: Option<PlantId>,
    pub job: Option<JobId>,
}

sim_core::state_hash_struct!(FarmPlot {
    pos,
    crop,
    plant,
    job
});

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

/// Item kind: a dense index into the content's item definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemKind(pub u16);

impl StateHash for ItemKind {
    fn state_hash(&self, hash: &mut StateHasher) {
        self.0.state_hash(hash);
    }
}

sim_core::state_hash_enum!(DigMode, BuildKind);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemLocation {
    Ground(Pos3),
    Carried(ResidentId),
    /// Inside a container item; the position follows the container.
    Inside(ItemId),
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
            Self::Inside(container) => {
                hash.write_u8(2);
                container.state_hash(hash);
            }
        }
    }
}

/// Units of one item kind that came into and left the world. The live units of a
/// kind always equal `produced - removed`; splitting and merging stacks keep both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub produced: u64,
    pub removed: u64,
}

sim_core::state_hash_struct!(Ledger { produced, removed });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Dig(DigMode),
    Build(BuildKind),
    Haul(ItemId),
    Fell(PlantId),
    /// Sow the farm plot at the job position.
    Sow,
    Harvest(PlantId),
    /// Build a workshop (`recipe` none) or run a recipe in it.
    Craft {
        workshop: u32,
        recipe: Option<u16>,
    },
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
            Self::Fell(plant) => {
                hash.write_u8(3);
                plant.state_hash(hash);
            }
            Self::Sow => hash.write_u8(4),
            Self::Harvest(plant) => {
                hash.write_u8(5);
                plant.state_hash(hash);
            }
            Self::Craft { workshop, recipe } => {
                hash.write_u8(6);
                (workshop, recipe).state_hash(hash);
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
    /// Fell a tree or harvest a crop standing on the plant's tile.
    Gather {
        job: JobId,
        plant: PlantId,
    },
    /// Stage 0 fetches one seed unit from `seed`, stage 1 plants it on `plot`.
    Sow {
        job: JobId,
        seed: ItemId,
        plot: Pos3,
        stage: u8,
    },
    /// Workshop job: stage 0 fetches `item`, stage 1 carries it to the input tile,
    /// stage 2 works, stage 3 plans the next step. All ingredients are reserved.
    Craft {
        job: JobId,
        workshop: u32,
        item: Option<ItemId>,
        stage: u8,
    },
}

impl Task {
    pub(crate) fn job(self) -> Option<JobId> {
        match self {
            Self::Dig { job, .. }
            | Self::Haul { job, .. }
            | Self::Build { job, .. }
            | Self::Gather { job, .. }
            | Self::Sow { job, .. }
            | Self::Craft { job, .. } => Some(job),
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
            (Self::Sow { seed, stage: 0, .. }, ResKey::Item(other)) => seed == other,
            (Self::Craft { .. }, ResKey::Item(_)) => true,
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
            Self::Gather { .. } => "gather",
            Self::Sow { .. } => "sow",
            Self::Craft { .. } => "craft",
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
            Self::Gather { job, plant } => {
                hash.write_u8(7);
                (job, plant).state_hash(hash);
            }
            Self::Sow {
                job,
                seed,
                plot,
                stage,
            } => {
                hash.write_u8(8);
                (job, seed, plot, stage).state_hash(hash);
            }
            Self::Craft {
                job,
                workshop,
                item,
                stage,
            } => {
                hash.write_u8(9);
                (job, workshop, item, stage).state_hash(hash);
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
        quantity: u32,
        quality: u8,
    },
    DespawnItem(ItemId),
    /// Moves all units of `from` onto the compatible stack `into`.
    Merge {
        from: ItemId,
        into: ItemId,
    },
    #[allow(dead_code, reason = "used by production and tests")]
    /// Moves `amount` units of a stack into a new stack on the same tile.
    Split {
        item: ItemId,
        amount: u32,
    },
    DespawnResident(ResidentId),
    SpawnPlant {
        at: Pos3,
        kind: u16,
        growth: u32,
    },
    DespawnPlant(PlantId),
}

/// Player commands. Content is named by string ID; a command resolves its IDs once
/// when it is applied, the simulation keeps only indices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FortressCommand {
    DesignateDig {
        from: Pos3,
        to: Pos3,
        mode: DigMode,
    },
    DesignateStockpile {
        from: Pos3,
        to: Pos3,
    },
    Build {
        pos: Pos3,
        kind: BuildKind,
    },
    CancelDesignations {
        from: Pos3,
        to: Pos3,
    },
    SpawnResidents {
        count: u32,
    },
    /// Farm plots for `crop` on standable floors; underground crops below the
    /// surface, others on it.
    DesignateFarm {
        from: Pos3,
        to: Pos3,
        crop: String,
    },
    /// Fell all trees in the box.
    DesignateFelling {
        from: Pos3,
        to: Pos3,
    },
    /// A workshop of content kind `kind` with its footprint corner at `pos`.
    PlaceWorkshop {
        kind: String,
        pos: Pos3,
    },
}
