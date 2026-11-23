//! Domain-neutral jobs, reservations and deterministic assignment.
//!
//! All tables keep capacity reserved at construction and iterate in key order.
//! Spatial job indices are derived data and rebuilt from the board.

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::{
    space::Pos3,
    state::{StateHash, StateHasher},
    topology::layered_heuristic,
};

/// Stable, monotonically assigned job identifier.
pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState<H> {
    Open,
    Claimed { by: H, since_tick: u64 },
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job<J, H> {
    pub id: JobId,
    /// Larger values are more urgent.
    pub priority: i32,
    pub pos: Pos3,
    pub state: JobState<H>,
    pub data: J,
}

impl<J: StateHash, H: StateHash> StateHash for Job<J, H> {
    fn state_hash(&self, hash: &mut StateHasher) {
        self.id.state_hash(hash);
        self.priority.state_hash(hash);
        self.pos.state_hash(hash);
        match &self.state {
            JobState::Open => hash.write_u8(0),
            JobState::Claimed { by, since_tick } => {
                hash.write_u8(1);
                by.state_hash(hash);
                since_tick.state_hash(hash);
            }
            JobState::Done => hash.write_u8(2),
        }
        self.data.state_hash(hash);
    }
}

/// Distance used for job proximity: the admissible layered walking estimate.
pub fn job_distance(a: Pos3, b: Pos3) -> u32 {
    layered_heuristic(a.x.abs_diff(b.x), a.y.abs_diff(b.y), a.z.abs_diff(b.z))
}

const CELL_XY: i32 = 8;
const CELL_Z: i32 = 2;

/// Counting-sort index of open jobs over coarse 3D cells.
struct JobIndex {
    columns: i32,
    rows: i32,
    layers: i32,
    offsets: Vec<u32>,
    cursors: Vec<u32>,
    entries: Vec<u32>,
    marks: Vec<bool>,
}

impl JobIndex {
    fn new(width: i32, height: i32, depth: i32, capacity: usize) -> Self {
        let columns = (width.max(1) - 1) / CELL_XY + 1;
        let rows = (height.max(1) - 1) / CELL_XY + 1;
        let layers = (depth.max(1) - 1) / CELL_Z + 1;
        let cells = (columns * rows * layers) as usize;
        Self {
            columns,
            rows,
            layers,
            offsets: vec![0; cells + 1],
            cursors: vec![0; cells],
            entries: Vec::with_capacity(capacity),
            marks: Vec::with_capacity(capacity),
        }
    }

    fn cell_coords(&self, p: Pos3) -> (i32, i32, i32) {
        (
            (p.x / CELL_XY).clamp(0, self.columns - 1),
            (p.y / CELL_XY).clamp(0, self.rows - 1),
            (p.z / CELL_Z).clamp(0, self.layers - 1),
        )
    }

    fn cell(&self, (x, y, z): (i32, i32, i32)) -> usize {
        ((z * self.rows + y) * self.columns + x) as usize
    }
}

/// Jobs in ascending ID order. IDs are never reused.
pub struct JobBoard<J, H> {
    jobs: Vec<Job<J, H>>,
    next_id: JobId,
    capacity: usize,
    index: JobIndex,
}

impl<J, H: Copy + Eq> JobBoard<J, H> {
    pub fn new(capacity: usize, width: i32, height: i32, depth: i32) -> Self {
        Self {
            jobs: Vec::with_capacity(capacity),
            next_id: 0,
            capacity,
            index: JobIndex::new(width, height, depth, capacity),
        }
    }

    /// Restores jobs with ascending unique IDs below `next_id`.
    pub fn restore(
        capacity: usize,
        width: i32,
        height: i32,
        depth: i32,
        next_id: JobId,
        jobs: Vec<Job<J, H>>,
    ) -> Option<Self> {
        if jobs.len() > capacity
            || jobs.windows(2).any(|pair| pair[0].id >= pair[1].id)
            || jobs.last().is_some_and(|job| job.id >= next_id)
        {
            return None;
        }
        let mut board = Self::new(capacity, width, height, depth);
        board.jobs.extend(jobs);
        board.next_id = next_id;
        Some(board)
    }

    pub fn next_id(&self) -> JobId {
        self.next_id
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.jobs.len() >= self.capacity
    }

    /// Adds an open job. Fails when the board is full or IDs are exhausted.
    pub fn add(&mut self, priority: i32, pos: Pos3, data: J) -> Option<JobId> {
        if self.is_full() {
            return None;
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1)?;
        self.jobs.push(Job {
            id,
            priority,
            pos,
            state: JobState::Open,
            data,
        });
        Some(id)
    }

    fn position(&self, id: JobId) -> Option<usize> {
        self.jobs.binary_search_by_key(&id, |job| job.id).ok()
    }

    pub fn get(&self, id: JobId) -> Option<&Job<J, H>> {
        self.jobs.get(self.position(id)?)
    }

    pub fn get_mut(&mut self, id: JobId) -> Option<&mut Job<J, H>> {
        let index = self.position(id)?;
        self.jobs.get_mut(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Job<J, H>> {
        self.jobs.iter()
    }

    pub fn claim(&mut self, id: JobId, by: H, tick: u64) -> bool {
        match self.get_mut(id) {
            Some(job) if job.state == JobState::Open => {
                job.state = JobState::Claimed {
                    by,
                    since_tick: tick,
                };
                true
            }
            _ => false,
        }
    }

    /// Returns a claimed job to the open state.
    pub fn release(&mut self, id: JobId) -> bool {
        match self.get_mut(id) {
            Some(job) if matches!(job.state, JobState::Claimed { .. }) => {
                job.state = JobState::Open;
                true
            }
            _ => false,
        }
    }

    pub fn complete(&mut self, id: JobId) -> bool {
        match self.get_mut(id) {
            Some(job) if job.state != JobState::Done => {
                job.state = JobState::Done;
                true
            }
            _ => false,
        }
    }

    pub fn remove(&mut self, id: JobId) -> Option<Job<J, H>> {
        let index = self.position(id)?;
        Some(self.jobs.remove(index))
    }

    /// Removes all jobs matching `remove`, preserving ID order.
    pub fn retain(&mut self, mut keep: impl FnMut(&Job<J, H>) -> bool) -> usize {
        let before = self.jobs.len();
        self.jobs.retain(|job| keep(job));
        before - self.jobs.len()
    }

    /// Open, claimed and done counts.
    pub fn counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for job in &self.jobs {
            counts[match job.state {
                JobState::Open => 0,
                JobState::Claimed { .. } => 1,
                JobState::Done => 2,
            }] += 1;
        }
        counts
    }

    /// Rebuilds the spatial index of open jobs that pass `indexable`, for example
    /// jobs with an accessible work site. Entries within a cell keep ID order.
    pub fn rebuild_index(&mut self, mut indexable: impl FnMut(&Job<J, H>) -> bool) {
        let index = &mut self.index;
        index.offsets.fill(0);
        let mut marks = std::mem::take(&mut index.marks);
        marks.clear();
        for job in &self.jobs {
            let include = job.state == JobState::Open && indexable(job);
            marks.push(include);
            if include {
                let cell = index.cell(index.cell_coords(job.pos));
                index.offsets[cell + 1] += 1;
            }
        }
        for i in 1..index.offsets.len() {
            index.offsets[i] += index.offsets[i - 1];
        }
        let cells = index.cursors.len();
        index.cursors.copy_from_slice(&index.offsets[..cells]);
        index.entries.clear();
        index.entries.resize(index.offsets[cells] as usize, 0);
        for (position, job) in self.jobs.iter().enumerate() {
            if marks[position] {
                let cell = index.cell(index.cell_coords(job.pos));
                index.entries[index.cursors[cell] as usize] = position as u32;
                index.cursors[cell] += 1;
            }
        }
        index.marks = marks;
    }

    /// The `k` nearest open jobs from the last index rebuild that pass `accept`,
    /// sorted by `(distance, id)`. At most `inspect` candidates are tested, in a
    /// fixed ring order, so the cost stays bounded when many jobs are rejected.
    pub fn nearest_open(
        &self,
        from: Pos3,
        k: usize,
        inspect: usize,
        mut accept: impl FnMut(&Job<J, H>) -> bool,
        out: &mut Vec<(u32, JobId)>,
    ) {
        out.clear();
        if k == 0 {
            return;
        }
        let index = &self.index;
        let mut inspected = 0;
        let mut consider = |position: u32, out: &mut Vec<(u32, JobId)>, inspected: &mut usize| {
            let Some(job) = self.jobs.get(position as usize) else {
                return;
            };
            if job.state != JobState::Open {
                return;
            }
            *inspected += 1;
            if accept(job) {
                let entry = (job_distance(from, job.pos), job.id);
                let at = out.partition_point(|e| *e < entry);
                if at < k {
                    if out.len() == k {
                        out.pop();
                    }
                    out.insert(at, entry);
                }
            }
        };
        // Without a reachable inspection limit both traversals yield the exact result.
        if index.entries.len() <= inspect {
            for &position in &index.entries {
                consider(position, out, &mut inspected);
            }
            return;
        }
        let (cx, cy, cz) = index.cell_coords(from);
        let max_ring = cx
            .max(index.columns - 1 - cx)
            .max(cy)
            .max(index.rows - 1 - cy)
            .max(cz)
            .max(index.layers - 1 - cz);
        for ring in 0..=max_ring {
            if ring > 0 && out.len() >= k {
                let lower = job_distance(
                    Pos3::new(0, 0, 0),
                    Pos3::new((ring - 1) * CELL_XY + 1, 0, 0),
                )
                .min(job_distance(
                    Pos3::new(0, 0, 0),
                    Pos3::new(0, 0, (ring - 1) * CELL_Z + 1),
                ));
                if out[k - 1].0 < lower {
                    break;
                }
            }
            for z in cz - ring..=cz + ring {
                if !(0..index.layers).contains(&z) {
                    continue;
                }
                for y in cy - ring..=cy + ring {
                    if !(0..index.rows).contains(&y) {
                        continue;
                    }
                    let face = (z - cz).abs() == ring || (y - cy).abs() == ring;
                    let step = if face || ring == 0 {
                        1
                    } else {
                        2 * ring as usize
                    };
                    for x in (cx - ring..=cx + ring).step_by(step) {
                        if !(0..index.columns).contains(&x) {
                            continue;
                        }
                        let cell = index.cell((x, y, z));
                        let range = index.offsets[cell] as usize..index.offsets[cell + 1] as usize;
                        for &position in &index.entries[range] {
                            if inspected >= inspect {
                                return;
                            }
                            consider(position, out, &mut inspected);
                        }
                    }
                }
            }
        }
    }
}

/// Greedy one-to-one matching of workers and nearby jobs.
#[derive(Default)]
pub struct Assigner {
    pairs: Vec<(Reverse<i64>, JobId, u32)>,
    candidates: Vec<(u32, JobId)>,
    worker_taken: Vec<bool>,
    job_taken: Vec<JobId>,
}

impl Assigner {
    pub fn with_capacity(workers: usize, k: usize) -> Self {
        Self {
            pairs: Vec::with_capacity(workers * k),
            candidates: Vec::with_capacity(k + 1),
            worker_taken: Vec::with_capacity(workers),
            job_taken: Vec::with_capacity(workers),
        }
    }

    /// Each worker considers at most `k` nearest open jobs passing `accept`
    /// (which must include the reachability check). Pairs are taken greedily by
    /// descending score, then job ID, then worker key. `out` receives
    /// `(worker, job)` sorted by worker key, independent of the input order.
    #[allow(clippy::too_many_arguments)]
    pub fn assign<J, H: Copy + Eq + Ord>(
        &mut self,
        board: &JobBoard<J, H>,
        workers: &[(H, Pos3)],
        k: usize,
        inspect: usize,
        mut accept: impl FnMut(H, Pos3, &Job<J, H>) -> bool,
        score: impl Fn(&Job<J, H>, u32) -> i64,
        out: &mut Vec<(H, JobId)>,
    ) {
        out.clear();
        self.pairs.clear();
        for (worker, &(key, pos)) in workers.iter().enumerate() {
            board.nearest_open(
                pos,
                k,
                inspect,
                |job| accept(key, pos, job),
                &mut self.candidates,
            );
            for &(distance, id) in &self.candidates {
                if let Some(job) = board.get(id) {
                    self.pairs
                        .push((Reverse(score(job, distance)), id, worker as u32));
                }
            }
        }
        self.pairs.sort_unstable_by(|a, b| {
            (a.0, a.1, workers[a.2 as usize].0).cmp(&(b.0, b.1, workers[b.2 as usize].0))
        });
        self.worker_taken.clear();
        self.worker_taken.resize(workers.len(), false);
        self.job_taken.clear();
        for &(_, id, worker) in &self.pairs {
            let taken = &mut self.worker_taken[worker as usize];
            if *taken {
                continue;
            }
            let at = self.job_taken.partition_point(|&j| j < id);
            if self.job_taken.get(at) == Some(&id) {
                continue;
            }
            *taken = true;
            self.job_taken.insert(at, id);
            out.push((workers[worker as usize].0, id));
        }
        out.sort_unstable_by_key(|&(worker, _)| worker);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation<K, H> {
    pub key: K,
    pub holder: H,
    pub since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim<K, H> {
    pub key: K,
    pub holder: H,
    /// Larger values win.
    pub priority: i32,
    /// Smaller values win among equal priorities.
    pub distance: u32,
}

/// Exclusive reservations, granted only by resolving collected claims.
pub struct Reservations<K, H> {
    entries: Vec<Reservation<K, H>>,
    claims: Vec<Claim<K, H>>,
    capacity: usize,
}

impl<K: Copy + Ord, H: Copy + Ord> Reservations<K, H> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            claims: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn restore(capacity: usize, entries: Vec<Reservation<K, H>>) -> Option<Self> {
        if entries.len() > capacity || entries.windows(2).any(|pair| pair[0].key >= pair[1].key) {
            return None;
        }
        let mut table = Self::with_capacity(capacity);
        table.entries.extend(entries);
        Some(table)
    }

    fn find(&self, key: K) -> Result<usize, usize> {
        self.entries.binary_search_by(|entry| entry.key.cmp(&key))
    }

    pub fn holder(&self, key: K) -> Option<H> {
        self.find(key).ok().map(|index| self.entries[index].holder)
    }

    pub fn is_reserved(&self, key: K) -> bool {
        self.find(key).is_ok()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Reservation<K, H>> {
        self.entries.iter()
    }

    /// Registers an intent. Claims take effect only in `resolve`.
    pub fn claim(&mut self, key: K, holder: H, priority: i32, distance: u32) {
        if self.claims.len() < self.claims.capacity() {
            self.claims.push(Claim {
                key,
                holder,
                priority,
                distance,
            });
        }
    }

    /// Sorts claims by key, then descending priority, ascending distance and holder,
    /// grants each free key to its first claim and reports every claim's outcome
    /// in that order. Claims on reserved keys or beyond capacity lose.
    pub fn resolve(&mut self, tick: u64, mut outcome: impl FnMut(&Claim<K, H>, bool)) {
        self.claims.sort_unstable_by(|a, b| {
            (a.key, Reverse(a.priority), a.distance, a.holder).cmp(&(
                b.key,
                Reverse(b.priority),
                b.distance,
                b.holder,
            ))
        });
        let mut previous = None;
        for i in 0..self.claims.len() {
            let claim = self.claims[i];
            let first = previous != Some(claim.key);
            previous = Some(claim.key);
            let won = first
                && self.entries.len() < self.capacity
                && match self.find(claim.key) {
                    Ok(_) => false,
                    Err(at) => {
                        self.entries.insert(
                            at,
                            Reservation {
                                key: claim.key,
                                holder: claim.holder,
                                since: tick,
                            },
                        );
                        true
                    }
                };
            outcome(&claim, won);
        }
        self.claims.clear();
    }

    pub fn release(&mut self, key: K) -> bool {
        match self.find(key) {
            Ok(index) => {
                self.entries.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// Releases everything held by `holder`, for example after its death.
    pub fn release_holder(&mut self, holder: H) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.holder != holder);
        before - self.entries.len()
    }

    /// Reservations older than `timeout` ticks, in key order.
    pub fn expired(&self, tick: u64, timeout: u64, out: &mut Vec<(K, H)>) {
        out.clear();
        out.extend(
            self.entries
                .iter()
                .filter(|entry| tick.saturating_sub(entry.since) > timeout)
                .map(|entry| (entry.key, entry.holder)),
        );
    }

    /// Every reservation has a living holder that holds the matching task.
    /// Keys are unique by construction.
    pub fn check(
        &self,
        alive: impl Fn(H) -> bool,
        holds: impl Fn(K, H) -> bool,
    ) -> Result<(), &'static str> {
        if self
            .entries
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err("reservation keys are not unique");
        }
        for entry in &self.entries {
            if !alive(entry.holder) {
                return Err("reservation holder is not alive");
            }
            if !holds(entry.key, entry.holder) {
                return Err("reservation holder does not hold the matching task");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entity::{EntityTable, TypedId},
        rng::{SystemRng, below},
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Worker {}
    type WorkerId = TypedId<Worker>;

    fn board_with_jobs(seed: u64, count: usize) -> JobBoard<u8, WorkerId> {
        let mut board = JobBoard::new(256, 40, 40, 6);
        let mut rng = SystemRng::new(seed, 51);
        for i in 0..count {
            let pos = Pos3::new(
                below(rng.next_u64(), 40) as i32,
                below(rng.next_u64(), 40) as i32,
                below(rng.next_u64(), 6) as i32,
            );
            board.add((i % 3) as i32, pos, (i % 5) as u8).unwrap();
        }
        board.rebuild_index(|_| true);
        board
    }

    #[test]
    fn nearest_open_matches_brute_force() {
        let board = board_with_jobs(4, 120);
        let mut out = Vec::new();
        for (x, y, z) in [(0, 0, 0), (20, 20, 3), (39, 5, 5), (10, 30, 1)] {
            let from = Pos3::new(x, y, z);
            board.nearest_open(from, 8, usize::MAX, |job| job.data != 0, &mut out);
            let mut expected: Vec<_> = board
                .iter()
                .filter(|job| job.data != 0)
                .map(|job| (job_distance(from, job.pos), job.id))
                .collect();
            expected.sort_unstable();
            expected.truncate(8);
            assert_eq!(out, expected);
        }
        for (x, y, z) in [(0, 0, 0), (20, 20, 3), (39, 39, 5)] {
            let from = Pos3::new(x, y, z);
            board.nearest_open(from, 4, 119, |_| true, &mut out);
            let mut expected: Vec<_> = board
                .iter()
                .map(|job| (job_distance(from, job.pos), job.id))
                .collect();
            expected.sort_unstable();
            expected.truncate(4);
            assert_eq!(out, expected, "ring traversal from {from:?}");
        }
        board.nearest_open(Pos3::new(0, 0, 0), 8, 3, |_| false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn assignment_is_independent_of_worker_order() {
        let board = board_with_jobs(9, 60);
        let mut table: EntityTable<Worker> = EntityTable::with_capacity(32);
        let mut rng = SystemRng::new(9, 52);
        let workers: Vec<(WorkerId, Pos3)> = (0..30)
            .map(|_| {
                (
                    table.create().unwrap(),
                    Pos3::new(
                        below(rng.next_u64(), 40) as i32,
                        below(rng.next_u64(), 40) as i32,
                        below(rng.next_u64(), 6) as i32,
                    ),
                )
            })
            .collect();
        let run = |workers: &[(WorkerId, Pos3)]| {
            let mut assigner = Assigner::with_capacity(32, 8);
            let mut out = Vec::new();
            assigner.assign(
                &board,
                workers,
                8,
                64,
                |_, _, job| job.data != 4,
                |job, distance| i64::from(job.priority) * 1000 - i64::from(distance),
                &mut out,
            );
            out
        };
        let forward = run(&workers);
        let mut reversed = workers.clone();
        reversed.reverse();
        assert_eq!(forward, run(&reversed));
        assert!(forward.len() > 10);
        let mut jobs: Vec<_> = forward.iter().map(|&(_, job)| job).collect();
        jobs.sort_unstable();
        jobs.dedup();
        assert_eq!(jobs.len(), forward.len(), "a job was assigned twice");
        assert!(
            forward
                .iter()
                .all(|&(_, job)| board.get(job).unwrap().data != 4)
        );
    }

    #[test]
    fn unreachable_jobs_are_never_assigned() {
        let board = board_with_jobs(2, 40);
        let workers = [(WorkerId::new(0, 0), Pos3::new(5, 5, 0))];
        let mut assigner = Assigner::with_capacity(1, 8);
        let mut out = Vec::new();
        assigner.assign(&board, &workers, 8, 64, |_, _, _| false, |_, _| 0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn competing_claims_never_double_reserve() {
        for seed in 0..10 {
            let mut rng = SystemRng::new(seed, 53);
            let mut claims: Vec<(u32, WorkerId, i32, u32)> = (0..40)
                .map(|i| {
                    (
                        below(rng.next_u64(), 6) as u32,
                        WorkerId::new(i, 0),
                        below(rng.next_u64(), 3) as i32,
                        below(rng.next_u64(), 50) as u32,
                    )
                })
                .collect();
            let resolve = |claims: &[(u32, WorkerId, i32, u32)]| {
                let mut table = Reservations::with_capacity(64);
                for &(key, holder, priority, distance) in claims {
                    table.claim(key, holder, priority, distance);
                }
                let mut wins = Vec::new();
                table.resolve(7, |claim, won| {
                    if won {
                        wins.push((claim.key, claim.holder));
                    }
                });
                (wins, table.iter().copied().collect::<Vec<_>>())
            };
            let (wins, entries) = resolve(&claims);
            claims.reverse();
            assert_eq!(resolve(&claims), (wins.clone(), entries.clone()));
            let mut keys: Vec<_> = wins.iter().map(|(key, _)| *key).collect();
            keys.dedup();
            assert_eq!(keys.len(), wins.len());
            for &(key, holder) in &wins {
                let best = claims
                    .iter()
                    .filter(|c| c.0 == key)
                    .min_by_key(|c| (Reverse(c.2), c.3, c.1))
                    .unwrap();
                assert_eq!(best.1, holder);
            }
            assert!(entries.iter().all(|e| e.since == 7));
        }
    }

    #[test]
    fn reservations_release_on_death_and_timeout() {
        let mut table: EntityTable<Worker> = EntityTable::with_capacity(4);
        let a = table.create().unwrap();
        let b = table.create().unwrap();
        let mut reservations = Reservations::with_capacity(8);
        reservations.claim(1u32, a, 0, 0);
        reservations.claim(2, a, 0, 0);
        reservations.claim(3, b, 0, 0);
        reservations.resolve(10, |_, _| {});
        assert_eq!(reservations.len(), 3);
        assert!(
            reservations
                .check(|h| table.is_alive(h), |_, _| true)
                .is_ok()
        );
        table.destroy(a);
        assert!(
            reservations
                .check(|h| table.is_alive(h), |_, _| true)
                .is_err()
        );
        assert_eq!(reservations.release_holder(a), 2);
        assert!(
            reservations
                .check(|h| table.is_alive(h), |_, _| true)
                .is_ok()
        );
        assert_eq!(reservations.holder(3), Some(b));
        let mut expired = Vec::new();
        reservations.expired(20, 10, &mut expired);
        assert!(expired.is_empty());
        reservations.expired(21, 10, &mut expired);
        assert_eq!(expired, [(3, b)]);
        reservations.claim(3, a, 9, 0);
        let mut lost = false;
        reservations.resolve(22, |_, won| lost = !won);
        assert!(lost, "a reserved key cannot be claimed again");
        assert!(reservations.release(3) && reservations.is_empty());
    }

    #[test]
    fn job_lifecycle_and_restore() {
        let mut board: JobBoard<u8, WorkerId> = JobBoard::new(2, 8, 8, 2);
        let worker = WorkerId::new(0, 0);
        let first = board.add(1, Pos3::new(1, 1, 0), 7).unwrap();
        let second = board.add(1, Pos3::new(2, 1, 0), 8).unwrap();
        assert!(board.add(1, Pos3::new(3, 1, 0), 9).is_none());
        assert!(board.claim(first, worker, 3));
        assert!(!board.claim(first, worker, 4));
        assert_eq!(board.counts(), [1, 1, 0]);
        assert!(board.release(first) && board.complete(second));
        assert_eq!(board.retain(|job| job.state != JobState::Done), 1);
        let jobs: Vec<_> = board.iter().cloned().collect();
        let restored = JobBoard::restore(2, 8, 8, 2, board.next_id(), jobs.clone()).unwrap();
        assert_eq!(restored.iter().cloned().collect::<Vec<_>>(), jobs);
        assert_eq!(restored.next_id(), 2);
        assert!(JobBoard::<u8, WorkerId>::restore(2, 8, 8, 2, 0, jobs).is_none());
    }
}
