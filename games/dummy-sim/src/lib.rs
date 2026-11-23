//! A headless workload with moving units, energy, and regenerating resources.

use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use sim_host::{SimError, SimMetrics, Simulation, StateHasher};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DummyConfig {
    pub world_width: f64,
    pub world_height: f64,
    pub initial_units: usize,
    pub max_units: usize,
    pub resource_nodes: usize,
    pub node_capacity: f64,
    pub node_regen: f64,
    pub pickup_radius: f64,
    pub pickup_rate: f64,
    pub max_energy: f64,
    pub initial_energy: f64,
    pub energy_drain: f64,
    pub max_speed: f64,
    pub steering_jitter: f64,
}

impl Default for DummyConfig {
    fn default() -> Self {
        Self {
            world_width: 1000.0,
            world_height: 1000.0,
            initial_units: 10_000,
            max_units: 50_000,
            resource_nodes: 64,
            node_capacity: 1500.0,
            node_regen: 180.0,
            pickup_radius: 100.0,
            pickup_rate: 2.0,
            max_energy: 100.0,
            initial_energy: 80.0,
            energy_drain: 0.55,
            max_speed: 2.5,
            steering_jitter: 0.35,
        }
    }
}

impl DummyConfig {
    fn validate(&self) -> Result<(), SimError> {
        for (name, value) in [
            ("world_width", self.world_width),
            ("world_height", self.world_height),
            ("node_capacity", self.node_capacity),
            ("node_regen", self.node_regen),
            ("pickup_radius", self.pickup_radius),
            ("pickup_rate", self.pickup_rate),
            ("max_energy", self.max_energy),
            ("initial_energy", self.initial_energy),
            ("energy_drain", self.energy_drain),
            ("max_speed", self.max_speed),
            ("steering_jitter", self.steering_jitter),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(SimError::InvalidConfig(format!(
                    "{name} must be finite and positive"
                )));
            }
        }

        if self.initial_units == 0
            || self.initial_units > self.max_units
            || u32::try_from(self.max_units).is_err()
        {
            return Err(SimError::InvalidConfig(
                "unit counts must satisfy 0 < initial_units <= max_units <= u32::MAX".into(),
            ));
        }
        if self.resource_nodes == 0 {
            return Err(SimError::InvalidConfig(
                "resource_nodes must be positive".into(),
            ));
        }
        if self.initial_energy > self.max_energy {
            return Err(SimError::InvalidConfig(
                "initial_energy must not exceed max_energy".into(),
            ));
        }
        if ![
            self.world_width,
            self.world_height,
            self.pickup_radius,
            self.max_speed,
            self.steering_jitter,
        ]
        .into_iter()
        .all(|value| value <= (f64::MAX / 16.0).sqrt())
            || self.pickup_radius * self.pickup_radius < f64::MIN_POSITIVE
            || self.max_speed * self.max_speed < f64::MIN_POSITIVE
        {
            return Err(SimError::InvalidConfig(
                "geometry must support finite squared distances and speeds".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DummyCommand {
    SpawnWave { count: usize },
    SetEnergyDrain { drain: f64 },
}

pub struct DummySim {
    config: DummyConfig,
    x: Vec<f64>,
    y: Vec<f64>,
    vx: Vec<f64>,
    vy: Vec<f64>,
    energy: Vec<f64>,
    alive: Vec<bool>,
    free_slots: Vec<u32>,
    node_x: Vec<f64>,
    node_y: Vec<f64>,
    node_resources: Vec<f64>,
    rng: ChaCha8Rng,
    tick: u64,
    energy_drain: f64,
    spawned_total: u128,
    died_total: u128,
    rejected_spawns: u128,
    rejected_commands: u128,
}

impl DummySim {
    fn active_count(&self) -> usize {
        self.alive.len() - self.free_slots.len()
    }

    fn spawn_wave(&mut self, count: usize) {
        let accepted = count.min(self.config.max_units - self.active_count());
        self.rejected_spawns = self
            .rejected_spawns
            .saturating_add((count - accepted) as u128);

        for _ in 0..accepted {
            let x = uniform(&mut self.rng) * self.config.world_width;
            let y = uniform(&mut self.rng) * self.config.world_height;
            let (vx, vy) = limit_speed(
                signed_uniform(&mut self.rng) * self.config.max_speed,
                signed_uniform(&mut self.rng) * self.config.max_speed,
                self.config.max_speed,
            );
            let energy = self.config.initial_energy;

            if let Some(slot) = self.free_slots.pop() {
                let i = slot as usize;
                self.x[i] = x;
                self.y[i] = y;
                self.vx[i] = vx;
                self.vy[i] = vy;
                self.energy[i] = energy;
                self.alive[i] = true;
            } else {
                self.x.push(x);
                self.y.push(y);
                self.vx.push(vx);
                self.vy.push(vy);
                self.energy.push(energy);
                self.alive.push(true);
            }
        }
        self.spawned_total += accepted as u128;
    }

    fn apply_command(&mut self, command: &DummyCommand) {
        match *command {
            DummyCommand::SpawnWave { count } => self.spawn_wave(count),
            DummyCommand::SetEnergyDrain { drain } if drain.is_finite() && drain >= 0.0 => {
                self.energy_drain = drain;
            }
            DummyCommand::SetEnergyDrain { .. } => {
                self.rejected_commands = self.rejected_commands.saturating_add(1);
            }
        }
    }

    fn closest_node(&self, x: f64, y: f64, radius_squared: f64) -> Option<usize> {
        let mut best = None;
        let mut best_distance = radius_squared;
        for i in 0..self.node_resources.len() {
            if self.node_resources[i] <= 0.0 {
                continue;
            }
            let dx = x - self.node_x[i];
            let dy = y - self.node_y[i];
            let distance = dx * dx + dy * dy;
            if distance < best_distance || (distance == best_distance && best.is_none()) {
                best_distance = distance;
                best = Some(i);
            }
        }
        best
    }
}

impl Simulation for DummySim {
    type Config = DummyConfig;
    type Command = DummyCommand;

    fn new(seed: u64, config: DummyConfig) -> Result<Self, SimError> {
        config.validate()?;
        let units = config.max_units;
        let nodes = config.resource_nodes;
        let mut sim = Self {
            energy_drain: config.energy_drain,
            config,
            x: reserve(units)?,
            y: reserve(units)?,
            vx: reserve(units)?,
            vy: reserve(units)?,
            energy: reserve(units)?,
            alive: reserve(units)?,
            free_slots: reserve(units)?,
            node_x: reserve(nodes)?,
            node_y: reserve(nodes)?,
            node_resources: reserve(nodes)?,
            rng: ChaCha8Rng::seed_from_u64(seed),
            tick: 0,
            spawned_total: 0,
            died_total: 0,
            rejected_spawns: 0,
            rejected_commands: 0,
        };
        for _ in 0..nodes {
            sim.node_x
                .push(uniform(&mut sim.rng) * sim.config.world_width);
            sim.node_y
                .push(uniform(&mut sim.rng) * sim.config.world_height);
            sim.node_resources.push(sim.config.node_capacity);
        }
        sim.spawn_wave(sim.config.initial_units);
        Ok(sim)
    }

    fn step(&mut self, commands: &[DummyCommand]) {
        for command in commands {
            self.apply_command(command);
        }

        let config = &self.config;
        let radius_squared = config.pickup_radius * config.pickup_radius;
        for i in 0..self.alive.len() {
            if !self.alive[i] {
                continue;
            }
            let vx = self.vx[i] + signed_uniform(&mut self.rng) * config.steering_jitter;
            let vy = self.vy[i] + signed_uniform(&mut self.rng) * config.steering_jitter;
            let (vx, vy) = limit_speed(vx, vy, config.max_speed);

            (self.x[i], self.vx[i]) = move_axis(self.x[i], vx, config.world_width);
            (self.y[i], self.vy[i]) = move_axis(self.y[i], vy, config.world_height);
            self.energy[i] -= self.energy_drain;
            if let Some(node) = self.closest_node(self.x[i], self.y[i], radius_squared) {
                let pickup = config
                    .pickup_rate
                    .min(self.node_resources[node])
                    .min(config.max_energy - self.energy[i]);
                self.energy[i] = (self.energy[i] + pickup).min(config.max_energy);
                self.node_resources[node] -= pickup;
            }
            if self.energy[i] <= 0.0 {
                self.alive[i] = false;
                self.free_slots.push(i as u32);
                self.died_total += 1;
            }
        }
        for resources in &mut self.node_resources {
            *resources = (*resources + config.node_regen).min(config.node_capacity);
        }
        self.tick += 1;
    }

    fn tick(&self) -> u64 {
        self.tick
    }

    fn is_finished(&self) -> bool {
        self.active_count() == 0
    }

    fn metrics(&self) -> SimMetrics {
        let active_entities = self.active_count();
        let mut avg_energy = 0.0;
        if active_entities != 0 {
            for (&energy, &alive) in self.energy.iter().zip(&self.alive) {
                if alive {
                    avg_energy += energy / active_entities as f64;
                }
            }
        }
        let custom_gauges = [
            ("avg_energy", avg_energy),
            ("total_node_resources", self.node_resources.iter().sum()),
            ("spawned_total", self.spawned_total as f64),
            ("died_total", self.died_total as f64),
            ("rejected_spawns", self.rejected_spawns as f64),
            ("rejected_commands", self.rejected_commands as f64),
            ("energy_drain", self.energy_drain),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value))
        .collect();
        SimMetrics {
            active_entities,
            custom_gauges,
        }
    }

    fn state_hash(&self) -> u64 {
        let mut hash = StateHasher::new();
        let config = &self.config;
        hash.write_u64(self.tick);
        hash.write_bytes(&self.rng.get_seed());
        hash.write_u128(self.rng.get_word_pos());
        hash.write_usize(config.max_units);
        for value in [
            config.world_width,
            config.world_height,
            config.node_capacity,
            config.node_regen,
            config.pickup_radius,
            config.pickup_rate,
            config.max_energy,
            config.initial_energy,
            config.max_speed,
            config.steering_jitter,
            self.energy_drain,
        ] {
            hash.write_f64(value);
        }
        hash.write_u128(self.spawned_total);
        hash.write_u128(self.died_total);
        hash.write_u128(self.rejected_spawns);
        hash.write_u128(self.rejected_commands);

        hash.write_usize(self.node_resources.len());
        for i in 0..self.node_resources.len() {
            hash.write_f64(self.node_x[i]);
            hash.write_f64(self.node_y[i]);
            hash.write_f64(self.node_resources[i]);
        }
        hash.write_usize(self.alive.len());
        for (i, &alive) in self.alive.iter().enumerate() {
            hash.write_bool(alive);
            if alive {
                hash.write_f64(self.x[i]);
                hash.write_f64(self.y[i]);
                hash.write_f64(self.vx[i]);
                hash.write_f64(self.vy[i]);
                hash.write_f64(self.energy[i]);
            }
        }
        hash.write_usize(self.free_slots.len());
        for &slot in &self.free_slots {
            hash.write_u32(slot);
        }
        hash.finish()
    }

    fn check_invariants(&self) -> Result<(), SimError> {
        let config = &self.config;
        let slots = self.alive.len();
        let mut seen = vec![false; slots];
        for &slot in &self.free_slots {
            let i = slot as usize;
            if i >= slots || self.alive[i] || seen[i] {
                return Err(SimError::InvariantViolation(format!(
                    "invalid or repeated free slot {i}"
                )));
            }
            seen[i] = true;
        }

        let speed_limit = config.max_speed * (1.0 + 1e-12);
        let mut live = 0;
        for i in 0..slots {
            if !self.alive[i] {
                continue;
            }
            live += 1;
            let energy = self.energy[i];
            let vx = self.vx[i];
            let vy = self.vy[i];
            if !(0.0..=config.world_width).contains(&self.x[i])
                || !(0.0..=config.world_height).contains(&self.y[i])
                || !energy.is_finite()
                || energy <= 0.0
                || energy > config.max_energy
                || !vx.is_finite()
                || !vy.is_finite()
                || (vx * vx + vy * vy).sqrt() > speed_limit
            {
                return Err(SimError::InvariantViolation(format!(
                    "invalid position, energy, or speed in live slot {i}"
                )));
            }
        }
        if live + self.free_slots.len() != slots {
            return Err(SimError::InvariantViolation(
                "live and free counts do not cover all slots".into(),
            ));
        }
        for (i, &resources) in self.node_resources.iter().enumerate() {
            if !(0.0..=config.node_capacity).contains(&resources) {
                return Err(SimError::InvariantViolation(format!(
                    "invalid resources at node {i}"
                )));
            }
        }
        Ok(())
    }
}

fn reserve<T>(capacity: usize) -> Result<Vec<T>, SimError> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).map_err(|error| {
        SimError::InvalidConfig(format!("could not reserve {capacity} entries: {error}"))
    })?;
    Ok(values)
}

fn uniform(rng: &mut ChaCha8Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

fn signed_uniform(rng: &mut ChaCha8Rng) -> f64 {
    uniform(rng) * 2.0 - 1.0
}

fn limit_speed(vx: f64, vy: f64, max_speed: f64) -> (f64, f64) {
    let speed_squared = vx * vx + vy * vy;
    if speed_squared > max_speed * max_speed {
        let scale = max_speed / speed_squared.sqrt();
        (vx * scale, vy * scale)
    } else {
        (vx, vy)
    }
}

fn move_axis(position: f64, velocity: f64, limit: f64) -> (f64, f64) {
    let next = position + velocity;
    if (next <= 0.0 && velocity < 0.0) || (next >= limit && velocity > 0.0) {
        (next.clamp(0.0, limit), -velocity)
    } else {
        (next, velocity)
    }
}

#[cfg(test)]
mod tests;
