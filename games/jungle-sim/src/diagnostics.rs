use sim_core::entity::EntityId;
use sim_host::{SimError, SimMetrics, StateHasher};

use crate::{BIOMASS_CAP, Event, HEALTH, JungleSim, NEED_LIMIT, Species};

impl JungleSim {
    #[allow(clippy::float_arithmetic)]
    pub(crate) fn collect_metrics(&self) -> SimMetrics {
        // Reporting converts integer totals to display gauges.
        let hunger = self
            .allocator
            .iter()
            .filter_map(|id| self.vitals.get(id))
            .map(|vitals| u64::from(vitals.hunger))
            .sum::<u64>();
        let biomass = self
            .world
            .as_slice()
            .iter()
            .map(|tile| u64::from(tile.biomass))
            .sum::<u64>();
        let active_entities = self.allocator.len();
        let custom_gauges = [
            ("herbivores", self.population[0] as f64),
            ("predators", self.population[1] as f64),
            ("births_total", self.births as f64),
            ("deaths_predation", self.deaths[0] as f64),
            ("deaths_hunger", self.deaths[1] as f64),
            ("deaths_thirst", self.deaths[2] as f64),
            ("deaths_age", self.deaths[3] as f64),
            ("total_biomass", biomass as f64),
            ("avg_hunger", hunger as f64 / active_entities.max(1) as f64),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect();
        SimMetrics {
            active_entities,
            custom_gauges,
        }
    }

    pub(crate) fn hashes(&self) -> [(&'static str, u64); 4] {
        let mut world = StateHasher::new();
        world.write_usize(self.world.as_slice().len());
        for tile in self.world.as_slice() {
            world.write_u8(tile.terrain as u8);
            world.write_u32(u32::from(tile.biomass));
            world.write_u32(u32::from(tile.water_distance));
        }

        let mut animals = StateHasher::new();
        self.allocator.hash_state(&mut animals);
        for id in self.allocator.iter() {
            if let (Some(a), Some(p)) = (self.vitals.get(id), self.position.get(id)) {
                animals.write_u64(id.to_u64());
                animals.write_u32(p.x as u32);
                animals.write_u32(p.y as u32);
                animals.write_u8(a.species as u8);
                for value in [a.hunger, a.thirst, a.health] {
                    animals.write_u32(u32::from(value));
                }
                animals.write_u64(a.age);
                animals.write_u64(a.move_at);
                animals.write_u8(a.action as u8);
                for reference in [a.herd, self.hunting.get(id).copied()] {
                    animals.write_bool(reference.is_some());
                    if let Some(reference) = reference {
                        animals.write_u64(reference.to_u64());
                    }
                }
                let pregnancy = self.pregnant.get(id);
                animals.write_bool(pregnancy.is_some());
                if let Some(&deadline) = pregnancy {
                    animals.write_u64(deadline);
                }
            }
        }

        let mut scheduler = StateHasher::new();
        scheduler.write_u64(self.events.sequence());
        scheduler.write_usize(self.events.len());
        for (tick, seq, event) in self.events.sorted_events() {
            scheduler.write_u64(tick);
            scheduler.write_u64(seq);
            match *event {
                Event::Birth(id) => {
                    scheduler.write_u8(0);
                    scheduler.write_u64(id.to_u64());
                }
                Event::DroughtEnd => scheduler.write_u8(1),
            }
        }

        let mut meta = StateHasher::new();
        meta.write_u64(self.tick);
        meta.write_u64(self.seed);
        self.config.hash_runtime(&mut meta);
        meta.write_u128(self.spawn_rng.word_pos());
        meta.write_u32(u32::from(self.growth));
        meta.write_bool(self.drought_until.is_some());
        if let Some(deadline) = self.drought_until {
            meta.write_u64(deadline);
        }
        meta.write_u64(self.births);
        for count in self.deaths {
            meta.write_u64(count);
        }
        for count in self.population {
            meta.write_usize(count);
        }
        [
            ("world", world.finish()),
            ("animals", animals.finish()),
            ("scheduler", scheduler.finish()),
            ("meta", meta.finish()),
        ]
    }

    pub(crate) fn hash_state(&self) -> u64 {
        let mut combined = StateHasher::new();
        for (_, hash) in self.hashes() {
            combined.write_u64(hash);
        }
        combined.finish()
    }

    pub(crate) fn describe(&self, id: EntityId) -> Option<Vec<(String, String)>> {
        if !self.allocator.is_alive(id) {
            return None;
        }
        let point = self.position.get(id)?;
        let a = self.vitals.get(id)?;
        let reference = |id: Option<EntityId>| {
            id.filter(|&id| self.allocator.is_alive(id))
                .map_or_else(|| "none".into(), |id| id.to_u64().to_string())
        };
        Some(
            [
                ("x", point.x.to_string()),
                ("y", point.y.to_string()),
                ("species", format!("{:?}", a.species)),
                ("hunger", a.hunger.to_string()),
                ("thirst", a.thirst.to_string()),
                ("health", a.health.to_string()),
                ("age", a.age.to_string()),
                ("move_at", a.move_at.to_string()),
                ("action", a.action.name().into()),
                ("herd", reference(a.herd)),
                ("target", reference(self.hunting.get(id).copied())),
                (
                    "birth_due",
                    self.pregnant
                        .get(id)
                        .map_or_else(|| "none".into(), u64::to_string),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
        )
    }

    pub(crate) fn validate_state(&self) -> Result<(), SimError> {
        let fail = |message: &str| SimError::InvariantViolation(message.into());
        if self.spatial.len() != self.allocator.len()
            || self.position.iter().count() != self.allocator.len()
            || self.vitals.iter().count() != self.allocator.len()
            || self.occupied.as_slice().iter().flatten().count() != self.allocator.len()
        {
            return Err(fail("allocator, occupancy, and component counts differ"));
        }
        let mut population = [0; 2];
        let mut hits = Vec::with_capacity(1);
        for id in self.allocator.iter() {
            let &point = self
                .position
                .get(id)
                .ok_or_else(|| fail("missing position"))?;
            let a = self
                .vitals
                .get(id)
                .ok_or_else(|| fail("missing animal needs"))?;
            if !self.world.get(point).is_some_and(|t| t.walkable())
                || self.occupied.get(point) != Some(&Some(id))
                || a.hunger >= NEED_LIMIT
                || a.thirst >= NEED_LIMIT
                || !(1..=HEALTH).contains(&a.health)
            {
                return Err(fail("invalid position, occupancy, or needs"));
            }
            if a.herd.is_some_and(|herd| {
                self.allocator.is_alive(herd)
                    && self
                        .vitals
                        .get(herd)
                        .is_none_or(|member| member.species != Species::Herbivore)
            }) {
                return Err(fail("valid herd ID must name a herbivore"));
            }
            self.spatial.query_radius(point, 0, &mut hits);
            if hits.as_slice() != [id] {
                return Err(fail("spatial index differs from live positions"));
            }
            population[a.species.index()] += 1;
        }
        if population != self.population
            || self
                .world
                .as_slice()
                .iter()
                .any(|t| t.biomass > BIOMASS_CAP || (!t.walkable() && t.biomass != 0))
            || self
                .hunting
                .iter()
                .map(|(id, _)| id)
                .chain(self.pregnant.iter().map(|(id, _)| id))
                .any(|id| !self.allocator.is_alive(id))
        {
            return Err(fail(
                "invalid population, biomass, or sparse component owner",
            ));
        }
        Ok(())
    }
}
