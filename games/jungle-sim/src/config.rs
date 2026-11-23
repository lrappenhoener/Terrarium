use serde::{Deserialize, Serialize};
use sim_host::{SimError, StateHasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Species {
    Herbivore,
    Predator,
}

impl Species {
    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeciesConfig {
    pub move_cooldown: u64,
    pub perception_radius: u32,
    pub hunger_rate: u16,
    pub thirst_rate: u16,
    pub max_age: u64,
    pub reproduction_threshold: u16,
    pub gestation_ticks: u64,
    pub litter_size: u8,
    pub attack_strength: u16,
}

impl Default for SpeciesConfig {
    fn default() -> Self {
        Self {
            move_cooldown: 3,
            perception_radius: 4,
            hunger_rate: 1,
            thirst_rate: 1,
            max_age: 24_000,
            reproduction_threshold: 80,
            gestation_ticks: 120,
            litter_size: 1,
            attack_strength: 0,
        }
    }
}

fn predator_defaults() -> SpeciesConfig {
    SpeciesConfig {
        move_cooldown: 2,
        perception_radius: 7,
        max_age: 24_000,
        reproduction_threshold: 50,
        gestation_ticks: 160,
        attack_strength: 120,
        ..SpeciesConfig::default()
    }
}

fn deserialize_predator<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SpeciesConfig, D::Error> {
    #[derive(Default, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Overrides {
        move_cooldown: Option<u64>,
        perception_radius: Option<u32>,
        hunger_rate: Option<u16>,
        thirst_rate: Option<u16>,
        max_age: Option<u64>,
        reproduction_threshold: Option<u16>,
        gestation_ticks: Option<u64>,
        litter_size: Option<u8>,
        attack_strength: Option<u16>,
    }
    let o = Overrides::deserialize(deserializer)?;
    let defaults = predator_defaults();
    Ok(SpeciesConfig {
        move_cooldown: o.move_cooldown.unwrap_or(defaults.move_cooldown),
        perception_radius: o.perception_radius.unwrap_or(defaults.perception_radius),
        hunger_rate: o.hunger_rate.unwrap_or(defaults.hunger_rate),
        thirst_rate: o.thirst_rate.unwrap_or(defaults.thirst_rate),
        max_age: o.max_age.unwrap_or(defaults.max_age),
        reproduction_threshold: o
            .reproduction_threshold
            .unwrap_or(defaults.reproduction_threshold),
        gestation_ticks: o.gestation_ticks.unwrap_or(defaults.gestation_ticks),
        litter_size: o.litter_size.unwrap_or(defaults.litter_size),
        attack_strength: o.attack_strength.unwrap_or(defaults.attack_strength),
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JungleConfig {
    pub world_size: i32,
    pub initial_herbivores: u32,
    pub initial_predators: u32,
    pub max_animals: u32,
    pub think_interval: u64,
    pub reorder_interval: u64,
    pub debug_shuffle_slots: bool,
    pub plant_regrowth_per_mille: u16,
    pub plant_update_stripes: usize,
    pub herbivore: SpeciesConfig,
    #[serde(deserialize_with = "deserialize_predator")]
    pub predator: SpeciesConfig,
}

impl Default for JungleConfig {
    fn default() -> Self {
        Self {
            world_size: 512,
            initial_herbivores: 10_000,
            initial_predators: 500,
            max_animals: 200_000,
            think_interval: 4,
            reorder_interval: 64,
            debug_shuffle_slots: false,
            plant_regrowth_per_mille: 30,
            plant_update_stripes: 32,
            herbivore: SpeciesConfig::default(),
            predator: predator_defaults(),
        }
    }
}

impl JungleConfig {
    pub(crate) fn species(&self, species: Species) -> SpeciesConfig {
        match species {
            Species::Herbivore => self.herbivore,
            Species::Predator => self.predator,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SimError> {
        let fail = |message: &str| SimError::InvalidConfig(message.into());
        if !(4..=4096).contains(&self.world_size)
            || !(1..=1_000_000).contains(&self.max_animals)
            || u64::from(self.initial_herbivores) + u64::from(self.initial_predators)
                > u64::from(self.max_animals)
        {
            return Err(fail("invalid world size or animal counts"));
        }
        if self.think_interval == 0
            || !(1..=4096).contains(&self.plant_update_stripes)
            || self.plant_regrowth_per_mille > 1000
        {
            return Err(fail("invalid update intervals or regrowth rate"));
        }
        for species in [self.herbivore, self.predator] {
            if species.move_cooldown == 0
                || !(1..=64).contains(&species.perception_radius)
                || species.hunger_rate == 0
                || species.thirst_rate == 0
                || species.max_age == 0
                || species.gestation_ticks == 0
                || !(1..=16).contains(&species.litter_size)
                || species.reproduction_threshold > 1000
            {
                return Err(fail("invalid species parameters"));
            }
        }
        if self.predator.attack_strength == 0 || self.herbivore.attack_strength != 0 {
            return Err(fail("only predators may have positive attack strength"));
        }
        Ok(())
    }

    pub(crate) fn hash_runtime(&self, hash: &mut StateHasher) {
        hash.write_u32(self.world_size as u32);
        hash.write_u32(self.max_animals);
        hash.write_u64(self.think_interval);
        hash.write_usize(self.plant_update_stripes);
        for c in [self.herbivore, self.predator] {
            for value in [
                c.move_cooldown,
                u64::from(c.perception_radius),
                u64::from(c.hunger_rate),
                u64::from(c.thirst_rate),
                c.max_age,
                u64::from(c.reproduction_threshold),
                c.gestation_ticks,
                u64::from(c.litter_size),
                u64::from(c.attack_strength),
            ] {
                hash.write_u64(value);
            }
        }
    }
}
