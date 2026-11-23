#![deny(clippy::float_arithmetic)]

//! Integer predator-prey simulation in a seeded jungle.

mod config;
mod decision;
mod diagnostics;
mod world;

use std::cmp::Reverse;

pub use config::{JungleConfig, Species, SpeciesConfig};
use serde::{Deserialize, Serialize};
use sim_core::{
    entity::{EntityCommand, EntityCommandBuffer, EntityId, EntityTable, SparseStorage},
    intent::IntentBuffer,
    rng::{SystemRng, below, stateless_u64},
    schedule::EventQueue,
    space::{Grid2, Point, SpatialHit, SpatialIndex},
};
use sim_host::{NoopProbe, PhaseProbe, SimError, SimMetrics, Simulation};

use decision::{Action, Decision, Intent};
use world::{BIOMASS_CAP, Terrain, Tile};

const NEED_LIMIT: u16 = 1000;
const HEALTH: u16 = 100;
const EXTERNAL: EntityId = EntityId {
    index: u32::MAX,
    generation: u32::MAX,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JungleCommand {
    SpawnAnimals { species: Species, count: u32 },
    SetPlantGrowth { per_mille: u16 },
    Drought { duration_ticks: u64 },
}

#[derive(Clone, Copy)]
struct Vitals {
    species: Species,
    hunger: u16,
    thirst: u16,
    health: u16,
    age: u64,
    move_at: u64,
    action: Action,
    herd: Option<EntityId>,
}

#[derive(Clone, Copy)]
enum Event {
    Birth(EntityId),
    DroughtEnd,
}
enum Spawn {
    External(Species),
    Litter {
        parent: EntityId,
        species: Species,
        at: Point,
    },
}

#[derive(Clone, Copy)]
enum Effect {
    Move(EntityId, Point),
    Attack(EntityId, EntityId),
    Eat(EntityId, usize, u16),
    Drink(EntityId),
    Gestate(EntityId),
}

#[derive(Clone, Copy)]
enum Death {
    Predation,
    Hunger,
    Thirst,
    Age,
}

sim_core::entity_columns! {
    struct AnimalColumns { position: Point, vitals: Vitals }
}

pub struct JungleSim {
    config: JungleConfig,
    seed: u64,
    tick: u64,
    world: Grid2<Tile>,
    walkable: usize,
    occupied: Grid2<Option<EntityId>>,
    allocator: EntityTable,
    columns: AnimalColumns,
    hunting: SparseStorage<EntityId>,
    pregnant: SparseStorage<u64>,
    spatial: SpatialIndex,
    locations: Vec<(SpatialHit, Point)>,
    nearby: Vec<SpatialHit>,
    decisions: Vec<(EntityId, Decision)>,
    moves: IntentBuffer<Point, ()>,
    attacks: IntentBuffer<EntityId, u16>,
    grazing: IntentBuffer<usize, u16>,
    effects: Vec<Effect>,
    events: EventQueue<Event>,
    commands: EntityCommandBuffer<Spawn>,
    spawn_rng: SystemRng,
    growth: u16,
    drought_until: Option<u64>,
    population: [usize; 2],
    births: u64,
    deaths: [u64; 4],
    query_counts: [u128; 2],
}

impl JungleSim {
    fn position(&self, id: EntityId) -> Option<&Point> {
        self.columns.position.get(self.allocator.slot(id)? as usize)
    }

    fn vitals(&self, id: EntityId) -> Option<&Vitals> {
        self.columns.vitals.get(self.allocator.slot(id)? as usize)
    }

    fn commands(&mut self, commands: &[JungleCommand]) {
        let room = self.config.max_animals as usize - self.allocator.len();
        let mut accepted = 0;
        for command in commands {
            match *command {
                JungleCommand::SpawnAnimals { species, count } => {
                    let count = (count as usize).min(room - accepted);
                    for ordinal in accepted..accepted + count {
                        self.commands.push(
                            EXTERNAL,
                            ordinal as u32,
                            EntityCommand::Spawn(Spawn::External(species)),
                        );
                    }
                    accepted += count;
                }
                JungleCommand::SetPlantGrowth { per_mille } => self.growth = per_mille.min(1000),
                JungleCommand::Drought { duration_ticks } => {
                    self.events
                        .retain(|event| !matches!(event, Event::DroughtEnd));
                    self.drought_until =
                        (duration_ticks != 0).then(|| self.tick.saturating_add(duration_ticks));
                    if let Some(end) = self.drought_until {
                        self.events.push(end, Event::DroughtEnd);
                    }
                }
            }
        }
    }

    fn rebuild_spatial(&mut self) {
        self.locations.clear();
        self.locations.extend(
            self.allocator
                .iter()
                .zip(self.columns.position.iter().copied())
                .enumerate()
                .map(|(slot, (id, point))| {
                    (
                        SpatialHit {
                            id,
                            slot: slot as u32,
                        },
                        point,
                    )
                }),
        );
        self.spatial.rebuild(self.locations.iter().copied());
    }

    fn open(&self, point: Point) -> bool {
        self.world.get(point).is_some_and(|tile| tile.walkable())
            && self.occupied.get(point).is_some_and(Option::is_none)
    }

    fn spawn_external(&mut self, species: Species, initial: bool) -> bool {
        if self.allocator.len() >= self.walkable
            || self.allocator.len() >= self.config.max_animals as usize
        {
            return false;
        }
        let len = self.world.as_slice().len();
        let size = self.config.world_size;
        let start = below(self.spawn_rng.next_u64(), len as u64) as usize;
        for offset in 0..len {
            let index = ((start + offset) % len) as i32;
            let point = Point {
                x: index % size,
                y: index / size,
            };
            if self.open(point) {
                return self.spawn_at(species, point, initial);
            }
        }
        false
    }

    fn spawn_at(&mut self, species: Species, point: Point, initial: bool) -> bool {
        if !self.open(point) {
            return false;
        }
        let Some(id) = self.allocator.create() else {
            return false;
        };
        let random = |salt| stateless_u64(self.seed, self.tick, id.to_u64(), salt);
        let age = if initial {
            below(random(1), self.config.species(species).max_age / 4)
        } else {
            0
        };
        self.columns.push(
            point,
            Vitals {
                species,
                hunger: below(random(2), 50) as u16,
                thirst: below(random(3), 30) as u16,
                health: HEALTH,
                age,
                move_at: self.tick,
                herd: None,
                action: Action::Roam,
            },
        );
        self.occupied.set(point, Some(id));
        self.population[species.index()] += 1;
        true
    }

    fn despawn(&mut self, id: EntityId) {
        if let Some(slot) = self.allocator.destroy(id) {
            if let Some((point, vitals)) = self.columns.swap_remove(slot) {
                self.population[vitals.species.index()] -= 1;
                self.occupied.set(point, None);
            }
            self.hunting.remove(id);
            self.pregnant.remove(id);
        }
    }

    fn decide(&mut self) {
        self.decisions.clear();
        for (slot, id) in self.allocator.iter().enumerate() {
            if sim_core::schedule::staggered(
                self.tick,
                self.config.think_interval,
                u64::from(id.index),
            ) && let Some(decision) = decision::decide(
                decision::View {
                    config: &self.config,
                    world: &self.world,
                    occupied: &self.occupied,
                    allocator: &self.allocator,
                    columns: &self.columns,
                    hunting: &self.hunting,
                    pregnant: &self.pregnant,
                    spatial: &self.spatial,
                    population: self.population,
                    walkable: self.walkable,
                    tick: self.tick,
                    seed: self.seed,
                },
                id,
                slot,
                &mut self.nearby,
                &mut self.query_counts,
            ) {
                self.decisions.push((id, decision));
            }
        }
    }

    fn resolve(&mut self) {
        self.moves.clear();
        self.attacks.clear();
        self.grazing.clear();
        self.effects.clear();
        for &(id, ref decision) in &self.decisions {
            match decision.intent {
                Intent::Move(point) => self.moves.push(point, id, ()),
                Intent::Attack(target, hunger) => self.attacks.push(target, id, hunger),
                Intent::Eat(tile, amount) => self.grazing.push(tile, id, amount),
                Intent::Drink => self.effects.push(Effect::Drink(id)),
                Intent::Gestate => self.effects.push(Effect::Gestate(id)),
                Intent::None => {}
            }
        }
        self.effects.sort_unstable_by_key(|effect| match *effect {
            Effect::Drink(id) | Effect::Gestate(id) => id,
            _ => EXTERNAL,
        });
        self.moves.resolve(
            |_| (),
            |&(point, id, ()), _| self.effects.push(Effect::Move(id, point)),
        );
        self.attacks.resolve(
            |&hunger| Reverse(hunger),
            |&(target, id, _), _| self.effects.push(Effect::Attack(id, target)),
        );
        self.grazing.resolve(
            |_| (),
            |winner, losers| {
                let mut available = self.world.as_slice()[winner.0].biomass;
                for &(tile, id, amount) in std::iter::once(winner).chain(losers) {
                    let taken = amount.min(available);
                    available -= taken;
                    self.effects.push(Effect::Eat(id, tile, taken));
                }
            },
        );
    }

    fn apply_effects(&mut self) {
        for &(id, ref decision) in &self.decisions {
            if let Some(animal) = self
                .allocator
                .slot(id)
                .and_then(|slot| self.columns.vitals.get_mut(slot as usize))
            {
                animal.action = decision.action;
                animal.herd = decision.herd;
            }
            if let Some(target) = decision.target {
                self.hunting.insert(id, target);
            } else {
                self.hunting.remove(id);
            }
        }
        for animal in &mut self.columns.vitals {
            let params = self.config.species(animal.species);
            animal.hunger = animal
                .hunger
                .saturating_add(params.hunger_rate)
                .min(NEED_LIMIT);
            animal.thirst = animal
                .thirst
                .saturating_add(params.thirst_rate)
                .min(NEED_LIMIT);
            animal.age = animal.age.saturating_add(1);
        }
        for index in 0..self.effects.len() {
            match self.effects[index] {
                Effect::Move(id, point) => {
                    if let (Some(slot), Some(tile)) =
                        (self.allocator.slot(id), self.world.get(point))
                    {
                        let old = &mut self.columns.position[slot as usize];
                        let animal = &mut self.columns.vitals[slot as usize];
                        self.occupied.set(*old, None);
                        self.occupied.set(point, Some(id));
                        *old = point;
                        let cost = 1 + u64::from(tile.terrain == Terrain::Jungle);
                        animal.move_at = self.tick.saturating_add(
                            self.config
                                .species(animal.species)
                                .move_cooldown
                                .saturating_mul(cost),
                        );
                    }
                }
                Effect::Attack(id, target) => {
                    let damage = self.config.predator.attack_strength;
                    if let Some(prey) = self
                        .allocator
                        .slot(target)
                        .and_then(|slot| self.columns.vitals.get_mut(slot as usize))
                        .filter(|vitals| vitals.health != 0)
                    {
                        prey.health = prey.health.saturating_sub(damage);
                        if prey.health == 0
                            && let Some(hunter) = self
                                .allocator
                                .slot(id)
                                .and_then(|slot| self.columns.vitals.get_mut(slot as usize))
                        {
                            hunter.hunger = 0;
                        }
                    }
                }
                Effect::Eat(id, tile, taken) => {
                    self.world.as_mut_slice()[tile].biomass -= taken;
                    if let Some(animal) = self
                        .allocator
                        .slot(id)
                        .and_then(|slot| self.columns.vitals.get_mut(slot as usize))
                    {
                        animal.hunger = animal.hunger.saturating_sub(taken);
                    }
                }
                Effect::Drink(id) => {
                    if let Some(animal) = self
                        .allocator
                        .slot(id)
                        .and_then(|slot| self.columns.vitals.get_mut(slot as usize))
                    {
                        animal.thirst = 0;
                    }
                }
                Effect::Gestate(id) => {
                    if let Some(animal) = self.vitals(id) {
                        let deadline = self
                            .tick
                            .saturating_add(self.config.species(animal.species).gestation_ticks);
                        if self.events.len() < self.config.max_animals as usize {
                            self.pregnant.insert(id, deadline);
                            self.events.push(deadline, Event::Birth(id));
                        }
                    }
                }
            }
        }
    }

    fn environment(&mut self) {
        let stripes = self.config.plant_update_stripes;
        let stripe = (self.tick % stripes as u64) as usize;
        let Some(tiles) = self.world.as_mut_slice().get_mut(stripe..) else {
            return;
        };
        let drought = self.drought_until.is_some_and(|end| self.tick < end);
        for tile in tiles.iter_mut().step_by(stripes) {
            if tile.walkable() {
                tile.biomass = if drought {
                    tile.biomass.saturating_sub(12)
                } else {
                    tile.biomass.saturating_add(self.growth).min(BIOMASS_CAP)
                };
            }
        }
    }

    fn lifecycle(&mut self) {
        for id in self.allocator.iter() {
            if let Some(animal) = self.vitals(id) {
                let cause = if animal.health == 0 {
                    Some(Death::Predation)
                } else if animal.hunger >= NEED_LIMIT {
                    Some(Death::Hunger)
                } else if animal.thirst >= NEED_LIMIT {
                    Some(Death::Thirst)
                } else if animal.age >= self.config.species(animal.species).max_age {
                    Some(Death::Age)
                } else {
                    None
                };
                if let Some(cause) = cause {
                    self.deaths[cause as usize] += 1;
                    self.commands.push(id, 0, EntityCommand::Despawn(id));
                }
            }
        }
        while let Some(event) = self.events.pop_due(self.tick) {
            match event {
                Event::Birth(id) => {
                    self.pregnant.remove(id);
                    if let (Some(vitals), Some(&at)) = (self.vitals(id), self.position(id)) {
                        self.commands.push(
                            id,
                            1,
                            EntityCommand::Spawn(Spawn::Litter {
                                parent: id,
                                species: vitals.species,
                                at,
                            }),
                        );
                    }
                }
                Event::DroughtEnd => self.drought_until = None,
            }
        }
        // Move the buffer out temporarily so its callback can update all simulation stores.
        let mut commands =
            std::mem::replace(&mut self.commands, EntityCommandBuffer::with_capacity(0));
        commands.apply(|command| match command {
            EntityCommand::Despawn(id) => self.despawn(id),
            EntityCommand::Spawn(Spawn::External(species)) => {
                self.spawn_external(species, false);
            }
            EntityCommand::Spawn(Spawn::Litter {
                parent,
                species,
                at,
            }) if self.allocator.is_alive(parent) => {
                let mut remaining = self.config.species(species).litter_size;
                let rotate =
                    below(stateless_u64(self.seed, self.tick, parent.to_u64(), 17), 25) as i32;
                for offset in 0..25 {
                    let cell = (offset + rotate) % 25;
                    let point = Point {
                        x: at.x + cell % 5 - 2,
                        y: at.y + cell / 5 - 2,
                    };
                    if self.spawn_at(species, point, false) {
                        self.births += 1;
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
            }
            EntityCommand::Spawn(Spawn::Litter { .. }) => {}
        });
        self.commands = commands;
    }
}

impl Simulation for JungleSim {
    type Config = JungleConfig;
    type Command = JungleCommand;

    fn new(seed: u64, config: JungleConfig) -> Result<Self, SimError> {
        config.validate()?;
        let cap = config.max_animals;
        let n = cap as usize;
        let size = config.world_size;
        let (world, walkable) = world::generate(seed, size)?;
        if config.initial_herbivores as usize + config.initial_predators as usize > walkable {
            return Err(SimError::InvalidConfig(
                "initial population exceeds walkable tiles".into(),
            ));
        }
        let occupied = Grid2::new(size, size, None)
            .ok_or_else(|| SimError::InvalidConfig("could not reserve occupancy".into()))?;
        let spatial = SpatialIndex::new(size, size, 8, n)
            .ok_or_else(|| SimError::InvalidConfig("could not reserve spatial index".into()))?;
        let mut sim = Self {
            growth: config.plant_regrowth_per_mille,
            config,
            seed,
            tick: 0,
            world,
            walkable,
            occupied,
            spatial,
            allocator: EntityTable::with_capacity(cap),
            columns: AnimalColumns::with_capacity(n),
            hunting: SparseStorage::with_capacity(cap),
            pregnant: SparseStorage::with_capacity(cap),
            locations: Vec::with_capacity(n),
            nearby: Vec::with_capacity(n),
            decisions: Vec::with_capacity(n),
            moves: IntentBuffer::with_capacity(n),
            attacks: IntentBuffer::with_capacity(n),
            grazing: IntentBuffer::with_capacity(n),
            effects: Vec::with_capacity(n),
            events: EventQueue::with_capacity(n + 1),
            commands: EntityCommandBuffer::with_capacity(2 * n),
            spawn_rng: SystemRng::new(seed, 1),
            drought_until: None,
            population: [0; 2],
            births: 0,
            deaths: [0; 4],
            query_counts: [0; 2],
        };
        for _ in 0..sim.config.initial_herbivores {
            sim.spawn_external(Species::Herbivore, true);
        }
        for _ in 0..sim.config.initial_predators {
            sim.spawn_external(Species::Predator, true);
        }
        sim.rebuild_spatial();
        Ok(sim)
    }

    fn step(&mut self, commands: &[JungleCommand]) {
        self.step_profiled(commands, &mut NoopProbe);
    }

    fn step_profiled(&mut self, commands: &[JungleCommand], probe: &mut dyn PhaseProbe) {
        fn phase(probe: &mut dyn PhaseProbe, name: &'static str, work: impl FnOnce()) {
            probe.enter(name);
            work();
            probe.exit(name);
        }
        phase(probe, "commands", || self.commands(commands));
        phase(probe, "spatial", || self.rebuild_spatial());
        phase(probe, "decide", || self.decide());
        phase(probe, "resolve", || self.resolve());
        phase(probe, "apply", || self.apply_effects());
        phase(probe, "environment", || self.environment());
        phase(probe, "lifecycle", || {
            self.lifecycle();
            self.rebuild_spatial();
        });
        self.tick += 1;
    }

    fn tick(&self) -> u64 {
        self.tick
    }
    fn is_finished(&self) -> bool {
        self.allocator.is_empty()
    }
    fn phase_dimensions(&self) -> Vec<(&'static str, &'static str)> {
        vec![("environment", "tiles")]
    }

    fn scale_units(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("entities", self.allocator.len() as u64),
            ("tiles", self.world.as_slice().len() as u64),
        ]
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
    fn describe_entity(&self, id: u64) -> Option<Vec<(String, String)>> {
        self.describe(EntityId::from_u64(id))
    }
    fn check_invariants(&self) -> Result<(), SimError> {
        self.validate_state()
    }
}

#[cfg(test)]
mod tests;
