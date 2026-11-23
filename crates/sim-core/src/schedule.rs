use std::{cmp::Ordering, collections::BinaryHeap};

pub fn staggered(tick: u64, interval: u64, key: u64) -> bool {
    interval != 0 && (u128::from(tick) + u128::from(key)).is_multiple_of(u128::from(interval))
}

struct Entry<T> {
    tick: u64,
    seq: u64,
    value: T,
}

impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.tick, other.seq).cmp(&(self.tick, self.seq))
    }
}
impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl<T> Eq for Entry<T> {}

pub struct EventQueue<T> {
    queue: BinaryHeap<Entry<T>>,
    seq: u64,
}

impl<T> EventQueue<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            queue: BinaryHeap::with_capacity(capacity),
            seq: 0,
        }
    }

    pub fn push(&mut self, tick: u64, value: T) {
        self.queue.push(Entry {
            tick,
            seq: self.seq,
            value,
        });
        self.seq += 1;
    }

    pub fn pop_due(&mut self, tick: u64) -> Option<T> {
        if self.queue.peek()?.tick > tick {
            return None;
        }
        self.queue.pop().map(|entry| entry.value)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.queue.retain(|entry| keep(&entry.value));
    }

    pub fn sequence(&self) -> u64 {
        self.seq
    }

    pub fn sorted_events(&self) -> Vec<(u64, u64, &T)> {
        let mut events: Vec<_> = self
            .queue
            .iter()
            .map(|e| (e.tick, e.seq, &e.value))
            .collect();
        events.sort_unstable_by_key(|&(tick, seq, _)| (tick, seq));
        events
    }
}
