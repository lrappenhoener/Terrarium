//! Economy diagnostics: job latencies, index statistics and production gauges.
//! Nothing here is hashed, saved or read by the simulation.

use std::cell::Cell;

use sim_core::jobs::JobId;

use crate::{FortressSim, types::JobKind};

/// Latency samples kept per job kind (a ring of the most recent ones).
const SAMPLES: usize = 4096;
/// Ticks at which job IDs first appeared; bounded, diagnostics stop when full.
const EPOCHS: usize = 1 << 17;

pub(crate) const LATENCY_KINDS: [&str; 8] = [
    "dig",
    "build",
    "haul",
    "fell",
    "sow",
    "harvest",
    "construct",
    "craft",
];

pub(crate) struct Economy {
    /// `(first job ID, tick)` whenever new job IDs appeared in a tick.
    epochs: Vec<(u64, u64)>,
    seen: u64,
    latency: Vec<Vec<u32>>,
    next: [usize; LATENCY_KINDS.len()],
    pub completed: [u64; LATENCY_KINDS.len()],
    pub index_queries: Cell<u64>,
    pub index_cells: Cell<u64>,
    pub index_candidates: Cell<u64>,
}

impl Economy {
    pub fn new() -> Self {
        Self {
            epochs: Vec::with_capacity(EPOCHS),
            seen: 0,
            latency: (0..LATENCY_KINDS.len())
                .map(|_| Vec::with_capacity(SAMPLES))
                .collect(),
            next: [0; LATENCY_KINDS.len()],
            completed: [0; LATENCY_KINDS.len()],
            index_queries: Cell::new(0),
            index_cells: Cell::new(0),
            index_candidates: Cell::new(0),
        }
    }

    /// Notes job IDs created during the tick that just ended.
    pub fn note_jobs(&mut self, next_id: u64, tick: u64) {
        if next_id > self.seen {
            if self.epochs.len() < self.epochs.capacity() {
                self.epochs.push((self.seen, tick));
            }
            self.seen = next_id;
        }
    }

    fn created(&self, job: JobId) -> Option<u64> {
        let at = self.epochs.partition_point(|&(first, _)| first <= job);
        at.checked_sub(1).map(|i| self.epochs[i].1)
    }

    pub fn record(&mut self, kind: usize, job: JobId, tick: u64) {
        let Some(created) = self.created(job) else {
            return;
        };
        let latency = u32::try_from(tick.saturating_sub(created)).unwrap_or(u32::MAX);
        let samples = &mut self.latency[kind];
        if samples.len() < SAMPLES {
            samples.push(latency);
        } else {
            samples[self.next[kind]] = latency;
            self.next[kind] = (self.next[kind] + 1) % SAMPLES;
        }
        self.completed[kind] += 1;
    }

    /// Nearest-rank p50 and p95 of the recent latencies of a kind.
    pub fn percentiles(&self, kind: usize) -> Option<(u32, u32)> {
        let mut sorted = self.latency[kind].clone();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_unstable();
        let rank = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)];
        Some((rank(50), rank(95)))
    }
}

impl FortressSim {
    /// Completes a job and records its latency by kind.
    pub(crate) fn complete_job(&mut self, job: JobId) {
        let kind = match self.jobs.get(job).map(|j| j.data) {
            Some(JobKind::Dig(_)) => 0,
            Some(JobKind::Build(_)) => 1,
            Some(JobKind::Haul(_)) => 2,
            Some(JobKind::Fell(_)) => 3,
            Some(JobKind::Sow) => 4,
            Some(JobKind::Harvest(_)) => 5,
            Some(JobKind::Craft { recipe: None, .. }) => 6,
            Some(JobKind::Craft { .. }) => 7,
            None => return,
        };
        self.economy.record(kind, job, self.tick);
        self.jobs.complete(job);
    }

    pub(crate) fn note_index_query(&self, cells: u32, candidates: u32) {
        let e = &self.economy;
        e.index_queries.set(e.index_queries.get() + 1);
        e.index_cells.set(e.index_cells.get() + u64::from(cells));
        e.index_candidates
            .set(e.index_candidates.get() + u64::from(candidates));
    }

    /// Gauges of the economy for `metrics()`; allocates outside the hot path.
    #[allow(clippy::float_arithmetic)]
    pub(crate) fn economy_gauges(&self, out: &mut Vec<(String, f64)>) {
        // Reporting converts integer totals to display gauges.
        let per_thousand = |value: u64| value as f64 * 1000.0 / self.tick.max(1) as f64;
        let categories = self.content.categories.len();
        let mut produced = vec![0u64; categories];
        let mut removed = vec![0u64; categories];
        for (kind, ledger) in self.ledger.iter().enumerate() {
            let c = usize::from(self.content.category_of(kind as u16));
            produced[c] += ledger.produced;
            removed[c] += ledger.removed;
        }
        let mut stock = vec![0u64; categories];
        let mut units = vec![0u64; categories];
        for slot in 0..self.icols.len() {
            let c = usize::from(self.content.category_of(self.icols.kind[slot].0));
            let quantity = u64::from(self.icols.quantity[slot]);
            units[c] += quantity;
            let stored = self
                .items
                .id(slot as u32)
                .and_then(|id| self.item_ground(id))
                .is_some_and(|p| self.is_stockpile(p));
            if stored {
                stock[c] += quantity;
            }
        }
        for c in 0..categories {
            let name = &self.content.ids.categories[c];
            out.push((
                format!("produced_per_1000_{name}"),
                per_thousand(produced[c]),
            ));
            out.push((
                format!("consumed_per_1000_{name}"),
                per_thousand(removed[c]),
            ));
            out.push((format!("stock_{name}"), stock[c] as f64));
            out.push((format!("units_{name}"), units[c] as f64));
        }
        let idle = (0..self.rcols.len())
            .filter(|&s| self.rcols.task[s] == crate::types::Task::Idle)
            .count();
        out.push((
            "idle_share".into(),
            idle as f64 / self.rcols.len().max(1) as f64,
        ));
        for (k, name) in LATENCY_KINDS.iter().enumerate() {
            if let Some((p50, p95)) = self.economy.percentiles(k) {
                out.push((format!("latency_p50_{name}"), f64::from(p50)));
                out.push((format!("latency_p95_{name}"), f64::from(p95)));
            }
            out.push((
                format!("jobs_completed_{name}"),
                self.economy.completed[k] as f64,
            ));
        }
        let hauls = self.diag.haul_count.max(1);
        out.push((
            "haul_distance_avg".into(),
            self.diag.haul_distance as f64 / hauls as f64,
        ));
        let open_hauls = self
            .jobs
            .iter()
            .filter(|j| {
                matches!(j.data, JobKind::Haul(_)) && j.state != sim_core::jobs::JobState::Done
            })
            .count();
        out.push(("hauls_open".into(), open_hauls as f64));
        out.push(("hauls_deferred".into(), self.diag.hauls_deferred as f64));
        out.push(("bulk_hauls".into(), self.diag.bulk_hauls as f64));
        let e = &self.economy;
        let queries = e.index_queries.get().max(1);
        out.push(("index_queries".into(), e.index_queries.get() as f64));
        out.push((
            "index_cells_per_query".into(),
            e.index_cells.get() as f64 / queries as f64,
        ));
        out.push((
            "index_candidates_per_query".into(),
            e.index_candidates.get() as f64 / queries as f64,
        ));
        out.push(("workshops".into(), self.workshops.len() as f64));
        out.push(("work_orders".into(), self.orders.len() as f64));
        out.push(("orders_waiting".into(), self.diag.orders_waiting as f64));
        out.push(("plants".into(), self.plants.len() as f64));
        out.push(("farm_plots".into(), self.farms.len() as f64));
        out.push(("season".into(), self.season() as f64));
        out.push(("crafted".into(), self.counters.crafted as f64));
        out.push(("felled".into(), self.counters.felled as f64));
        out.push(("harvested".into(), self.counters.harvested as f64));
        out.push(("sown".into(), self.counters.sown as f64));
    }
}
