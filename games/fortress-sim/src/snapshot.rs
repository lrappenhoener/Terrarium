//! Snapshot payload version 1. The world is stored as the tiles that differ from
//! the world regenerated from seed and configuration. Connectivity, path caches,
//! job indices, slot layout and scratch buffers are rebuilt on load.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sim_core::{
    entity::{AllocatorState, EntityTable},
    jobs::{Job, JobBoard, Reservation, Reservations},
    path::{PathRequest, PathRequestQueue},
    schedule::EventQueue,
    space::{Pos3, Tile},
};
use sim_host::{SimError, StateHasher};

use crate::{
    Counters, FortressConfig, FortressSim, ItemRow, ResidentRow,
    types::{Event, ItemId, JobKind, ResKey, ResidentId},
    world,
};

#[derive(Serialize, Deserialize)]
struct ResidentRecord {
    id: ResidentId,
    row: ResidentRow,
    path: Vec<Pos3>,
}

#[derive(Serialize, Deserialize)]
struct ItemRecord {
    id: ItemId,
    row: ItemRow,
}

#[derive(Serialize, Deserialize)]
struct State {
    config: String,
    seed: u64,
    tick: u64,
    tiles: Vec<(u32, Tile)>,
    stockpiles: Vec<Pos3>,
    resident_ids: AllocatorState,
    residents: Vec<ResidentRecord>,
    item_ids: AllocatorState,
    items: Vec<ItemRecord>,
    next_job: u64,
    jobs: Vec<Job<JobKind, ResidentId>>,
    reservations: Vec<Reservation<ResKey, ResidentId>>,
    path_requests: Vec<PathRequest<ResidentId, Pos3>>,
    event_sequence: u64,
    events: Vec<(u64, u64, Event)>,
    counters: Counters,
}

fn fail(error: impl ToString) -> SimError {
    SimError::Snapshot(error.to_string())
}

impl FortressSim {
    /// Logical indices and tiles that differ from the generated world.
    fn tile_changes(&self) -> Result<Vec<(u32, Tile)>, SimError> {
        let generated = world::generate(self.seed, &self.config)?;
        Ok(self
            .world
            .iter_logical()
            .zip(generated.iter_logical())
            .enumerate()
            .filter(|(_, ((_, now), (_, then)))| now != then)
            .map(|(index, ((_, tile), _))| (index as u32, *tile))
            .collect())
    }

    pub(crate) fn encode_snapshot(&self, out: &mut Vec<u8>) -> Result<(), SimError> {
        let residents = self
            .residents
            .iter_by_id()
            .map(|id| {
                let row = self
                    .residents
                    .slot(id)
                    .and_then(|slot| self.rcols.row(slot))
                    .ok_or_else(|| fail("missing resident row"))?;
                Ok(ResidentRecord {
                    id,
                    row,
                    path: self.route_nodes(id).collect(),
                })
            })
            .collect::<Result<_, SimError>>()?;
        let items = self
            .items
            .iter_by_id()
            .map(|id| {
                let row = self
                    .items
                    .slot(id)
                    .and_then(|slot| self.icols.row(slot))
                    .ok_or_else(|| fail("missing item row"))?;
                Ok(ItemRecord { id, row })
            })
            .collect::<Result<_, SimError>>()?;
        let state = State {
            config: serde_json::to_string(&self.config).map_err(fail)?,
            seed: self.seed,
            tick: self.tick,
            tiles: self.tile_changes()?,
            stockpiles: self.stockpiles.clone(),
            resident_ids: self.residents.snapshot_state(),
            residents,
            item_ids: self.items.snapshot_state(),
            items,
            next_job: self.jobs.next_id(),
            jobs: self.jobs.iter().cloned().collect(),
            reservations: self.reservations.iter().copied().collect(),
            path_requests: self.queue.snapshot(),
            event_sequence: self.events.sequence(),
            events: self
                .events
                .sorted_events()
                .into_iter()
                .map(|(tick, seq, event)| (tick, seq, *event))
                .collect(),
            counters: self.counters,
        };
        *out = postcard::to_allocvec(&state).map_err(fail)?;
        Ok(())
    }

    pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<Self, SimError> {
        let (state, rest) = postcard::take_from_bytes::<State>(bytes).map_err(fail)?;
        if !rest.is_empty() {
            return Err(fail("trailing snapshot data"));
        }
        let config: FortressConfig = serde_json::from_str(&state.config).map_err(fail)?;
        config.validate()?;
        let mut world = world::generate(state.seed, &config)?;
        if state.tiles.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(fail("tile changes must ascend by index"));
        }
        for (index, tile) in state.tiles {
            let p = world
                .pos(index as usize)
                .ok_or_else(|| fail("tile change outside the world"))?;
            world.set(p, tile);
        }
        world.clear_changes();
        let mut sim = Self::empty(state.seed, config, world);
        sim.tick = state.tick;
        let cap = sim.config.max_path_nodes as usize;
        if state.stockpiles.windows(2).any(|pair| pair[0] >= pair[1])
            || state.stockpiles.len() > crate::STOCKPILE_CAPACITY
        {
            return Err(fail("invalid stockpiles"));
        }
        sim.stockpiles.extend(state.stockpiles);
        if state
            .residents
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
            || state.items.windows(2).any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(fail("entities must be ordered by stable ID"));
        }
        sim.residents = EntityTable::restore_state(
            sim.config.max_residents,
            state.resident_ids,
            state.residents.iter().map(|r| r.id),
        )
        .ok_or_else(|| fail("invalid resident IDs"))?;
        for record in state.residents {
            if record.path.len() > cap || record.path.len() != usize::from(record.row.route.len) {
                return Err(fail("invalid stored path"));
            }
            let base = record.id.index as usize * cap;
            for (i, p) in record.path.iter().enumerate() {
                sim.paths[base + i] = sim
                    .world
                    .index(*p)
                    .ok_or_else(|| fail("path node outside the world"))?
                    as u32;
            }
            sim.rcols.push_row(record.row);
        }
        sim.items = EntityTable::restore_state(
            sim.config.max_items,
            state.item_ids,
            state.items.iter().map(|r| r.id),
        )
        .ok_or_else(|| fail("invalid item IDs"))?;
        for record in state.items {
            sim.icols.push_row(record.row);
        }
        let (w, d) = (sim.config.world_size_xy, sim.config.world_depth);
        sim.jobs = JobBoard::restore(
            sim.config.max_jobs as usize,
            w,
            w,
            d,
            state.next_job,
            state.jobs,
        )
        .ok_or_else(|| fail("invalid jobs"))?;
        sim.reservations = Reservations::restore(
            sim.config.max_residents as usize * 2 + 16,
            state.reservations,
        )
        .ok_or_else(|| fail("invalid reservations"))?;
        sim.queue =
            PathRequestQueue::restore(sim.config.max_residents as usize, state.path_requests)
                .ok_or_else(|| fail("invalid path requests"))?;
        sim.events = EventQueue::restore(16, state.event_sequence, state.events)
            .ok_or_else(|| fail("invalid scheduler state"))?;
        sim.counters = state.counters;
        sim.validate_state()?;
        Ok(sim)
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hash = StateHasher::new();
        hash.write_bytes(b"fortress-content-v1");
        self.config.hash_runtime(&mut hash);
        hash.write_u32(self.config.initial_residents);
        hash.write_u32(self.config.initial_food);
        hash.write_bool(self.config.debug_shuffle_slots);
        hash.finish()
    }

    pub(crate) fn write_debug_dump(&self, out: &mut String) -> Result<(), SimError> {
        out.clear();
        writeln!(
            out,
            "meta\ttick={} seed={} counters={:?} events={:?} next_job={}",
            self.tick,
            self.seed,
            self.counters,
            self.events.sorted_events(),
            self.jobs.next_id()
        )
        .map_err(fail)?;
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.residents.slot(id) else {
                continue;
            };
            write!(out, "resident:{}:{}\t", id.index, id.generation).map_err(fail)?;
            self.rcols.dump_row(slot, out);
            let path: Vec<_> = self.route_nodes(id).collect();
            writeln!(out, " path={path:?}").map_err(fail)?;
        }
        for id in self.items.iter_by_id() {
            let Some(slot) = self.items.slot(id) else {
                continue;
            };
            write!(out, "item:{}:{}\t", id.index, id.generation).map_err(fail)?;
            self.icols.dump_row(slot, out);
            writeln!(out).map_err(fail)?;
        }
        for job in self.jobs.iter() {
            writeln!(
                out,
                "job:{}\tpos={:?} priority={} state={:?} data={:?}",
                job.id, job.pos, job.priority, job.state, job.data
            )
            .map_err(fail)?;
        }
        for entry in self.reservations.iter() {
            writeln!(
                out,
                "reservation:{:?}\tholder={:?} since={}",
                entry.key, entry.holder, entry.since
            )
            .map_err(fail)?;
        }
        for request in self.queue.snapshot() {
            writeln!(
                out,
                "request:{}:{}\tstart={:?} goal={:?} submitted={}",
                request.key.index,
                request.key.generation,
                request.start,
                request.goal,
                request.submitted
            )
            .map_err(fail)?;
        }
        for p in &self.stockpiles {
            writeln!(out, "stockpile:{},{},{}\t1", p.x, p.y, p.z).map_err(fail)?;
        }
        for (index, tile) in self.tile_changes()? {
            if let Some(p) = self.world.pos(index as usize) {
                writeln!(
                    out,
                    "tile:{},{},{}\t{:?} floor={} material={}",
                    p.x, p.y, p.z, tile.kind, tile.floor, tile.material
                )
                .map_err(fail)?;
            }
        }
        Ok(())
    }
}
