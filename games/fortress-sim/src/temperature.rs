//! Heat as energy deviation from a depth-dependent ambient temperature.
//!
//! Only chunks with nonzero deviation (or a heat source) hold buffers from a fixed
//! pool. Temperature deviation is `energy / heat capacity` of the tile material.
//! Conduction runs over face pairs `(p, p + d)` for `d` in `+x, +y, +z`: the flux
//! `(T_p - T_q) * min(k_p, k_q) / divisor`, truncated toward zero, leaves `p` and
//! enters `q`, so every pair conserves energy exactly. A pair is processed by the
//! chunk of `p` if that chunk was active at the start of the phase, otherwise by the
//! active chunk of `q`; pairs into inactive chunks activate them. Exposed magma
//! keeps its temperature (energy counted as injected), deviations below the ambient
//! threshold return to zero (counted as returned), so
//! `total = injected - returned` holds exactly. A chunk whose deviations are all
//! zero holds no information, so releasing it is exact: sleeping is neutral.

use sim_core::{
    entity::EntityCommand,
    physics::local_index,
    space::{ChunkId, Pos3, TileKind},
};

use crate::{
    FortressSim, ItemKind, SOURCE_EVENT,
    fluids::{amount, is_magma},
    types::{ItemLocation, Op},
};

/// Heat counters. Logical state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HeatBalance {
    pub total: i64,
    pub injected: i64,
    pub returned: i64,
}

sim_core::state_hash_struct!(HeatBalance {
    total,
    injected,
    returned
});

const BOIL_UNITS: u8 = 1;

impl FortressSim {
    pub(crate) fn ambient(&self, z: i32) -> i32 {
        let p = &self.config.physics;
        p.surface_temperature + p.depth_warming * (self.config.world_depth - 1 - z)
    }

    fn capacity(&self, p: Pos3) -> i32 {
        self.world
            .get(p)
            .and_then(|t| self.config.materials.get(usize::from(t.material)))
            .map_or(4, |m| m.heat_capacity.max(1))
    }

    fn conductivity(&self, p: Pos3) -> i32 {
        self.world
            .get(p)
            .and_then(|t| self.config.materials.get(usize::from(t.material)))
            .map_or(4, |m| m.conductivity)
    }

    pub(crate) fn energy(&self, p: Pos3) -> i32 {
        self.world
            .chunk_of(p)
            .and_then(|c| self.heat.front(c))
            .map_or(0, |buffer| buffer[local_index(p)])
    }

    pub(crate) fn temperature(&self, p: Pos3) -> i32 {
        self.ambient(p.z) + self.energy(p) / self.capacity(p)
    }

    pub(crate) fn too_hot(&self, p: Pos3) -> bool {
        self.temperature(p) >= self.config.physics.heat_damage_at
    }

    fn exposed_magma(&self, p: Pos3) -> bool {
        let fluid = self.fluid_at(p);
        is_magma(fluid)
            && amount(fluid) > 0
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
                let q = p.offset(dx, dy, dz);
                self.world.get(q).is_some_and(|t| t.kind != TileKind::Solid)
                    && !is_magma(self.fluid_at(q))
            })
    }

    /// Recomputes exposed magma inside the given chunks and one tile around.
    pub(crate) fn update_heat_sources(&mut self, chunks: &[ChunkId]) {
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
            self.heat_sources.retain(|p| !inside(p));
            for z in low.z.max(0)..high.z {
                for y in low.y.max(0)..high.y {
                    for x in low.x.max(0)..high.x {
                        let p = Pos3::new(x, y, z);
                        if self.exposed_magma(p)
                            && self.heat_sources.len() < self.heat_sources.capacity()
                        {
                            self.heat_sources.push(p);
                        }
                    }
                }
            }
        }
        self.heat_sources.sort_unstable();
        self.heat_sources.dedup();
    }

    pub(crate) fn rebuild_heat_sources(&mut self) {
        self.heat_sources.clear();
        let all: Vec<ChunkId> = (0..self.world.chunk_count() as ChunkId).collect();
        self.update_heat_sources(&all);
    }

    fn add_energy(&mut self, p: Pos3, delta: i32) -> bool {
        let Some(chunk) = self.world.chunk_of(p) else {
            return false;
        };
        if !self.heat.acquire(chunk) {
            return false;
        }
        if let Some(buffer) = self.heat.back_mut(chunk) {
            buffer[local_index(p)] += delta;
        }
        true
    }

    fn set_marks(&mut self) {
        let count = self.world.chunk_count();
        self.scratch.heat_was_active.clear();
        self.scratch.heat_was_active.resize(count, false);
        self.scratch.heat_chunks.clear();
        for chunk in 0..count as ChunkId {
            if self.heat.has(chunk) {
                self.scratch.heat_was_active[chunk as usize] = true;
                self.scratch.heat_chunks.push(chunk);
            }
        }
    }

    pub(crate) fn temperature_step(&mut self) {
        // Sources keep their chunk bound and fix their temperature in the front buffer.
        for i in 0..self.heat_sources.len() {
            let p = self.heat_sources[i];
            if let Some(chunk) = self.world.chunk_of(p)
                && self.heat.acquire(chunk)
            {
                let target =
                    (self.config.physics.magma_temperature - self.ambient(p.z)) * self.capacity(p);
                if let Some(buffer) = self.heat.front_mut(chunk) {
                    let cell = &mut buffer[local_index(p)];
                    let delta = target - *cell;
                    *cell = target;
                    self.heat_balance.injected += i64::from(delta);
                    self.heat_balance.total += i64::from(delta);
                }
            }
        }
        self.set_marks();
        self.heat.begin_step();
        let divisor = self.config.physics.conduction_divisor;
        let mut processed = 0u64;
        for index in 0..self.scratch.heat_chunks.len() {
            let chunk = self.scratch.heat_chunks[index];
            let Some((low, high)) = self.world.chunk_bounds(chunk) else {
                continue;
            };
            for z in low.z..high.z {
                for y in low.y..high.y {
                    for x in low.x..high.x {
                        let p = Pos3::new(x, y, z);
                        processed += 1;
                        for (dx, dy, dz) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
                            let q = p.offset(dx, dy, dz);
                            if self.world.chunk_of(q).is_some() {
                                self.conduct(p, q, divisor);
                            }
                            // Pairs owned by an inactive chunk below this one.
                            let r = p.offset(-dx, -dy, -dz);
                            if let Some(owner) = self.world.chunk_of(r)
                                && owner != chunk
                                && !self.scratch.heat_was_active[owner as usize]
                            {
                                self.conduct(r, p, divisor);
                            }
                        }
                    }
                }
            }
        }
        self.diag.heat_tiles += processed;
        self.diag.heat_last_tiles = processed;
        self.heat.swap();
        self.heat_effects();
        self.settle_heat();
    }

    /// Flux between face neighbors `p` and `q`, processed once per pair.
    fn conduct(&mut self, p: Pos3, q: Pos3, divisor: i32) {
        let (Some(cp), Some(cq)) = (self.world.chunk_of(p), self.world.chunk_of(q)) else {
            return;
        };
        let tp = self.energy(p) / self.capacity(p);
        let tq = self.energy(q) / self.capacity(q);
        if tp == tq {
            return;
        }
        let k = self.conductivity(p).min(self.conductivity(q));
        let flux = (tp - tq) * k / divisor;
        if flux == 0 {
            return;
        }
        // Both sides must hold buffers; if the pool is exhausted the pair waits.
        let ok = self.heat.acquire(cp) && self.heat.acquire(cq);
        if !ok {
            self.diag.heat_pool_exhausted += 1;
            return;
        }
        self.add_energy(p, -flux);
        self.add_energy(q, flux);
    }

    /// Returns small deviations to ambient and releases all-zero chunks.
    fn settle_heat(&mut self) {
        let threshold = self.config.physics.ambient_threshold;
        let count = self.world.chunk_count() as ChunkId;
        let mut active = 0;
        for chunk in 0..count {
            if !self.heat.has(chunk) {
                continue;
            }
            let Some((low, high)) = self.world.chunk_bounds(chunk) else {
                continue;
            };
            let mut nonzero = false;
            for z in low.z..high.z {
                for y in low.y..high.y {
                    for x in low.x..high.x {
                        let p = Pos3::new(x, y, z);
                        let capacity = self.capacity(p);
                        let source = self.heat_sources.binary_search(&p).is_ok();
                        if let Some(buffer) = self.heat.front_mut(chunk) {
                            let cell = &mut buffer[local_index(p)];
                            if *cell != 0 && !source && (*cell / capacity).abs() < threshold {
                                self.heat_balance.returned += i64::from(*cell);
                                self.heat_balance.total -= i64::from(*cell);
                                *cell = 0;
                            }
                            nonzero |= *cell != 0 || source;
                        }
                    }
                }
            }
            if nonzero || !self.config.physics.sleeping {
                active += 1;
            } else {
                self.heat.release(chunk);
            }
        }
        self.diag.heat_last_chunks = active;
    }

    /// Boiling water, burning food and the heat threshold of the impassable marker.
    /// Runs before chunks are released, so cooled tiles lose the marker.
    fn heat_effects(&mut self) {
        let boil = self.config.physics.boil_at;
        let ignite = self.config.physics.food_ignition;
        self.scratch.hot.clear();
        self.scratch.fluid_tiles.clear();
        for chunk in 0..self.world.chunk_count() as ChunkId {
            if !self.heat.has(chunk) {
                continue;
            }
            if let Some((low, high)) = self.world.chunk_bounds(chunk) {
                for z in low.z..high.z {
                    for y in low.y..high.y {
                        for x in low.x..high.x {
                            let p = Pos3::new(x, y, z);
                            if let Some(&tile) = self.world.get(p)
                                && tile.kind != TileKind::Solid
                            {
                                let blocked =
                                    self.impassable_fluid(self.fluid_at(p)) || self.too_hot(p);
                                if tile.impassable() != blocked {
                                    self.scratch
                                        .fluid_tiles
                                        .push((p, tile.with_impassable(blocked)));
                                }
                            }
                        }
                    }
                }
            }
        }
        for i in 0..self.scratch.fluid_tiles.len() {
            let (p, tile) = self.scratch.fluid_tiles[i];
            self.world.set(p, tile);
        }
        self.scratch.fluid_tiles.clear();
        for chunk in 0..self.world.chunk_count() as ChunkId {
            if !self.heat.has(chunk) {
                continue;
            }
            if let Some((low, high)) = self.world.chunk_bounds(chunk) {
                for z in low.z..high.z {
                    for y in low.y..high.y {
                        for x in low.x..high.x {
                            let p = Pos3::new(x, y, z);
                            if self.energy(p) != 0 && self.temperature(p) >= boil.min(ignite) {
                                self.scratch.hot.push(p);
                            }
                        }
                    }
                }
            }
        }
        for i in 0..self.scratch.hot.len() {
            let p = self.scratch.hot[i];
            let temperature = self.temperature(p);
            let fluid = self.fluid_at(p);
            if temperature >= boil && !is_magma(fluid) && amount(fluid) > 0 {
                let gone = amount(fluid).min(BOIL_UNITS);
                self.boil(p, fluid - gone, gone);
            }
            if temperature >= ignite {
                for slot in 0..self.icols.len() {
                    if self.icols.kind[slot] == ItemKind::Food
                        && self.icols.location[slot] == ItemLocation::Ground(p)
                        && let Some(id) = self.items.id(slot as u32)
                    {
                        self.ops.push(
                            (SOURCE_EVENT, u64::MAX - 1),
                            id.index,
                            EntityCommand::Spawn(Op::DespawnItem(id)),
                        );
                        self.diag.items_destroyed += 1;
                    }
                }
            }
        }
    }

    /// Recounts the total energy deviation; used by invariants.
    pub(crate) fn count_energy(&self) -> i64 {
        let mut total = 0i64;
        for chunk in 0..self.world.chunk_count() as ChunkId {
            if let Some(buffer) = self.heat.front(chunk) {
                total += buffer.iter().map(|e| i64::from(*e)).sum::<i64>();
            }
        }
        total
    }

    /// Nonzero energy deviations in logical tile order.
    pub(crate) fn energy_cells(&self) -> Vec<(u32, i32)> {
        let mut cells = Vec::new();
        for chunk in 0..self.world.chunk_count() as ChunkId {
            if self.heat.front(chunk).is_none() {
                continue;
            }
            self.world.for_each_in_chunk(chunk, |p, _| {
                let e = self.energy(p);
                if e != 0 {
                    cells.push((self.world.index(p).unwrap_or(0) as u32, e));
                }
            });
        }
        cells.sort_unstable();
        cells
    }
}

impl FortressSim {
    /// Restores one energy cell into the front buffer when loading.
    pub(crate) fn load_energy(&mut self, p: Pos3, energy: i32) -> Result<(), sim_host::SimError> {
        let chunk = self
            .world
            .chunk_of(p)
            .ok_or_else(|| sim_host::SimError::Snapshot("energy outside the world".into()))?;
        if !self.heat.acquire(chunk) {
            return Err(sim_host::SimError::Snapshot(
                "heat pool exhausted while loading".into(),
            ));
        }
        if let Some(buffer) = self.heat.front_mut(chunk) {
            buffer[local_index(p)] = energy;
        }
        Ok(())
    }
}
