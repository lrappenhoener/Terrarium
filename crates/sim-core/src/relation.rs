//! One-to-many membership by dense slots, for example items inside containers.
//!
//! Each child belongs to at most one parent. Parents keep intrusive lists, so
//! inserting and removing never allocate after construction. The lists are derived
//! data: their order depends on the update history and must never be observable;
//! callers that pick a member choose by a stable key.

const NONE: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct Membership {
    parent: Vec<u32>,
    head: Vec<u32>,
    next: Vec<u32>,
    prev: Vec<u32>,
    count: Vec<u32>,
}

impl Membership {
    pub fn new(capacity: usize) -> Self {
        Self {
            parent: vec![NONE; capacity],
            head: vec![NONE; capacity],
            next: vec![NONE; capacity],
            prev: vec![NONE; capacity],
            count: vec![0; capacity],
        }
    }

    pub fn clear(&mut self) {
        self.parent.fill(NONE);
        self.head.fill(NONE);
        self.next.fill(NONE);
        self.prev.fill(NONE);
        self.count.fill(0);
    }

    pub fn parent(&self, child: u32) -> Option<u32> {
        self.parent
            .get(child as usize)
            .copied()
            .filter(|&p| p != NONE)
    }

    pub fn count(&self, parent: u32) -> u32 {
        self.count.get(parent as usize).copied().unwrap_or(0)
    }

    /// Makes `child` a member of `parent`, leaving any previous parent.
    pub fn insert(&mut self, child: u32, parent: u32) -> bool {
        let (c, p) = (child as usize, parent as usize);
        if c >= self.parent.len() || p >= self.parent.len() || child == parent {
            return false;
        }
        self.remove(child);
        let head = self.head[p];
        self.next[c] = head;
        self.prev[c] = NONE;
        if head != NONE {
            self.prev[head as usize] = child;
        }
        self.head[p] = child;
        self.parent[c] = parent;
        self.count[p] += 1;
        true
    }

    pub fn remove(&mut self, child: u32) -> bool {
        let c = child as usize;
        let Some(parent) = self.parent(child) else {
            return false;
        };
        let (prev, next) = (self.prev[c], self.next[c]);
        if prev == NONE {
            self.head[parent as usize] = next;
        } else {
            self.next[prev as usize] = next;
        }
        if next != NONE {
            self.prev[next as usize] = prev;
        }
        self.parent[c] = NONE;
        self.next[c] = NONE;
        self.prev[c] = NONE;
        self.count[parent as usize] -= 1;
        true
    }

    /// Members of `parent` in list order, which is not stable across histories.
    pub fn children(&self, parent: u32) -> impl Iterator<Item = u32> + '_ {
        let mut slot = self.head.get(parent as usize).copied().unwrap_or(NONE);
        std::iter::from_fn(move || {
            if slot == NONE {
                return None;
            }
            let current = slot;
            slot = self.next[current as usize];
            Some(current)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{SystemRng, below};

    #[test]
    fn membership_matches_a_parent_table_after_random_updates() {
        let n = 64u32;
        let mut relation = Membership::new(n as usize);
        let mut model: Vec<Option<u32>> = vec![None; n as usize];
        let mut rng = SystemRng::new(9, 1);
        for _ in 0..5000 {
            let child = below(rng.next_u64(), u64::from(n)) as u32;
            if below(rng.next_u64(), 3) == 0 {
                assert_eq!(
                    relation.remove(child),
                    model[child as usize].take().is_some()
                );
            } else {
                let parent = below(rng.next_u64(), u64::from(n)) as u32;
                let ok = relation.insert(child, parent);
                assert_eq!(ok, child != parent);
                if ok {
                    model[child as usize] = Some(parent);
                }
            }
            for parent in 0..n {
                let mut members: Vec<u32> = relation.children(parent).collect();
                members.sort_unstable();
                let expected: Vec<u32> = (0..n)
                    .filter(|&c| model[c as usize] == Some(parent))
                    .collect();
                assert_eq!(members, expected);
                assert_eq!(relation.count(parent) as usize, expected.len());
            }
        }
    }
}
