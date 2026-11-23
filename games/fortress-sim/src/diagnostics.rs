use sim_core::{
    jobs::JobState,
    space::{TileKind, standable},
    state::StateHash,
};
use sim_host::{SimError, SimMetrics, StateHasher};

use crate::{
    FortressSim, JobKind,
    types::{ItemKind, ItemLocation, ResKey, ResidentId, RouteStatus, Task},
};

impl FortressSim {
    #[allow(clippy::float_arithmetic)]
    pub(crate) fn collect_metrics(&self) -> SimMetrics {
        // Reporting converts integer totals to display gauges.
        let mut kinds = [0u64; 3];
        for kind in &self.icols.kind {
            kinds[*kind as usize] += 1;
        }
        let [open, claimed, done] = self.jobs.counts();
        let path = &self.diag.path;
        let ticks = self.diag.path_ticks.max(1) as f64;
        let per = |value: u64, count: u64| value as f64 / count.max(1) as f64;
        let c = &self.connectivity.stats;
        let custom_gauges = [
            ("residents", self.residents.len() as f64),
            ("items_stone", kinds[ItemKind::Stone as usize] as f64),
            ("items_ore", kinds[ItemKind::Ore as usize] as f64),
            ("items_food", kinds[ItemKind::Food as usize] as f64),
            ("jobs_open", open as f64),
            ("jobs_claimed", claimed as f64),
            ("jobs_done_pending", done as f64),
            ("jobs_completed", self.counters.completed_jobs as f64),
            ("dug_tiles", self.counters.dug as f64),
            ("built", self.counters.built as f64),
            ("hauled", self.counters.hauled as f64),
            ("eaten", self.counters.eaten as f64),
            ("caravans", self.counters.caravans as f64),
            ("deaths_hunger", self.counters.deaths[0] as f64),
            (
                "invalid_designations",
                self.counters.invalid_designations as f64,
            ),
            ("aborted_tasks", self.counters.aborted_tasks as f64),
            ("stockpile_tiles", self.stockpiles.len() as f64),
            (
                "lowest_level_reached",
                f64::from(self.diag.lowest_level.min(self.config.world_depth)),
            ),
            ("path_searches_per_tick", path.searches as f64 / ticks),
            (
                "path_expansions_per_search",
                per(path.expansions, path.searches),
            ),
            ("path_found", path.found as f64),
            ("path_unreachable", path.unreachable as f64),
            ("path_budget_exceeded", path.budget_exceeded as f64),
            ("path_skipped_unreachable", path.skipped as f64),
            ("path_avg_nodes", per(path.path_nodes, path.found)),
            (
                "path_queue_wait_ticks",
                per(path.wait_ticks, path.searches + path.skipped),
            ),
            ("path_queue_len", self.queue.len() as f64),
            ("route_failures", self.diag.route_failures as f64),
            (
                "unreachable_prechecks",
                self.diag.unreachable_prechecks as f64,
            ),
            ("blocked_steps", self.diag.blocked_steps as f64),
            ("build_waits", self.diag.build_waits as f64),
            ("hpa_fallbacks", self.diag.hpa_fallbacks as f64),
            ("changed_chunks_total", self.diag.changed_chunks as f64),
            ("connectivity_updates", c.updates as f64),
            ("connectivity_changed_tiles", c.changed_tiles as f64),
            ("connectivity_merges", c.merges as f64),
            ("connectivity_flood_fills", c.flood_fills as f64),
            ("connectivity_flood_tiles", c.flood_tiles as f64),
            (
                "connectivity_flood_tiles_per_fill",
                per(c.flood_tiles, c.flood_fills),
            ),
            ("connectivity_full_rebuilds", c.full_rebuilds as f64),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect();
        SimMetrics {
            active_entities: self.residents.len() + self.items.len(),
            custom_gauges,
        }
    }

    pub(crate) fn hashes(&self) -> [(&'static str, u64); 5] {
        let mut world = StateHasher::new();
        world.write_u32(self.world.width() as u32);
        world.write_u32(self.world.height() as u32);
        world.write_u32(self.world.depth() as u32);
        for (_, tile) in self.world.iter_logical() {
            tile.state_hash(&mut world);
        }
        self.stockpiles.state_hash(&mut world);

        let mut residents = StateHasher::new();
        self.residents.hash_state(&mut residents);
        residents.write_usize(self.residents.len());
        for id in self.residents.iter_by_id() {
            if let Some(slot) = self.residents.slot(id) {
                id.state_hash(&mut residents);
                self.rcols.hash_row(slot, &mut residents);
                for node in self.route_nodes(id) {
                    node.state_hash(&mut residents);
                }
            }
        }

        let mut items = StateHasher::new();
        self.items.hash_state(&mut items);
        items.write_usize(self.items.len());
        for id in self.items.iter_by_id() {
            if let Some(slot) = self.items.slot(id) {
                id.state_hash(&mut items);
                self.icols.hash_row(slot, &mut items);
            }
        }

        let mut jobs = StateHasher::new();
        jobs.write_u64(self.jobs.next_id());
        jobs.write_usize(self.jobs.len());
        for job in self.jobs.iter() {
            job.state_hash(&mut jobs);
        }
        jobs.write_usize(self.reservations.len());
        for entry in self.reservations.iter() {
            (entry.key, entry.holder, entry.since).state_hash(&mut jobs);
        }
        let mut pending = self.queue.snapshot();
        pending.sort_unstable_by_key(|r| (r.submitted, r.key));
        jobs.write_usize(pending.len());
        for r in pending {
            (r.key, r.start, r.goal, r.submitted).state_hash(&mut jobs);
        }

        let mut meta = StateHasher::new();
        meta.write_u64(self.tick);
        meta.write_u64(self.seed);
        self.config.hash_runtime(&mut meta);
        self.counters.state_hash(&mut meta);
        meta.write_u64(self.events.sequence());
        meta.write_usize(self.events.len());
        for (tick, seq, _) in self.events.sorted_events() {
            meta.write_u64(tick);
            meta.write_u64(seq);
            meta.write_u8(0);
        }
        [
            ("world", world.finish()),
            ("residents", residents.finish()),
            ("items", items.finish()),
            ("jobs", jobs.finish()),
            ("meta", meta.finish()),
        ]
    }

    pub(crate) fn hash_state(&self) -> u64 {
        let mut combined = StateHasher::new();
        combined.write_bytes(b"fortress-logical-v1");
        for (_, hash) in self.hashes() {
            combined.write_u64(hash);
        }
        combined.finish()
    }

    pub(crate) fn describe(&self, raw: u64) -> Option<Vec<(String, String)>> {
        let id = ResidentId::from_u64(raw);
        let slot = self.rslot(id)?;
        let p = self.rcols.position[slot];
        let task = self.rcols.task[slot];
        Some(
            [
                ("x", p.x.to_string()),
                ("y", p.y.to_string()),
                ("z", p.z.to_string()),
                ("hunger", self.rcols.needs[slot].hunger.to_string()),
                ("fatigue", self.rcols.needs[slot].fatigue.to_string()),
                ("action", task.name().into()),
                (
                    "job",
                    task.job().map_or_else(|| "none".into(), |j| j.to_string()),
                ),
                (
                    "carried",
                    self.rcols.carried[slot]
                        .map_or_else(|| "none".into(), |i| i.to_u64().to_string()),
                ),
                ("route", format!("{:?}", self.rcols.route[slot])),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
        )
    }

    fn task_holds(&self, key: ResKey, holder: ResidentId) -> bool {
        self.rslot(holder)
            .is_some_and(|slot| self.rcols.task[slot].holds(key))
    }

    pub(crate) fn validate_state(&self) -> Result<(), SimError> {
        let fail = |message: &str| Err(SimError::InvariantViolation(message.into()));
        if self.rcols.len() != self.residents.len() || self.icols.len() != self.items.len() {
            return fail("entity tables and columns differ in length");
        }
        let cap = self.config.max_path_nodes as usize;
        for id in self.residents.iter_by_id() {
            let Some(slot) = self.rslot(id) else {
                return fail("live resident without slot");
            };
            let p = self.rcols.position[slot];
            if !self.world.get(p).is_some_and(|t| standable(*t)) {
                return fail("resident does not stand on a standable tile");
            }
            if let Some(item) = self.rcols.carried[slot] {
                let carried = self
                    .islot(item)
                    .is_some_and(|s| self.icols.location[s] == ItemLocation::Carried(id));
                if !carried {
                    return fail("carried item does not name its carrier");
                }
            }
            let route = self.rcols.route[slot];
            if usize::from(route.len) > cap || route.next > route.len.max(1) {
                return fail("route exceeds its storage");
            }
            if route.status == RouteStatus::Waiting && !self.queue.contains(id) {
                return fail("waiting resident has no path request");
            }
            if let Some(job) = self.rcols.task[slot].job() {
                let claimed = self.jobs.get(job).is_some_and(|j| {
                    matches!(j.state, JobState::Claimed { by, .. } if by == id)
                        || j.state == JobState::Done
                });
                if !claimed {
                    return fail("task references a job it does not hold");
                }
            }
        }
        for request in self.queue.snapshot() {
            let waiting = self
                .rslot(request.key)
                .is_some_and(|slot| self.rcols.route[slot].status == RouteStatus::Waiting);
            if !waiting {
                return fail("path request without waiting resident");
            }
        }
        let mut stock = vec![0u32; self.stockpiles.len()];
        for id in self.items.iter_by_id() {
            let Some(slot) = self.islot(id) else {
                return fail("live item without slot");
            };
            match self.icols.location[slot] {
                ItemLocation::Carried(holder) => {
                    let ok = self
                        .rslot(holder)
                        .is_some_and(|s| self.rcols.carried[s] == Some(id));
                    if !ok {
                        return fail("carried item has no living carrier holding it");
                    }
                }
                ItemLocation::Ground(p) => {
                    if self.world.get(p).is_none_or(|t| t.kind == TileKind::Solid) {
                        return fail("item lies in a solid tile or outside the world");
                    }
                    if let Ok(index) = self.stockpiles.binary_search(&p) {
                        stock[index] += 1;
                    }
                }
            }
            if let Some(job) = self.icols.haul[slot]
                && !self
                    .jobs
                    .get(job)
                    .is_some_and(|j| j.data == JobKind::Haul(id))
            {
                return fail("item references a missing haul job");
            }
        }
        if stock.iter().any(|&count| count > 1) {
            return fail("stockpile tile holds more than one item");
        }
        if self.stockpiles.windows(2).any(|pair| pair[0] >= pair[1]) {
            return fail("stockpile tiles are not sorted and unique");
        }
        self.reservations
            .check(
                |holder| self.residents.is_alive(holder),
                |key, holder| self.task_holds(key, holder),
            )
            .or_else(&fail)?;
        for job in self.jobs.iter() {
            let Some(&tile) = self.world.get(job.pos) else {
                return fail("job outside the world");
            };
            let valid = match (job.state, job.data) {
                (JobState::Done, _) => true,
                (_, JobKind::Dig(_)) => tile.kind == TileKind::Solid,
                (JobState::Open, JobKind::Build(kind)) => crate::build_target_valid(tile, kind),
                (_, JobKind::Build(_)) => true,
                (_, JobKind::Haul(item)) => self
                    .islot(item)
                    .is_some_and(|s| self.icols.haul[s] == Some(job.id)),
            };
            if !valid {
                return fail("job references an invalid location or item");
            }
            if let JobState::Claimed { by, .. } = job.state
                && !self
                    .rslot(by)
                    .is_some_and(|s| self.rcols.task[s].job() == Some(job.id))
            {
                return fail("claimed job without matching task");
            }
        }
        for task in &self.rcols.task {
            if let Task::Eat { item } | Task::Haul { item, .. } | Task::Build { item, .. } = *task
                && self.islot(item).is_none()
            {
                return fail("task references a missing item");
            }
        }
        Ok(())
    }
}
