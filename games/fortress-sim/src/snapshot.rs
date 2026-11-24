//! Snapshot payload version 4. The world is stored as the tiles that differ from
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
struct PlantRecord {
    id: crate::types::PlantId,
    row: crate::PlantRow,
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
    fluids: Vec<(u32, u8)>,
    balance: crate::fluids::FluidBalance,
    energy: Vec<(u32, i32)>,
    heat_balance: crate::temperature::HeatBalance,
    support: Vec<(Pos3, u8)>,
    ledger: Vec<crate::types::Ledger>,
    plant_ids: AllocatorState,
    plants: Vec<PlantRecord>,
    farms: Vec<crate::types::FarmPlot>,
    workshops: Vec<crate::types::Workshop>,
    next_workshop: u32,
    slot_zone: Vec<u32>,
    zones: Vec<crate::types::Zone>,
    next_zone: u32,
    orders: Vec<crate::types::WorkOrder>,
    next_order: u32,
}

/// Payload version 2 starts with this tag and the generator fingerprint. Version 1
/// payloads start directly with the state and are accepted with a warning.
const TAG_V2: [u8; 8] = *b"FORTSNP2";
const TAG_V3: [u8; 8] = *b"FORTSNP3";
/// Payload version 4 adds the content fingerprint after the generator fingerprint.
const TAG_V4: [u8; 8] = *b"FORTSNP4";

#[derive(Serialize, Deserialize)]
struct Generator {
    version: u32,
    hash: u64,
}

fn fail(error: impl ToString) -> SimError {
    SimError::Snapshot(error.to_string())
}

impl FortressSim {
    /// Logical indices and tiles that differ from the generated world.
    fn tile_changes(&self) -> Result<Vec<(u32, Tile)>, SimError> {
        let generated = world::generate(self.seed, &self.config, &self.content)?;
        Ok(self
            .world
            .iter_logical()
            .zip(generated.iter_logical())
            .enumerate()
            .filter(|(_, ((_, now), (_, then)))| now != then)
            .map(|(index, ((_, tile), _))| (index as u32, *tile))
            .collect())
    }

    /// Logical indices and fluid bytes that differ from the generated fluids.
    fn fluid_changes(&self) -> Result<Vec<(u32, u8)>, SimError> {
        let generated =
            world::generate_fluids(&self.config).ok_or_else(|| fail("could not reserve fluids"))?;
        Ok(self
            .fluid
            .iter_logical()
            .zip(generated.iter_logical())
            .enumerate()
            .filter(|(_, ((_, now), (_, then)))| now != then)
            .map(|(index, ((_, value), _))| (index as u32, *value))
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
            fluids: self.fluid_changes()?,
            balance: self.balance,
            energy: self.energy_cells(),
            heat_balance: self.heat_balance,
            support: self.support.pending(),
            ledger: self.ledger.clone(),
            plant_ids: self.plants.snapshot_state(),
            plants: self
                .plants
                .iter_by_id()
                .filter_map(|id| {
                    let row = self.plants.slot(id).and_then(|slot| self.pcols.row(slot))?;
                    Some(PlantRecord { id, row })
                })
                .collect(),
            farms: self.farms.clone(),
            workshops: self.workshops.clone(),
            next_workshop: self.next_workshop,
            slot_zone: self.slot_zone.clone(),
            zones: self.zones.clone(),
            next_zone: self.next_zone,
            orders: self.orders.clone(),
            next_order: self.next_order,
        };
        let (version, hash) = world::generator_fingerprint(&self.config, &self.content);
        *out = postcard::to_allocvec(&(
            TAG_V4,
            Generator { version, hash },
            self.content.fingerprint,
            state,
        ))
        .map_err(fail)?;
        Ok(())
    }

    pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<Self, SimError> {
        // The generator version is checked before anything else is decoded. Payload
        // versions 1 and 2 belong to generator version 1 and cannot be rebuilt.
        if ![TAG_V2, TAG_V3, TAG_V4]
            .iter()
            .any(|tag| bytes.starts_with(tag))
        {
            return Err(fail(format!(
                "Weltgenerator geändert: Version 1 → {}",
                world::GENERATOR_VERSION
            )));
        }
        let ((tag, generator), rest) =
            postcard::take_from_bytes::<([u8; 8], Generator)>(bytes).map_err(fail)?;
        if generator.version != world::GENERATOR_VERSION {
            return Err(fail(format!(
                "Weltgenerator geändert: Version {} → {}",
                generator.version,
                world::GENERATOR_VERSION
            )));
        }
        if tag != TAG_V4 {
            return Err(fail(
                "Fortress-Snapshot-Nutzlast v2/v3 wird nicht mehr unterstützt",
            ));
        }
        let generator = Some(generator);
        let (stored_content, rest) = postcard::take_from_bytes::<u64>(rest).map_err(fail)?;
        let (state, rest) = postcard::take_from_bytes::<State>(rest).map_err(fail)?;
        if !rest.is_empty() {
            return Err(fail("trailing snapshot data"));
        }
        let config: FortressConfig = serde_json::from_str(&state.config).map_err(fail)?;
        config.validate()?;
        let content = crate::GameContent::load(&config.mods)
            .map_err(|error| fail(format!("Inhalt nicht ladbar: {error}")))?;
        if content.fingerprint != stored_content {
            return Err(fail(format!(
                "Inhalt geändert: Fingerabdruck {stored_content:#018x} → {:#018x}",
                content.fingerprint
            )));
        }
        let (version, hash) = world::generator_fingerprint(&config, &content);
        let warning = match generator {
            Some(stored) if stored.version != version => {
                return Err(fail(format!(
                    "Weltgenerator geändert: Version {} → {version}",
                    stored.version
                )));
            }
            Some(stored) if stored.hash != hash => {
                return Err(fail(format!(
                    "Weltgenerator geändert: Fingerabdruck {:#018x} → {hash:#018x}",
                    stored.hash
                )));
            }
            Some(_) => None,
            None => None::<String>,
        };
        let mut world = world::generate(state.seed, &config, &content)?;
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
        let mut sim = Self::empty(state.seed, config, content, world)?;
        sim.tick = state.tick;
        let cap = sim.config.max_path_nodes as usize;
        if state.stockpiles.windows(2).any(|pair| pair[0] >= pair[1])
            || state.stockpiles.len() > crate::STOCKPILE_CAPACITY
        {
            return Err(fail("invalid stockpiles"));
        }
        if state.slot_zone.len() != state.stockpiles.len()
            || state.zones.windows(2).any(|pair| pair[0].id >= pair[1].id)
            || state.zones.iter().any(|z| z.id >= state.next_zone)
            || state
                .slot_zone
                .iter()
                .any(|id| state.zones.binary_search_by_key(id, |z| z.id).is_err())
        {
            return Err(fail("invalid stockpile zones"));
        }
        // Extend the reserved buffers so later inserts do not allocate in a step.
        sim.slot_items.resize(state.stockpiles.len(), 0);
        sim.slot_item.resize(state.stockpiles.len(), None);
        sim.stockpiles.extend(state.stockpiles);
        sim.slot_zone.extend(state.slot_zone);
        sim.zones.extend(state.zones);
        sim.next_zone = state.next_zone;
        if state.orders.windows(2).any(|pair| pair[0].id >= pair[1].id)
            || state.orders.iter().any(|o| o.id >= state.next_order)
            || state.orders.len() > crate::orders::MAX_ORDERS
        {
            return Err(fail("work orders must ascend by ID below the next ID"));
        }
        sim.orders.extend(state.orders);
        sim.next_order = state.next_order;
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
        if state.ledger.len() != sim.ledger.len() {
            return Err(fail("item ledger does not match the content"));
        }
        sim.ledger = state.ledger;
        sim.plants = EntityTable::restore_state(
            sim.config.max_plants,
            state.plant_ids,
            state.plants.iter().map(|r| r.id),
        )
        .ok_or_else(|| fail("invalid plant IDs"))?;
        for record in state.plants {
            sim.pcols.push_row(record.row);
        }
        if state
            .farms
            .windows(2)
            .any(|pair| pair[0].pos >= pair[1].pos)
            || state.farms.len() > sim.config.max_farm_plots as usize
        {
            return Err(fail("farm plots must ascend by position"));
        }
        sim.farms.extend(state.farms);
        if state
            .workshops
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
            || state.workshops.iter().any(|w| w.id >= state.next_workshop)
            || state.workshops.len() > crate::MAX_WORKSHOPS
        {
            return Err(fail("workshops must ascend by ID below the next ID"));
        }
        sim.workshops.extend(state.workshops);
        sim.next_workshop = state.next_workshop;
        sim.rebuild_item_index();
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
        sim.load_warnings.extend(warning);
        if state.fluids.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(fail("fluid changes must ascend by index"));
        }
        for (index, value) in state.fluids {
            let p = sim
                .fluid
                .pos(index as usize)
                .ok_or_else(|| fail("fluid change outside the world"))?;
            sim.fluid.set(p, value);
        }
        sim.fluid.clear_changes();
        sim.balance = state.balance;
        sim.heat_balance = state.heat_balance;
        sim.support.restore(&state.support);
        sim.init_physics();
        if state.energy.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(fail("energy cells must ascend by index"));
        }
        for (index, energy) in state.energy {
            let p = sim
                .world
                .pos(index as usize)
                .ok_or_else(|| fail("energy cell outside the world"))?;
            sim.load_energy(p, energy)?;
        }
        sim.validate_state()?;
        Ok(sim)
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hash = StateHasher::new();
        hash.write_bytes(b"fortress-content-v1");
        self.config.hash_runtime(&mut hash);
        hash.write_u64(self.content.fingerprint);
        hash.write_u32(self.config.initial_residents);
        hash.write_u32(self.config.starting_food());
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
        for (p, zone) in self.stockpiles.iter().zip(&self.slot_zone) {
            writeln!(out, "stockpile:{},{},{}\tzone={zone}", p.x, p.y, p.z).map_err(fail)?;
        }
        for id in self.plants.iter_by_id() {
            let Some(slot) = self.pslot(id) else { continue };
            write!(out, "plant:{}:{}\t", id.index, id.generation).map_err(fail)?;
            self.pcols.dump_row(slot as u32, out);
            writeln!(out).map_err(fail)?;
        }
        for o in &self.orders {
            writeln!(
                out,
                "order:{}\tworkshop={:?} recipe={} amount={} condition={:?} done={}",
                o.id,
                o.workshop,
                self.content.ids.recipes[usize::from(o.recipe)],
                o.amount,
                o.condition,
                o.done
            )
            .map_err(fail)?;
        }
        for z in &self.zones {
            writeln!(
                out,
                "zone:{}\tcategories={:#x} materials={:?} priority={} containers={}",
                z.id,
                z.categories,
                z.materials(),
                z.priority,
                z.containers
            )
            .map_err(fail)?;
        }
        for w in &self.workshops {
            writeln!(
                out,
                "workshop:{}\tkind={} pos={:?} built={} job={:?}",
                w.id,
                self.content.ids.workshops[usize::from(w.kind)],
                w.pos,
                w.built,
                w.job
            )
            .map_err(fail)?;
        }
        for plot in &self.farms {
            writeln!(
                out,
                "farm:{},{},{}\tcrop={} plant={:?} job={:?}",
                plot.pos.x, plot.pos.y, plot.pos.z, plot.crop, plot.plant, plot.job
            )
            .map_err(fail)?;
        }
        for (kind, entry) in self.ledger.iter().enumerate() {
            writeln!(
                out,
                "ledger:{}\tproduced={} removed={}",
                self.content.ids.items[kind], entry.produced, entry.removed
            )
            .map_err(fail)?;
        }
        for (index, tile) in self.tile_changes()? {
            if let Some(p) = self.world.pos(index as usize) {
                writeln!(
                    out,
                    "tile:{},{},{}\t{:?} floor={} impassable={} material={}",
                    p.x,
                    p.y,
                    p.z,
                    tile.kind,
                    tile.floor(),
                    tile.impassable(),
                    tile.material
                )
                .map_err(fail)?;
            }
        }
        Ok(())
    }
}
