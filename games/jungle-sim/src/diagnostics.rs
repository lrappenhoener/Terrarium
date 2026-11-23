use sim_core::{entity::EntityId, space::SpatialIndex};
use sim_host::{SimError, SimMetrics, StateHasher};

use crate::{BIOMASS_CAP, Event, HEALTH, JungleSim, NEED_LIMIT, Species, spatial_entries};

impl JungleSim {
    #[allow(clippy::float_arithmetic)]
    pub(crate) fn collect_metrics(&self) -> SimMetrics {
        // Reporting converts integer totals to display gauges.
        let hunger = self
            .allocator
            .iter()
            .filter_map(|id| self.vitals(id))
            .map(|vitals| u64::from(vitals.hunger))
            .sum::<u64>();
        let biomass = self
            .world
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
            (
                "radius_candidates_per_query",
                self.query_counts[1] as f64 / self.query_counts[0].max(1) as f64,
            ),
            (
                "radius_queries_per_tick",
                self.query_counts[0] as f64 / self.tick.max(1) as f64,
            ),
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
        world.write_u32(self.config.world_size as u32);
        for tile in self.world.iter() {
            world.write_u8(tile.terrain as u8);
            world.write_u32(u32::from(tile.biomass));
            world.write_u32(u32::from(tile.water_distance));
        }

        let mut animals = StateHasher::new();
        self.allocator.hash_state(&mut animals);
        animals.write_usize(self.allocator.len());
        for id in self.allocator.iter_by_id() {
            if let (Some(a), Some(p)) = (self.vitals(id), self.position(id)) {
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
                animals.write_bool(a.herd.is_some());
                if let Some(herd) = a.herd {
                    animals.write_u64(herd.to_u64());
                }
            }
        }
        let mut hunting: Vec<_> = self.hunting.iter().collect();
        hunting.sort_unstable_by_key(|&(id, _)| id);
        animals.write_usize(hunting.len());
        for (id, target) in hunting {
            animals.write_u64(id.to_u64());
            animals.write_u64(target.to_u64());
        }
        let mut pregnant: Vec<_> = self.pregnant.iter().collect();
        pregnant.sort_unstable_by_key(|&(id, _)| id);
        animals.write_usize(pregnant.len());
        for (id, &deadline) in pregnant {
            animals.write_u64(id.to_u64());
            animals.write_u64(deadline);
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
        combined.write_bytes(b"jungle-logical-v1");
        for (_, hash) in self.hashes() {
            combined.write_u64(hash);
        }
        combined.finish()
    }

    pub(crate) fn describe(&self, id: EntityId) -> Option<Vec<(String, String)>> {
        if !self.allocator.is_alive(id) {
            return None;
        }
        let point = self.position(id)?;
        let a = self.vitals(id)?;
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
        let size = self.config.world_size;
        let mut spatial = SpatialIndex::new(size, size, 8, 2, self.allocator.len())
            .ok_or_else(|| fail("cannot construct invariant index"))?;
        spatial.rebuild(spatial_entries(&self.allocator, &self.columns));
        if spatial.len() != self.allocator.len()
            || self.columns.position.len() != self.allocator.len()
            || self.columns.vitals.len() != self.allocator.len()
            || self.occupied.iter().flatten().count() != self.allocator.len()
        {
            return Err(fail("allocator, occupancy, and component counts differ"));
        }
        let mut population = [0; 2];
        let mut hits = Vec::with_capacity(1);
        for id in self.allocator.iter() {
            let &point = self.position(id).ok_or_else(|| fail("missing position"))?;
            let a = self
                .vitals(id)
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
                        .vitals(herd)
                        .is_none_or(|member| member.species != Species::Herbivore)
            }) {
                return Err(fail("valid herd ID must name a herbivore"));
            }
            spatial.query_radius(point, 0, 1 << a.species.index(), &mut hits);
            if hits.len() != 1 || hits[0].id != id || Some(hits[0].slot) != self.allocator.slot(id)
            {
                return Err(fail("spatial index differs from live positions"));
            }
            population[a.species.index()] += 1;
        }
        if population != self.population
            || self
                .world
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
