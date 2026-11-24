use serde::{Deserialize, Serialize};
use sim_host::{SimError, StateHasher};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MaterialConfig {
    pub name: String,
    pub dig_ticks: u32,
    pub ore: bool,
    /// Water-bearing rock that releases water into adjacent open tiles.
    pub aquifer: bool,
    /// Whether the generator uses the material as a rock layer.
    pub layer: bool,
    /// Heat conductivity and capacity in integer units.
    pub conductivity: i32,
    pub heat_capacity: i32,
}

impl Default for MaterialConfig {
    fn default() -> Self {
        Self {
            name: "stone".into(),
            dig_ticks: 20,
            ore: false,
            aquifer: false,
            layer: true,
            conductivity: 4,
            heat_capacity: 4,
        }
    }
}

fn material(name: &str, dig_ticks: u32, ore: bool) -> MaterialConfig {
    MaterialConfig {
        name: name.into(),
        dig_ticks,
        ore,
        ..MaterialConfig::default()
    }
}

/// World physics settings. Temperatures are absolute integer degrees.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhysicsConfig {
    /// Disabling sleep simulates every woken chunk each tick; results are identical.
    pub sleeping: bool,
    /// Stable updates before a chunk sleeps; at least the rotation period of 4.
    pub sleep_after: u16,
    pub river: bool,
    pub river_width: i32,
    pub river_initial_level: u8,
    pub magma_sea: bool,
    pub aquifer_center_x: i32,
    pub aquifer_center_y: i32,
    pub aquifer_radius: i32,
    pub aquifer_z_low: i32,
    pub aquifer_z_high: i32,
    /// Each exposed aquifer face adds one unit of water every this many ticks.
    pub aquifer_interval: u64,
    /// Shallow surface water evaporates one unit with chance 1 in this per tick.
    pub evaporation_chance: u64,
    pub water_impassable_at: u8,
    pub magma_impassable_at: u8,
    pub drown_ticks: u16,
    pub surface_temperature: i32,
    /// Ambient warming per level below the top of the world.
    pub depth_warming: i32,
    pub magma_temperature: i32,
    /// Divides pairwise heat flux; larger values conduct more slowly.
    pub conduction_divisor: i32,
    /// Temperature deviations below this return to ambient.
    pub ambient_threshold: i32,
    pub heat_damage_at: i32,
    pub boil_at: i32,
    pub food_ignition: i32,
    pub heat_pool_chunks: u32,
    /// Support checks: flood-fill units per tick and initial limit per check.
    pub support_budget: u64,
    pub support_check_limit: u64,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            sleeping: true,
            sleep_after: 4,
            river: true,
            river_width: 3,
            river_initial_level: 5,
            magma_sea: true,
            aquifer_center_x: 60,
            aquifer_center_y: 40,
            aquifer_radius: 10,
            aquifer_z_low: 9,
            aquifer_z_high: 10,
            aquifer_interval: 6,
            evaporation_chance: 100,
            water_impassable_at: 4,
            magma_impassable_at: 1,
            drown_ticks: 150,
            surface_temperature: 10,
            depth_warming: 1,
            magma_temperature: 1200,
            conduction_divisor: 16,
            ambient_threshold: 2,
            heat_damage_at: 80,
            boil_at: 100,
            food_ignition: 180,
            heat_pool_chunks: 192,
            support_budget: 20_000,
            support_check_limit: 4_096,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pathfinding {
    Astar,
    Hierarchical,
}

/// Effective settings. Material 0 is the surface soil; the remaining non-ore
/// materials form rock layers from top to bottom; ore materials form veins.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FortressConfig {
    pub world_size_xy: i32,
    pub world_depth: i32,
    pub ground_level: i32,
    pub mountain_center_x: i32,
    pub mountain_center_y: i32,
    pub mountain_radius: i32,
    pub mountain_height: i32,
    pub ore_veins: u32,
    pub ore_vein_length: u32,
    pub materials: Vec<MaterialConfig>,
    pub embark_x: i32,
    pub embark_y: i32,
    pub initial_residents: u32,
    pub initial_food: u32,
    /// Additional starting rations per initial resident.
    pub initial_food_per_resident: u32,
    pub max_residents: u32,
    pub max_items: u32,
    pub max_jobs: u32,
    pub max_path_nodes: u32,
    pub hunger_rate: u16,
    pub hungry_at: u16,
    pub starve_at: u16,
    pub fatigue_rate: u16,
    pub tired_at: u16,
    pub sleep_recovery: u16,
    pub eat_ticks: u32,
    pub build_ticks: u32,
    pub move_speed: u32,
    pub caravan_interval: u64,
    pub caravan_food_per_resident: u32,
    pub caravan_food_min: u32,
    pub job_candidates: u32,
    pub job_inspect_limit: u32,
    pub reservation_timeout: u64,
    pub path_budget_per_tick: u32,
    pub path_expansion_limit: u32,
    pub pathfinding: Pathfinding,
    pub physics: PhysicsConfig,
    pub haul_scan_interval: u64,
    pub debug_shuffle_slots: bool,
}

impl Default for FortressConfig {
    fn default() -> Self {
        Self {
            world_size_xy: 96,
            world_depth: 32,
            ground_level: 14,
            mountain_center_x: 48,
            mountain_center_y: 48,
            mountain_radius: 36,
            mountain_height: 16,
            ore_veins: 24,
            ore_vein_length: 40,
            materials: vec![
                material("soil", 8, false),
                material("sandstone", 14, false),
                material("limestone", 18, false),
                material("granite", 26, false),
                material("basalt", 32, false),
                material("iron_ore", 30, true),
                material("copper_ore", 24, true),
                material("gold_ore", 36, true),
                MaterialConfig {
                    aquifer: true,
                    layer: false,
                    ..material("wet_sandstone", 14, false)
                },
                MaterialConfig {
                    conductivity: 6,
                    heat_capacity: 6,
                    layer: false,
                    ..material("obsidian", 60, false)
                },
            ],
            embark_x: 6,
            embark_y: 48,
            initial_residents: 25,
            initial_food: 60,
            initial_food_per_resident: 0,
            max_residents: 2_000,
            max_items: 40_000,
            max_jobs: 20_000,
            max_path_nodes: 1_024,
            hunger_rate: 1,
            hungry_at: 1_500,
            starve_at: 3_000,
            fatigue_rate: 1,
            tired_at: 3_000,
            sleep_recovery: 10,
            eat_ticks: 20,
            build_ticks: 30,
            move_speed: 10,
            caravan_interval: 1_000,
            caravan_food_per_resident: 1,
            caravan_food_min: 10,
            job_candidates: 8,
            job_inspect_limit: 64,
            reservation_timeout: 3_000,
            path_budget_per_tick: 20_000,
            path_expansion_limit: 8_000,
            pathfinding: Pathfinding::Astar,
            physics: PhysicsConfig::default(),
            haul_scan_interval: 64,
            debug_shuffle_slots: false,
        }
    }
}

impl FortressConfig {
    pub(crate) fn validate(&self) -> Result<(), SimError> {
        let fail = |message: &str| Err(SimError::InvalidConfig(message.into()));
        if !(16..=1024).contains(&self.world_size_xy)
            || !(8..=128).contains(&self.world_depth)
            || !(2..self.world_depth - 2).contains(&self.ground_level)
            || self.ground_level + self.mountain_height >= self.world_depth
            || self.mountain_radius < 1
            || self.mountain_height < 0
            || !(0..self.world_size_xy).contains(&self.embark_x)
            || !(0..self.world_size_xy).contains(&self.embark_y)
        {
            return fail("invalid world dimensions, ground level or mountain");
        }
        if self.materials.len() < 2
            || self.materials.len() > usize::from(u16::MAX)
            || self
                .materials
                .iter()
                .any(|m| m.dig_ticks == 0 || m.dig_ticks > 100_000)
            || self.materials[0].ore
            || !self.materials.iter().skip(1).any(|m| !m.ore && m.layer)
            || self
                .materials
                .iter()
                .any(|m| m.conductivity < 0 || m.heat_capacity <= 0)
            || self.obsidian().is_none()
        {
            return fail("materials need soil first, at least one rock and positive dig times");
        }
        if self.initial_residents > self.max_residents
            || !(1..=1_000_000).contains(&self.max_residents)
            || !(1..=4_000_000).contains(&self.max_items)
            || u64::from(self.initial_food)
                + u64::from(self.initial_food_per_resident) * u64::from(self.initial_residents)
                > u64::from(self.max_items)
            || !(1..=4_000_000).contains(&self.max_jobs)
            || !(8..=65_535).contains(&self.max_path_nodes)
        {
            return fail("invalid capacities");
        }
        if self.hunger_rate == 0
            || self.hungry_at == 0
            || self.starve_at <= self.hungry_at
            || self.fatigue_rate == 0
            || self.tired_at == 0
            || self.sleep_recovery == 0
            || self.eat_ticks == 0
            || self.build_ticks == 0
            || self.move_speed == 0
            || self.move_speed > 1_000
            || self.caravan_interval == 0
            || self.job_candidates == 0
            || self.job_inspect_limit < self.job_candidates
            || self.reservation_timeout == 0
            || self.path_budget_per_tick == 0
            || self.path_expansion_limit == 0
            || self.haul_scan_interval == 0
            || !(1..=7).contains(&self.physics.water_impassable_at)
            || !(1..=7).contains(&self.physics.magma_impassable_at)
            || self.physics.river_initial_level > 7
            || self.physics.sleep_after < 4
            || self.physics.aquifer_interval == 0
            || self.physics.evaporation_chance == 0
            || self.physics.conduction_divisor < 8
            || self.physics.support_budget == 0
            || self.physics.support_check_limit == 0
        {
            return fail("invalid rates, durations or budgets");
        }
        Ok(())
    }

    /// Material produced when water meets magma.
    pub(crate) fn obsidian(&self) -> Option<u16> {
        self.materials
            .iter()
            .position(|m| m.name == "obsidian")
            .and_then(|i| u16::try_from(i).ok())
    }

    pub(crate) fn aquifer_material(&self) -> Option<u16> {
        self.materials
            .iter()
            .position(|m| m.aquifer)
            .and_then(|i| u16::try_from(i).ok())
    }

    pub(crate) fn starting_food(&self) -> u32 {
        self.initial_food.saturating_add(
            self.initial_food_per_resident
                .saturating_mul(self.initial_residents),
        )
    }

    pub(crate) fn hash_runtime(&self, hash: &mut StateHasher) {
        for value in [
            self.world_size_xy,
            self.world_depth,
            self.ground_level,
            self.mountain_center_x,
            self.mountain_center_y,
            self.mountain_radius,
            self.mountain_height,
            self.embark_x,
            self.embark_y,
        ] {
            hash.write_u32(value as u32);
        }
        for value in [
            self.ore_veins,
            self.ore_vein_length,
            self.max_residents,
            self.max_items,
            self.max_jobs,
            self.max_path_nodes,
            self.eat_ticks,
            self.build_ticks,
            self.move_speed,
            self.caravan_food_per_resident,
            self.caravan_food_min,
            self.job_candidates,
            self.job_inspect_limit,
            self.path_budget_per_tick,
            self.path_expansion_limit,
        ] {
            hash.write_u32(value);
        }
        for value in [
            self.hunger_rate,
            self.hungry_at,
            self.starve_at,
            self.fatigue_rate,
            self.tired_at,
            self.sleep_recovery,
        ] {
            hash.write_u32(u32::from(value));
        }
        for value in [
            self.caravan_interval,
            self.reservation_timeout,
            self.haul_scan_interval,
        ] {
            hash.write_u64(value);
        }
        hash.write_u8(self.pathfinding as u8);
        let p = &self.physics;
        hash.write_bool(p.river);
        hash.write_bool(p.magma_sea);
        for value in [
            p.river_width,
            p.aquifer_center_x,
            p.aquifer_center_y,
            p.aquifer_radius,
            p.aquifer_z_low,
            p.aquifer_z_high,
            p.surface_temperature,
            p.depth_warming,
            p.magma_temperature,
            p.conduction_divisor,
            p.ambient_threshold,
            p.heat_damage_at,
            p.boil_at,
            p.food_ignition,
        ] {
            hash.write_u32(value as u32);
        }
        for value in [
            p.aquifer_interval,
            p.evaporation_chance,
            p.support_budget,
            p.support_check_limit,
            u64::from(p.heat_pool_chunks),
        ] {
            hash.write_u64(value);
        }
        for value in [
            p.river_initial_level,
            p.water_impassable_at,
            p.magma_impassable_at,
        ] {
            hash.write_u8(value);
        }
        hash.write_u32(u32::from(p.drown_ticks));
        // Sleeping is excluded: it is proven not to change behavior.
        hash.write_usize(self.materials.len());
        for m in &self.materials {
            hash.write_bytes(m.name.as_bytes());
            hash.write_u8(0);
            hash.write_u32(m.dig_ticks);
            hash.write_bool(m.ore);
            hash.write_bool(m.aquifer);
            hash.write_bool(m.layer);
            hash.write_u32(m.conductivity as u32);
            hash.write_u32(m.heat_capacity as u32);
        }
    }
}
