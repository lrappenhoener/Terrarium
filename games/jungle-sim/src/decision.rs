use sim_core::{
    behavior::select_best,
    entity::{EntityId, EntityTable, SparseStorage},
    rng::{below, stateless_u64},
    space::{ChunkedGrid2, Point, SpatialHit, SpatialIndex},
};

use crate::{
    AnimalColumns, JungleConfig, Species,
    world::{Tile, neighbors},
};

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum Action {
    Graze,
    Drink,
    Flee,
    Hunt,
    Follow,
    Roam,
    Rest,
    Mate,
}

impl Action {
    fn from_choice(choice: u32) -> Self {
        match choice {
            0 => Self::Graze,
            1 => Self::Drink,
            2 => Self::Flee,
            3 => Self::Hunt,
            4 => Self::Follow,
            5 => Self::Roam,
            7 => Self::Mate,
            _ => Self::Rest,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Graze => "graze",
            Self::Drink => "drink",
            Self::Flee => "flee",
            Self::Hunt => "hunt",
            Self::Follow => "follow",
            Self::Roam => "roam",
            Self::Rest => "rest",
            Self::Mate => "mate",
        }
    }
}

pub(crate) enum Intent {
    None,
    Move(Point),
    Attack(EntityId, u16),
    Eat((i32, i32), u16),
    Drink,
    Gestate,
}

pub(crate) struct Decision {
    pub action: Action,
    pub target: Option<EntityId>,
    pub herd: Option<EntityId>,
    pub intent: Intent,
}

pub(crate) struct View<'a> {
    pub config: &'a JungleConfig,
    pub world: &'a ChunkedGrid2<Tile>,
    pub occupied: &'a ChunkedGrid2<Option<EntityId>>,
    pub allocator: &'a EntityTable,
    pub columns: &'a AnimalColumns,
    pub hunting: &'a SparseStorage<EntityId>,
    pub pregnant: &'a SparseStorage<u64>,
    pub spatial: &'a SpatialIndex,
    pub population: [usize; 2],
    pub walkable: usize,
    pub tick: u64,
    pub seed: u64,
}

impl View<'_> {
    fn position(&self, id: EntityId) -> Option<Point> {
        self.columns
            .position
            .get(self.allocator.slot(id)? as usize)
            .copied()
    }

    fn move_to(&self, point: Point, random: u64, score: impl Fn(Point, Tile) -> i64) -> Intent {
        neighbors(point)
            .enumerate()
            .filter_map(|(ordinal, next)| {
                let &tile = self.world.get(next)?;
                (tile.walkable() && matches!(self.occupied.get(next), Some(None)))
                    .then(|| (score(next, tile), ((random >> (ordinal * 8)) & 255), next))
            })
            .max()
            .map_or(Intent::None, |(_, _, point)| Intent::Move(point))
    }
}

pub(crate) fn decide(
    view: View<'_>,
    id: EntityId,
    slot: usize,
    out: &mut Vec<SpatialHit>,
    query_counts: &mut [u128; 2],
) -> Option<Decision> {
    let &animal = view.columns.vitals.get(slot)?;
    let &point = view.columns.position.get(slot)?;
    let tile = view.world.get(point)?;
    let params = view.config.species(animal.species);
    let herbivore = animal.species == Species::Herbivore;
    let random = stateless_u64(view.seed, view.tick, id.to_u64(), 0);
    let capacity = if herbivore {
        view.walkable / 20 + 16
    } else {
        view.population[0] / 15 + 16
    };
    let may_mate = view.population[animal.species.index()] < capacity
        && view.pregnant.get(id).is_none()
        && animal.age > 80
        && animal.hunger < params.reproduction_threshold
        && animal.thirst < 80
        && below(random, if herbivore { 72 } else { 36 }) == 0;
    let mut nearest = |category: usize| {
        query_counts[0] += 1;
        query_counts[1] +=
            view.spatial
                .query_radius(point, params.perception_radius, 1 << category, out)
                as u128;
        out.iter()
            .filter(|hit| hit.id != id)
            .map(|hit| {
                (
                    point.distance_squared(view.columns.position[hit.slot as usize]),
                    hit.id,
                )
            })
            .min()
    };
    let current_herd = animal.herd.filter(|&friend| {
        herbivore
            && view.position(friend).is_some_and(|p| {
                point.distance_squared(p) <= u128::from(params.perception_radius).pow(2)
            })
    });
    let closest = nearest(1 - animal.species.index());
    let closest_friend = if (herbivore && current_herd.is_none()) || may_mate {
        nearest(animal.species.index())
    } else {
        None
    };
    let previous = view.hunting.get(id).copied().and_then(|target| {
        let p = view.position(target)?;
        let distance = point.distance_squared(p);
        (distance <= u128::from(params.perception_radius).pow(2)).then_some((distance, target))
    });
    let target = if herbivore {
        closest
    } else {
        previous.into_iter().chain(closest).min()
    };
    let herd = if herbivore {
        current_herd.or_else(|| closest_friend.map(|(_, id)| id))
    } else {
        None
    };
    let hunger = i32::from(animal.hunger);
    let thirst = i32::from(animal.thirst);
    let can_mate = may_mate && closest_friend.is_some();
    let scores = [
        (
            Action::Graze as u32,
            if herbivore && hunger >= 24 {
                hunger + 20
            } else {
                -1
            },
        ),
        (
            Action::Drink as u32,
            if thirst >= 42 { thirst * 3 } else { -1 },
        ),
        (
            Action::Flee as u32,
            if herbivore && target.is_some_and(|(distance, _)| distance <= 9) {
                160
            } else {
                -1
            },
        ),
        (
            Action::Hunt as u32,
            if !herbivore && hunger >= 25 && target.is_some() {
                hunger + 120
            } else {
                -1
            },
        ),
        (
            Action::Follow as u32,
            if herd.is_some() && (random >> 8).is_multiple_of(7) {
                20
            } else {
                -1
            },
        ),
        (Action::Roam as u32, 10),
        (
            Action::Rest as u32,
            if random.is_multiple_of(5) && hunger < 30 && thirst < 20 {
                15
            } else {
                0
            },
        ),
        (Action::Mate as u32, if can_mate { 170 } else { -1 }),
    ];
    let action = Action::from_choice(select_best(&scores)?);
    let ready = animal.move_at <= view.tick;
    let intent = match action {
        Action::Drink if tile.water_distance == 1 => Intent::Drink,
        Action::Drink if ready => {
            view.move_to(point, random, |_, tile| -i64::from(tile.water_distance))
        }
        Action::Flee if ready => target
            .and_then(|(_, id)| view.position(id))
            .map_or(Intent::None, |threat| {
                view.move_to(point, random, |p, _| p.distance_squared(threat) as i64)
            }),
        Action::Hunt => target.map_or(Intent::None, |(distance, target)| {
            if distance <= 1 {
                return Intent::Attack(target, animal.hunger);
            }
            if !ready {
                return Intent::None;
            }
            view.position(target).map_or(Intent::None, |target| {
                view.move_to(point, random, |p, _| -(p.distance_squared(target) as i64))
            })
        }),
        Action::Graze => {
            let richest = std::iter::once(point)
                .chain(neighbors(point))
                .filter_map(|p| {
                    let tile = view.world.get(p)?;
                    tile.walkable()
                        .then_some((tile.biomass, std::cmp::Reverse((p.y, p.x))))
                })
                .max();
            match richest {
                Some((biomass, std::cmp::Reverse(index))) if biomass >= 12 => {
                    Intent::Eat(index, animal.hunger.min(75))
                }
                _ if ready => view.move_to(point, random, |_, tile| i64::from(tile.biomass)),
                _ => Intent::None,
            }
        }
        Action::Follow if ready => herd
            .and_then(|friend| view.position(friend))
            .map_or(Intent::None, |friend| {
                view.move_to(point, random, |p, _| -(p.distance_squared(friend) as i64))
            }),
        Action::Roam if ready => view.move_to(point, random, |_, _| 0),
        Action::Mate => Intent::Gestate,
        _ => Intent::None,
    };
    Some(Decision {
        action,
        target: if herbivore {
            None
        } else {
            target.map(|(_, id)| id)
        },
        herd,
        intent,
    })
}
