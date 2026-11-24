//! Building blocks for world physics: active chunk sets with sleep and wake rules,
//! pooled per-chunk double buffers for cellular automata, and a budgeted work queue.
//! All structures reserve their memory at construction and iterate in Morton order.

use crate::space::{CHUNK_X, CHUNK_Y, CHUNK_Z, ChunkId, ChunkedGrid3, Pos3};

const NONE: u32 = u32::MAX;
pub const CHUNK_VOLUME: usize = (CHUNK_X * CHUNK_Y * CHUNK_Z) as usize;

/// Chunks that need simulation. A chunk falls asleep after `sleep_after`
/// consecutive updates that changed nothing and could not change anything; any
/// change in it or a face neighbor (including above and below) wakes it.
pub struct ActiveChunkSet {
    awake: Vec<bool>,
    stable: Vec<u16>,
    list: Vec<ChunkId>,
    neighbors: Vec<[ChunkId; 6]>,
    sleep_after: u16,
    sleeping_enabled: bool,
    wakes: u64,
}

impl ActiveChunkSet {
    pub fn new<T>(grid: &ChunkedGrid3<T>, sleep_after: u16) -> Self {
        let count = grid.chunk_count();
        let neighbors = (0..count as ChunkId)
            .map(|chunk| {
                let origin = grid.chunk_origin(chunk).unwrap_or_default();
                let offsets = [
                    (-CHUNK_X, 0, 0),
                    (CHUNK_X, 0, 0),
                    (0, -CHUNK_Y, 0),
                    (0, CHUNK_Y, 0),
                    (0, 0, -CHUNK_Z),
                    (0, 0, CHUNK_Z),
                ];
                offsets.map(|(dx, dy, dz)| grid.chunk_of(origin.offset(dx, dy, dz)).unwrap_or(NONE))
            })
            .collect();
        Self {
            awake: vec![false; count],
            stable: vec![0; count],
            list: Vec::with_capacity(count),
            neighbors,
            sleep_after: sleep_after.max(1),
            sleeping_enabled: true,
            wakes: 0,
        }
    }

    /// Disabling sleep keeps every woken chunk awake; used to prove neutrality.
    pub fn set_sleeping_enabled(&mut self, enabled: bool) {
        self.sleeping_enabled = enabled;
    }

    pub fn wake(&mut self, chunk: ChunkId) {
        if let Some(awake) = self.awake.get_mut(chunk as usize) {
            if !*awake {
                self.wakes += 1;
            }
            *awake = true;
            self.stable[chunk as usize] = 0;
        }
    }

    /// Wakes the chunk and its six face neighbors.
    pub fn wake_around(&mut self, chunk: ChunkId) {
        self.wake(chunk);
        if let Some(neighbors) = self.neighbors.get(chunk as usize).copied() {
            for neighbor in neighbors {
                if neighbor != NONE {
                    self.wake(neighbor);
                }
            }
        }
    }

    /// Records the outcome of updating an awake chunk. `changed` wakes the
    /// neighborhood; `may_change` (for example a pending random event) prevents
    /// sleep without counting as a change.
    pub fn report(&mut self, chunk: ChunkId, changed: bool, may_change: bool) {
        let index = chunk as usize;
        if index >= self.awake.len() {
            return;
        }
        if changed {
            self.wake_around(chunk);
        } else if may_change {
            self.stable[index] = 0;
        } else {
            self.stable[index] = self.stable[index].saturating_add(1);
            if self.sleeping_enabled && self.stable[index] >= self.sleep_after {
                self.awake[index] = false;
                self.stable[index] = 0;
            }
        }
    }

    /// Puts a chunk to sleep regardless of its history, for example when it holds
    /// nothing to simulate at all.
    pub fn sleep(&mut self, chunk: ChunkId) {
        if self.sleeping_enabled
            && let Some(awake) = self.awake.get_mut(chunk as usize)
        {
            *awake = false;
            self.stable[chunk as usize] = 0;
        }
    }

    /// Snapshot of the awake chunks in Morton order for this tick's iteration.
    pub fn prepare(&mut self) -> &[ChunkId] {
        self.list.clear();
        self.list.extend(
            self.awake
                .iter()
                .enumerate()
                .filter(|(_, awake)| **awake)
                .map(|(chunk, _)| chunk as ChunkId),
        );
        &self.list
    }

    pub fn prepared(&self) -> &[ChunkId] {
        &self.list
    }

    pub fn is_awake(&self, chunk: ChunkId) -> bool {
        self.awake.get(chunk as usize).copied().unwrap_or(false)
    }

    pub fn awake_count(&self) -> usize {
        self.awake.iter().filter(|a| **a).count()
    }

    pub fn chunk_count(&self) -> usize {
        self.awake.len()
    }

    pub fn wakes(&self) -> u64 {
        self.wakes
    }

    pub fn neighbors(&self, chunk: ChunkId) -> [ChunkId; 6] {
        self.neighbors
            .get(chunk as usize)
            .copied()
            .unwrap_or([NONE; 6])
    }
}

/// Fixed pool of per-chunk double buffers. Reading uses the front buffer of the
/// previous step, writing the back buffer; `swap` flips all pairs at once.
/// Chunks without a buffer read as `T::default()`.
pub struct ChunkDoubleBuffer<T> {
    data: Vec<T>,
    slot_of: Vec<u32>,
    owner: Vec<ChunkId>,
    free: Vec<u32>,
    parity: usize,
    pub exhausted: u64,
}

impl<T: Copy + Default> ChunkDoubleBuffer<T> {
    pub fn new(chunk_count: usize, slots: usize) -> Self {
        Self {
            data: vec![T::default(); slots * 2 * CHUNK_VOLUME],
            slot_of: vec![NONE; chunk_count],
            owner: vec![NONE; slots],
            free: (0..slots as u32).rev().collect(),
            parity: 0,
            exhausted: 0,
        }
    }

    pub fn has(&self, chunk: ChunkId) -> bool {
        self.slot_of.get(chunk as usize).is_some_and(|s| *s != NONE)
    }

    /// Binds a zeroed buffer pair to the chunk. Fails deterministically, and is
    /// counted, when the pool is exhausted.
    pub fn acquire(&mut self, chunk: ChunkId) -> bool {
        let Some(slot) = self.slot_of.get(chunk as usize).copied() else {
            return false;
        };
        if slot != NONE {
            return true;
        }
        let Some(slot) = self.free.pop() else {
            self.exhausted += 1;
            return false;
        };
        self.slot_of[chunk as usize] = slot;
        self.owner[slot as usize] = chunk;
        let base = slot as usize * 2 * CHUNK_VOLUME;
        self.data[base..base + 2 * CHUNK_VOLUME].fill(T::default());
        true
    }

    pub fn release(&mut self, chunk: ChunkId) {
        if let Some(slot) = self.slot_of.get_mut(chunk as usize)
            && *slot != NONE
        {
            self.owner[*slot as usize] = NONE;
            self.free.push(*slot);
            *slot = NONE;
        }
    }

    fn range(&self, chunk: ChunkId, back: bool) -> Option<std::ops::Range<usize>> {
        let slot = *self.slot_of.get(chunk as usize)?;
        if slot == NONE {
            return None;
        }
        let half = (self.parity ^ usize::from(back)) * CHUNK_VOLUME;
        let start = slot as usize * 2 * CHUNK_VOLUME + half;
        Some(start..start + CHUNK_VOLUME)
    }

    pub fn front(&self, chunk: ChunkId) -> Option<&[T]> {
        let range = self.range(chunk, false)?;
        Some(&self.data[range])
    }

    pub fn back_mut(&mut self, chunk: ChunkId) -> Option<&mut [T]> {
        let range = self.range(chunk, true)?;
        Some(&mut self.data[range])
    }

    pub fn front_mut(&mut self, chunk: ChunkId) -> Option<&mut [T]> {
        let range = self.range(chunk, false)?;
        Some(&mut self.data[range])
    }

    /// Copies front to back for every bound chunk, the start of a step.
    pub fn begin_step(&mut self) {
        for slot in 0..self.owner.len() {
            if self.owner[slot] != NONE {
                let base = slot * 2 * CHUNK_VOLUME;
                let (front, back) = if self.parity == 0 {
                    (base, base + CHUNK_VOLUME)
                } else {
                    (base + CHUNK_VOLUME, base)
                };
                self.data.copy_within(front..front + CHUNK_VOLUME, back);
            }
        }
    }

    pub fn swap(&mut self) {
        self.parity ^= 1;
    }

    pub fn bound(&self) -> usize {
        self.owner.len() - self.free.len()
    }

    pub fn capacity(&self) -> usize {
        self.owner.len()
    }
}

/// Index of a position inside its chunk, matching `ChunkedGrid3` chunk layout.
pub fn local_index(p: Pos3) -> usize {
    let lx = (p.x & (CHUNK_X - 1)) as usize;
    let ly = (p.y & (CHUNK_Y - 1)) as usize;
    let lz = (p.z & (CHUNK_Z - 1)) as usize;
    (lz * CHUNK_Y as usize + ly) * CHUNK_X as usize + lx
}

/// Deterministic work queue: unique keys processed in ascending order, up to a
/// budget of work units per tick; unfinished work stays queued for later ticks.
pub struct BudgetQueue<K> {
    items: Vec<K>,
    done: Vec<bool>,
}

impl<K: Copy + Ord> BudgetQueue<K> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            done: Vec::with_capacity(capacity),
        }
    }

    /// Adds a key unless it is queued already. Returns false when full.
    pub fn push(&mut self, key: K) -> bool {
        match self.items.binary_search(&key) {
            Ok(_) => true,
            Err(at) if self.items.len() < self.items.capacity() => {
                self.items.insert(at, key);
                true
            }
            Err(_) => false,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.items.iter()
    }

    /// Processes keys in ascending order while the budget lasts. `work` returns
    /// the units it used and whether the key is finished; unfinished keys remain.
    /// A key may use more than the remaining budget, so a tick uses at most the
    /// budget plus one item's cost.
    pub fn process(&mut self, budget: u64, mut work: impl FnMut(K, u64) -> (u64, bool)) -> u64 {
        let mut used = 0;
        self.done.clear();
        self.done.resize(self.items.len(), false);
        for i in 0..self.items.len() {
            if used >= budget {
                break;
            }
            let (cost, finished) = work(self.items[i], budget - used);
            used = used.saturating_add(cost.max(1));
            self.done[i] = finished;
        }
        let done = &self.done;
        let mut index = 0;
        self.items.retain(|_| {
            let keep = !done[index];
            index += 1;
            keep
        });
        used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_wake_neighbors_and_sleep_after_stable_updates() {
        let grid = ChunkedGrid3::new(48, 32, 16, 0u8).unwrap();
        let mut set = ActiveChunkSet::new(&grid, 4);
        let center = grid.chunk_of(Pos3::new(20, 20, 9)).unwrap();
        set.wake(center);
        assert_eq!(set.prepare(), [center]);
        set.report(center, true, false);
        let mut expected: Vec<_> = [
            Pos3::new(20, 20, 9),
            Pos3::new(4, 20, 9),
            Pos3::new(36, 20, 9),
            Pos3::new(20, 4, 9),
            Pos3::new(20, 20, 1),
        ]
        .iter()
        .map(|p| grid.chunk_of(*p).unwrap())
        .collect();
        expected.sort_unstable();
        assert_eq!(
            set.prepare(),
            expected.as_slice(),
            "out-of-world neighbors are skipped"
        );
        for _ in 0..3 {
            set.report(center, false, false);
        }
        assert!(set.is_awake(center));
        set.report(center, false, true);
        for _ in 0..3 {
            set.report(center, false, false);
        }
        assert!(set.is_awake(center), "a possible change resets stability");
        set.report(center, false, false);
        assert!(!set.is_awake(center));
        set.set_sleeping_enabled(false);
        set.wake(center);
        for _ in 0..10 {
            set.report(center, false, false);
        }
        assert!(set.is_awake(center));
    }

    #[test]
    fn double_buffers_swap_and_report_exhaustion() {
        let mut pool: ChunkDoubleBuffer<i32> = ChunkDoubleBuffer::new(5, 2);
        assert!(pool.acquire(3) && pool.acquire(1) && pool.acquire(3));
        assert!(!pool.acquire(4));
        assert_eq!(pool.exhausted, 1);
        pool.back_mut(3).unwrap()[7] = 9;
        assert_eq!(pool.front(3).unwrap()[7], 0);
        pool.swap();
        assert_eq!(pool.front(3).unwrap()[7], 9);
        pool.begin_step();
        assert_eq!(pool.back_mut(3).unwrap()[7], 9);
        pool.release(3);
        assert!(pool.acquire(4));
        assert_eq!(pool.front(4).unwrap()[7], 0, "reused buffers start zeroed");
        assert!(pool.front(3).is_none());
        assert_eq!(pool.bound(), 2);
        assert_eq!(local_index(Pos3::new(17, 33, 9)), (16 + 1) * 16 + 1);
    }

    #[test]
    fn budget_queue_is_independent_of_insertion_order() {
        let run = |keys: &[u32]| {
            let mut queue = BudgetQueue::with_capacity(16);
            for &key in keys {
                assert!(queue.push(key));
            }
            let mut log = Vec::new();
            let mut ticks = 0;
            while !queue.is_empty() {
                ticks += 1;
                queue.process(5, |key, _| {
                    log.push((ticks, key));
                    (u64::from(key % 4 + 1), !key.is_multiple_of(3) || ticks > 2)
                });
            }
            log
        };
        let keys = [9, 2, 7, 4, 4, 11, 3, 6];
        let mut reversed = keys;
        reversed.reverse();
        let log = run(&keys);
        assert_eq!(log, run(&reversed));
        assert_eq!(log.first(), Some(&(1, 2)));
        let mut full = BudgetQueue::with_capacity(1);
        assert!(full.push(1) && full.push(1) && !full.push(2));
    }
}
