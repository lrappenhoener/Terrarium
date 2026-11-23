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

    /// Returns false when the stable event sequence is exhausted.
    pub fn push(&mut self, tick: u64, value: T) -> bool {
        let Some(next) = self.seq.checked_add(1) else {
            return false;
        };
        self.queue.push(Entry {
            tick,
            seq: self.seq,
            value,
        });
        self.seq = next;
        true
    }

    /// Rebuilds a queue with its original stable tie-break sequence.
    pub fn restore(capacity: usize, sequence: u64, events: Vec<(u64, u64, T)>) -> Option<Self> {
        if events.len() > capacity {
            return None;
        }
        let mut result = Self::with_capacity(capacity);
        result.seq = sequence;
        let mut seen = std::collections::BTreeSet::new();
        for (tick, seq, value) in events {
            if seq >= sequence || !seen.insert(seq) {
                return None;
            }
            result.queue.push(Entry { tick, seq, value });
        }
        Some(result)
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

#[cfg(test)]
mod tests {
    use super::EventQueue;

    #[test]
    fn exhausted_sequence_rejects_scheduling_without_wrapping() {
        let mut queue = EventQueue::with_capacity(1);
        queue.seq = u64::MAX - 1;
        assert!(queue.push(5, 1));
        assert!(!queue.push(5, 2));
        assert_eq!(queue.sequence(), u64::MAX);
        assert_eq!(queue.pop_due(5), Some(1));
        assert_eq!(queue.pop_due(5), None);
    }
}
