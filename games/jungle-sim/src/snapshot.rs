use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sim_core::{
    entity::{AllocatorState, EntityId, EntityTable},
    rng::SystemRng,
    schedule::EventQueue,
    space::ChunkedGrid2,
};
use sim_host::{SimError, StateHasher};

use crate::{AnimalRow, BIOMASS_CAP, Event, JungleConfig, JungleSim, Terrain, Tile, world};

#[derive(Serialize, Deserialize)]
struct Animal {
    id: EntityId,
    // Postcard encodes the nested row exactly like its inlined fields.
    row: AnimalRow,
    target: Option<EntityId>,
    pregnancy: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct State {
    config: String,
    seed: u64,
    tick: u64,
    tiles: Vec<(Terrain, u16)>,
    allocator: AllocatorState,
    animals: Vec<Animal>,
    event_sequence: u64,
    events: Vec<(u64, u64, Event)>,
    rng: (u64, u64, u128),
    growth: u16,
    drought_until: Option<u64>,
    population: [usize; 2],
    births: u64,
    deaths: [u64; 4],
    query_counts: [u128; 2],
}

fn fail(error: impl ToString) -> SimError {
    SimError::Snapshot(error.to_string())
}

impl JungleSim {
    pub(crate) fn encode_snapshot(&self, out: &mut Vec<u8>) -> Result<(), SimError> {
        let animals = self
            .allocator
            .iter_by_id()
            .map(|id| {
                let row = self
                    .allocator
                    .slot(id)
                    .and_then(|slot| self.columns.row(slot))
                    .ok_or_else(|| fail("missing animal row"))?;
                Ok(Animal {
                    id,
                    row,
                    target: self.hunting.get(id).copied(),
                    pregnancy: self.pregnant.get(id).copied(),
                })
            })
            .collect::<Result<_, SimError>>()?;
        let state = State {
            config: serde_json::to_string(&self.config).map_err(fail)?,
            seed: self.seed,
            tick: self.tick,
            tiles: self
                .world
                .iter()
                .map(|tile| (tile.terrain, tile.biomass))
                .collect(),
            allocator: self.allocator.snapshot_state(),
            animals,
            event_sequence: self.events.sequence(),
            events: self
                .events
                .sorted_events()
                .into_iter()
                .map(|(tick, seq, event)| (tick, seq, *event))
                .collect(),
            rng: (self.seed, 1, self.spawn_rng.word_pos()),
            growth: self.growth,
            drought_until: self.drought_until,
            population: self.population,
            births: self.births,
            deaths: self.deaths,
            query_counts: self.query_counts,
        };
        *out = postcard::to_allocvec(&state).map_err(fail)?;
        Ok(())
    }

    pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<Self, SimError> {
        let (state, rest) = postcard::take_from_bytes::<State>(bytes).map_err(fail)?;
        if !rest.is_empty() {
            return Err(fail("trailing snapshot data"));
        }
        let config: JungleConfig = serde_json::from_str(&state.config).map_err(fail)?;
        config.validate()?;
        let size = config.world_size;
        let cap = config.max_animals;
        if state.tiles.len() != (size as usize).pow(2)
            || state.animals.len() > cap as usize
            || state.rng.0 != state.seed
            || state.rng.1 != 1
            || state.rng.2 >= (1u128 << 68)
            || state.growth > BIOMASS_CAP
        {
            return Err(fail("invalid snapshot dimensions, RNG or growth"));
        }
        let mut tiles = ChunkedGrid2::new(
            size,
            size,
            Tile {
                terrain: Terrain::Grass,
                biomass: 0,
                water_distance: u16::MAX,
            },
        )
        .ok_or_else(|| fail("could not reserve world"))?;
        let mut walkable = 0;
        for (index, (terrain, biomass)) in state.tiles.into_iter().enumerate() {
            let tile = tiles
                .get_index_mut(index)
                .ok_or_else(|| fail("invalid tile index"))?;
            tile.terrain = terrain;
            tile.biomass = biomass;
            walkable += usize::from(tile.walkable());
        }
        world::rebuild_water_distances(&mut tiles, size);
        if state
            .animals
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(fail("animals must be ordered by stable ID"));
        }
        let mut sim = Self::empty(state.seed, config, tiles, walkable)?;
        sim.tick = state.tick;
        sim.allocator = EntityTable::restore_state(
            cap,
            state.allocator,
            state.animals.iter().map(|animal| animal.id),
        )
        .ok_or_else(|| fail("invalid allocator state"))?;
        for animal in state.animals {
            if !sim.open(animal.row.position) {
                return Err(fail("invalid or occupied animal position"));
            }
            sim.occupied.set(animal.row.position, Some(animal.id));
            sim.columns.push_row(animal.row);
            if let Some(target) = animal.target {
                sim.hunting.insert(animal.id, target);
            }
            if let Some(deadline) = animal.pregnancy {
                sim.pregnant.insert(animal.id, deadline);
            }
        }
        sim.events = EventQueue::restore(cap as usize + 1, state.event_sequence, state.events)
            .ok_or_else(|| fail("invalid scheduler state"))?;
        sim.spawn_rng = SystemRng::restore(state.rng.0, state.rng.1, state.rng.2);
        sim.growth = state.growth;
        sim.drought_until = state.drought_until;
        sim.population = state.population;
        sim.births = state.births;
        sim.deaths = state.deaths;
        sim.query_counts = state.query_counts;
        sim.rebuild_spatial();
        sim.validate_state()?;
        Ok(sim)
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hash = StateHasher::new();
        hash.write_bytes(b"jungle-content-v1");
        self.config.hash_runtime(&mut hash);
        hash.write_u32(self.config.initial_herbivores);
        hash.write_u32(self.config.initial_predators);
        hash.write_u32(u32::from(self.config.plant_regrowth_per_mille));
        hash.write_u64(self.config.reorder_interval);
        hash.write_bool(self.config.debug_shuffle_slots);
        hash.finish()
    }

    /// One `key<TAB>value` line per logical record, with unique stable keys.
    pub(crate) fn write_debug_dump(&self, out: &mut String) -> Result<(), SimError> {
        out.clear();
        writeln!(out, "meta\ttick={} seed={} growth={} drought={:?} rng_word={} births={} deaths={:?} population={:?} queries={:?} config={:?}",
            self.tick, self.seed, self.growth, self.drought_until, self.spawn_rng.word_pos(), self.births, self.deaths,
            self.population, self.query_counts, self.config).map_err(fail)?;
        writeln!(out, "allocator\t{:?}", self.allocator.snapshot_state()).map_err(fail)?;
        for id in self.allocator.iter_by_id() {
            let Some(slot) = self.allocator.slot(id) else {
                continue;
            };
            write!(out, "animal:{}:{}\t", id.index, id.generation).map_err(fail)?;
            self.columns.dump_row(slot, out);
            writeln!(
                out,
                " target={:?} pregnancy={:?}",
                self.hunting.get(id),
                self.pregnant.get(id)
            )
            .map_err(fail)?;
        }
        let size = self.config.world_size as usize;
        for (index, tile) in self.world.iter().enumerate() {
            if tile.terrain != Terrain::Grass || tile.biomass != BIOMASS_CAP {
                writeln!(
                    out,
                    "tile:{},{}\t{:?} biomass={}",
                    index % size,
                    index / size,
                    tile.terrain,
                    tile.biomass
                )
                .map_err(fail)?;
            }
        }
        writeln!(out, "scheduler\tsequence={}", self.events.sequence()).map_err(fail)?;
        for (tick, seq, event) in self.events.sorted_events() {
            writeln!(out, "event:{seq}\ttick={tick} {event:?}").map_err(fail)?;
        }
        Ok(())
    }
}
