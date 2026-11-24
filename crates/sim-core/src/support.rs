//! Support as graph connectivity to anchors, with targeted, budgeted checks.
//!
//! Elements (for example solid tiles) support each other through the neighbors a
//! `SupportGraph` reports; anchors are supported by definition. After elements are
//! removed, their former neighbors are queued as seeds. A check searches the
//! component of a seed, preferring lower levels, and stops at the first anchor; a
//! component without an anchor is unsupported. A check that exceeds its unit limit
//! is retried in the next tick with a doubled limit, so a decision may be delayed
//! but is always the same. The result of a complete check equals a full
//! recomputation of all unanchored components.

use std::{cmp::Reverse, collections::BinaryHeap};

use crate::{physics::BudgetQueue, space::Pos3};

pub trait SupportGraph {
    fn is_element(&self, p: Pos3) -> bool;
    fn is_anchor(&self, p: Pos3) -> bool;
    /// Replaces `out` with the elements that `p` connects to. Must be symmetric.
    fn neighbors(&self, p: Pos3, out: &mut Vec<Pos3>);
    /// Dense index below `len` for visited marks.
    fn index(&self, p: Pos3) -> Option<usize>;
    fn node_count(&self) -> usize;
}

/// Diagnostic counters, excluded from hashes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupportStats {
    pub checks: u64,
    pub units: u64,
    pub retries: u64,
    pub unsupported_components: u64,
}

pub struct SupportChecker {
    queue: BudgetQueue<Pos3>,
    attempts: Vec<(Pos3, u8)>,
    stamp: Vec<u32>,
    verdict: Vec<u8>,
    generation: u32,
    frontier: BinaryHeap<Reverse<(i32, Pos3)>>,
    visited: Vec<Pos3>,
    neighbors: Vec<Pos3>,
    pub stats: SupportStats,
}

const SUPPORTED: u8 = 1;
const UNSUPPORTED: u8 = 2;

enum Outcome {
    Supported,
    Unsupported,
    Exceeded,
}

impl SupportChecker {
    pub fn new(len: usize, queue_capacity: usize) -> Self {
        Self {
            queue: BudgetQueue::with_capacity(queue_capacity),
            attempts: Vec::with_capacity(queue_capacity),
            stamp: vec![0; len],
            verdict: vec![0; len],
            generation: 0,
            frontier: BinaryHeap::with_capacity(1 << 16),
            visited: Vec::with_capacity(1 << 16),
            neighbors: Vec::with_capacity(16),
            stats: SupportStats::default(),
        }
    }

    /// Queues the elements next to a removed element.
    pub fn note_removed(&mut self, graph: &impl SupportGraph, p: Pos3) {
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let q = p.offset(dx, dy, dz);
            if graph.is_element(q) {
                self.queue.push(q);
            }
        }
    }

    /// Queues an added element, which may itself lack support.
    pub fn note_added(&mut self, graph: &impl SupportGraph, p: Pos3) {
        if graph.is_element(p) {
            self.queue.push(p);
        }
    }

    pub fn pending(&self) -> Vec<(Pos3, u8)> {
        self.queue
            .iter()
            .map(|&p| {
                let tries = self
                    .attempts
                    .binary_search_by_key(&p, |e| e.0)
                    .map_or(0, |i| self.attempts[i].1);
                (p, tries)
            })
            .collect()
    }

    pub fn restore(&mut self, pending: &[(Pos3, u8)]) {
        for &(p, tries) in pending {
            self.queue.push(p);
            if tries > 0 {
                self.attempts.push((p, tries));
            }
        }
        self.attempts.sort_unstable_by_key(|e| e.0);
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn next_generation(&mut self) {
        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.stamp.fill(0);
                1
            }
        };
    }

    /// Searches the component of `seed` with at most `limit` units.
    fn search(&mut self, graph: &impl SupportGraph, seed: Pos3, limit: u64) -> (Outcome, u64) {
        self.frontier.clear();
        self.visited.clear();
        let local = self.generation;
        let Some(start) = graph.index(seed) else {
            return (Outcome::Supported, 1);
        };
        if self.stamp[start] == local && self.verdict[start] != 0 {
            let verdict = self.verdict[start];
            return (
                if verdict == SUPPORTED {
                    Outcome::Supported
                } else {
                    Outcome::Unsupported
                },
                1,
            );
        }
        // Marks of an unfinished search use the verdict 0 within this generation.
        let mut seen = std::mem::take(&mut self.visited);
        self.frontier.push(Reverse((seed.z, seed)));
        self.stamp[start] = local;
        self.verdict[start] = 0;
        seen.push(seed);
        let mut units = 0;
        let outcome = 'search: loop {
            let Some(Reverse((_, p))) = self.frontier.pop() else {
                break Outcome::Unsupported;
            };
            units += 1;
            if graph.is_anchor(p) {
                break Outcome::Supported;
            }
            if units > limit {
                break Outcome::Exceeded;
            }
            graph.neighbors(p, &mut self.neighbors);
            for i in 0..self.neighbors.len() {
                let q = self.neighbors[i];
                let Some(j) = graph.index(q) else { continue };
                if self.stamp[j] == local {
                    // Connected to a component decided earlier in this run.
                    match self.verdict[j] {
                        SUPPORTED => break 'search Outcome::Supported,
                        UNSUPPORTED => break 'search Outcome::Unsupported,
                        _ => continue,
                    }
                }
                self.stamp[j] = local;
                self.verdict[j] = 0;
                seen.push(q);
                self.frontier.push(Reverse((q.z, q)));
            }
        };
        let verdict = match outcome {
            Outcome::Supported => SUPPORTED,
            Outcome::Unsupported => UNSUPPORTED,
            Outcome::Exceeded => 0,
        };
        for &p in &seen {
            if let Some(i) = graph.index(p) {
                if verdict == 0 {
                    // Forget unfinished marks so a later search starts fresh.
                    self.stamp[i] = 0;
                } else {
                    self.verdict[i] = verdict;
                }
            }
        }
        self.visited = seen;
        (outcome, units)
    }

    /// Runs queued checks within `budget` units and appends the positions of every
    /// unsupported component found to `out` in ascending order. `limit` is the
    /// first unit limit of a check; retries double it.
    pub fn run(&mut self, graph: &impl SupportGraph, budget: u64, limit: u64, out: &mut Vec<Pos3>) {
        out.clear();
        self.next_generation();
        let mut queue = std::mem::replace(&mut self.queue, BudgetQueue::with_capacity(0));
        let mut collected = std::mem::take(out);
        queue.process(budget, |seed, _| {
            let tries = self
                .attempts
                .binary_search_by_key(&seed, |e| e.0)
                .map_or(0, |i| self.attempts[i].1);
            let allowed = limit.saturating_mul(1 << tries.min(20));
            self.stats.checks += 1;
            let (outcome, units) = self.search(graph, seed, allowed);
            self.stats.units += units;
            match outcome {
                Outcome::Exceeded => {
                    self.stats.retries += 1;
                    match self.attempts.binary_search_by_key(&seed, |e| e.0) {
                        Ok(i) => self.attempts[i].1 = tries.saturating_add(1),
                        Err(i) => {
                            if self.attempts.len() < self.attempts.capacity() {
                                self.attempts.insert(i, (seed, 1));
                            }
                        }
                    }
                    (units, false)
                }
                Outcome::Unsupported => {
                    if units > 1 || !collected.contains(&seed) {
                        self.stats.unsupported_components += 1;
                    }
                    collected.extend(self.visited.iter().copied());
                    self.forget(seed);
                    (units, true)
                }
                Outcome::Supported => {
                    self.forget(seed);
                    (units, true)
                }
            }
        });
        collected.sort_unstable();
        collected.dedup();
        *out = collected;
        self.queue = queue;
    }

    fn forget(&mut self, seed: Pos3) {
        if let Ok(i) = self.attempts.binary_search_by_key(&seed, |e| e.0) {
            self.attempts.remove(i);
        }
    }

    /// Reference: every element in an unanchored component, by full recomputation.
    pub fn full(graph: &impl SupportGraph, all: impl Iterator<Item = Pos3>) -> Vec<Pos3> {
        let mut verdict = vec![0u8; graph.node_count()];
        let mut out = Vec::new();
        let mut neighbors = Vec::new();
        for p in all {
            let Some(i) = graph.index(p) else { continue };
            if verdict[i] != 0 || !graph.is_element(p) {
                continue;
            }
            let mut component = vec![p];
            verdict[i] = 3;
            let mut cursor = 0;
            let mut anchored = false;
            while let Some(&q) = component.get(cursor) {
                cursor += 1;
                anchored |= graph.is_anchor(q);
                graph.neighbors(q, &mut neighbors);
                for &r in &neighbors {
                    if let Some(j) = graph.index(r)
                        && verdict[j] == 0
                    {
                        verdict[j] = 3;
                        component.push(r);
                    }
                }
            }
            if !anchored {
                out.extend(component);
            }
        }
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        rng::{SystemRng, below},
        space::ChunkedGrid3,
    };

    /// Solid cells connect through six neighbors; the bottom level anchors.
    struct Blocks<'a>(&'a ChunkedGrid3<bool>);

    impl SupportGraph for Blocks<'_> {
        fn is_element(&self, p: Pos3) -> bool {
            self.0.get(p).copied().unwrap_or(false)
        }
        fn is_anchor(&self, p: Pos3) -> bool {
            self.is_element(p) && p.z == 0
        }
        fn neighbors(&self, p: Pos3, out: &mut Vec<Pos3>) {
            out.clear();
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let q = p.offset(dx, dy, dz);
                if self.is_element(q) {
                    out.push(q);
                }
            }
        }
        fn index(&self, p: Pos3) -> Option<usize> {
            self.0.index(p)
        }
        fn node_count(&self) -> usize {
            self.0.len()
        }
    }

    #[test]
    fn targeted_checks_match_full_recomputation() {
        let (w, h, d) = (14, 12, 8);
        for seed in [1, 2, 3, 42] {
            let mut rng = SystemRng::new(seed, 81);
            let mut grid = ChunkedGrid3::new(w, h, d, true).unwrap();
            let mut checker = SupportChecker::new(grid.len(), 4096);
            let mut out = Vec::new();
            let mut collapses = 0;
            for round in 0..600 {
                for _ in 0..1 + below(rng.next_u64(), 4) {
                    let p = Pos3::new(
                        below(rng.next_u64(), w as u64) as i32,
                        below(rng.next_u64(), h as u64) as i32,
                        below(rng.next_u64(), d as u64) as i32,
                    );
                    if below(rng.next_u64(), 3) == 0 {
                        grid.set(p, true);
                        checker.note_added(&Blocks(&grid), p);
                    } else if grid.get(p) == Some(&true) {
                        grid.set(p, false);
                        checker.note_removed(&Blocks(&grid), p);
                    }
                }
                // A small budget delays decisions; run until the queue is empty.
                let mut unsupported = Vec::new();
                while !checker.is_empty() {
                    checker.run(&Blocks(&grid), 40, 8, &mut out);
                    unsupported.extend(out.iter().copied());
                }
                unsupported.sort_unstable();
                unsupported.dedup();
                let all: Vec<Pos3> = grid.iter_logical().map(|(p, _)| p).collect();
                let expected = SupportChecker::full(&Blocks(&grid), all.into_iter());
                assert_eq!(unsupported, expected, "seed {seed} round {round}");
                // Collapse: remove what is unsupported; nothing anchored depends on it.
                collapses += usize::from(!unsupported.is_empty());
                for p in unsupported {
                    grid.set(p, false);
                }
            }
            assert!(collapses > 5, "seed {seed}");
            assert!(checker.stats.retries > 0);
        }
    }
}
