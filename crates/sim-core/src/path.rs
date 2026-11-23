//! Bounded A* with reusable buffers and a per-tick budgeted request queue.

use std::{cmp::Reverse, collections::BinaryHeap};

use serde::{Deserialize, Serialize};

use crate::topology::Topology;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathResult {
    /// The path was written to the caller's buffer, from start to goal inclusive.
    Found {
        cost: u32,
    },
    Unreachable,
    BudgetExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOutcome {
    pub result: PathResult,
    pub expansions: u32,
}

/// A* over dense node indices. Visited marks use generation stamps, so a search
/// never clears the arrays. Open-list keys `(f, h, node)` make ties deterministic.
pub struct AStar<N> {
    g: Vec<u32>,
    parent: Vec<N>,
    seen: Vec<u32>,
    closed: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32, N)>>,
    neighbors: Vec<(N, u32)>,
}

impl<N: Copy + Ord + core::fmt::Debug> AStar<N> {
    pub fn with_capacity(nodes: usize, open: usize) -> Self {
        Self {
            g: Vec::with_capacity(nodes),
            parent: Vec::with_capacity(nodes),
            seen: Vec::with_capacity(nodes),
            closed: Vec::with_capacity(nodes),
            generation: 0,
            heap: BinaryHeap::with_capacity(open),
            neighbors: Vec::with_capacity(32),
        }
    }

    fn prepare(&mut self, count: usize, fill: N) {
        if self.g.len() < count {
            self.g.resize(count, 0);
            self.parent.resize(count, fill);
            self.seen.resize(count, 0);
            self.closed.resize(count, 0);
        }
        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.seen.fill(0);
                self.closed.fill(0);
                1
            }
        };
        self.heap.clear();
    }

    /// Searches at most `limit` expansions. A found path is written to `path`.
    pub fn search<T: Topology<Node = N>>(
        &mut self,
        topology: &T,
        start: N,
        goal: N,
        limit: u32,
        path: &mut Vec<N>,
    ) -> SearchOutcome {
        path.clear();
        let count = topology.node_count();
        let start_index = topology.node_index(start);
        if start_index >= count || topology.node_index(goal) >= count {
            return SearchOutcome {
                result: PathResult::Unreachable,
                expansions: 0,
            };
        }
        self.prepare(count, start);
        let generation = self.generation;
        self.g[start_index] = 0;
        self.seen[start_index] = generation;
        let h = topology.heuristic(start, goal);
        self.heap.push(Reverse((h, h, start)));
        let mut expansions = 0;
        while let Some(Reverse((_, _, node))) = self.heap.pop() {
            let index = topology.node_index(node);
            if self.closed[index] == generation {
                continue;
            }
            if node == goal {
                let cost = self.g[index];
                let mut current = node;
                path.push(current);
                while current != start {
                    current = self.parent[topology.node_index(current)];
                    path.push(current);
                }
                path.reverse();
                return SearchOutcome {
                    result: PathResult::Found { cost },
                    expansions,
                };
            }
            if expansions >= limit {
                return SearchOutcome {
                    result: PathResult::BudgetExceeded,
                    expansions,
                };
            }
            self.closed[index] = generation;
            expansions += 1;
            let base = self.g[index];
            topology.neighbors(node, &mut self.neighbors);
            for &(next, step) in &self.neighbors {
                let j = topology.node_index(next);
                if j >= count || self.closed[j] == generation {
                    continue;
                }
                let candidate = base.saturating_add(step);
                if self.seen[j] != generation || candidate < self.g[j] {
                    self.seen[j] = generation;
                    self.g[j] = candidate;
                    self.parent[j] = node;
                    let h = topology.heuristic(next, goal);
                    self.heap
                        .push(Reverse((candidate.saturating_add(h), h, next)));
                }
            }
        }
        SearchOutcome {
            result: PathResult::Unreachable,
            expansions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRequest<K, N> {
    pub key: K,
    pub start: N,
    pub goal: N,
    pub submitted: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub searches: u64,
    pub expansions: u64,
    pub found: u64,
    pub unreachable: u64,
    pub budget_exceeded: u64,
    /// Requests answered as unreachable by the precheck, without a search.
    pub skipped: u64,
    pub path_nodes: u64,
    pub wait_ticks: u64,
}

impl QueueStats {
    pub fn add(&mut self, other: &Self) {
        self.searches += other.searches;
        self.expansions += other.expansions;
        self.found += other.found;
        self.unreachable += other.unreachable;
        self.budget_exceeded += other.budget_exceeded;
        self.skipped += other.skipped;
        self.path_nodes += other.path_nodes;
        self.wait_ticks += other.wait_ticks;
    }
}

/// Collects path requests and answers them in `(submitted tick, key)` order
/// within a global expansion budget per tick. Requests that do not fit wait for
/// later ticks. Ordering by submission tick first prevents starvation of large
/// keys; within a tick the requesting key decides, independent of insertion order.
pub struct PathRequestQueue<K, N> {
    pending: Vec<PathRequest<K, N>>,
}

impl<K: Copy + Ord, N: Copy + Ord + core::fmt::Debug> PathRequestQueue<K, N> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pending: Vec::with_capacity(capacity),
        }
    }

    /// Adds or replaces the request of `key`.
    pub fn push(&mut self, key: K, start: N, goal: N, tick: u64) {
        let request = PathRequest {
            key,
            start,
            goal,
            submitted: tick,
        };
        match self.pending.iter_mut().find(|r| r.key == key) {
            Some(existing) => *existing = request,
            None => self.pending.push(request),
        }
    }

    pub fn cancel(&mut self, key: K) {
        self.pending.retain(|r| r.key != key);
    }

    pub fn contains(&self, key: K) -> bool {
        self.pending.iter().any(|r| r.key == key)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Pending requests in processing order.
    pub fn sorted(&mut self) -> &[PathRequest<K, N>] {
        self.pending.sort_unstable_by_key(|r| (r.submitted, r.key));
        &self.pending
    }

    /// Restores requests with unique keys.
    pub fn restore(capacity: usize, mut requests: Vec<PathRequest<K, N>>) -> Option<Self> {
        requests.sort_unstable_by_key(|r| (r.submitted, r.key));
        let mut keys: Vec<K> = requests.iter().map(|r| r.key).collect();
        keys.sort_unstable();
        if keys.windows(2).any(|pair| pair[0] == pair[1]) {
            return None;
        }
        let mut queue = Self::with_capacity(capacity.max(requests.len()));
        queue.pending.extend(requests);
        Some(queue)
    }

    /// Processes requests until the tick budget is used up. A search may use at
    /// most `per_search` expansions, so a tick uses at most `budget + per_search`.
    /// Requests failing `reachable` are answered as unreachable without a search.
    #[allow(clippy::too_many_arguments)]
    pub fn process<T: Topology<Node = N>>(
        &mut self,
        astar: &mut AStar<N>,
        topology: &T,
        tick: u64,
        budget: u32,
        per_search: u32,
        reachable: impl Fn(N, N) -> bool,
        path: &mut Vec<N>,
        mut deliver: impl FnMut(&PathRequest<K, N>, SearchOutcome, &[N]),
    ) -> QueueStats {
        self.pending.sort_unstable_by_key(|r| (r.submitted, r.key));
        let mut stats = QueueStats::default();
        let mut remaining = budget;
        let mut done = 0;
        for request in &self.pending {
            if remaining == 0 {
                break;
            }
            done += 1;
            stats.wait_ticks += tick.saturating_sub(request.submitted);
            if !reachable(request.start, request.goal) {
                stats.skipped += 1;
                path.clear();
                deliver(
                    request,
                    SearchOutcome {
                        result: PathResult::Unreachable,
                        expansions: 0,
                    },
                    path,
                );
                continue;
            }
            let outcome = astar.search(topology, request.start, request.goal, per_search, path);
            remaining = remaining.saturating_sub(outcome.expansions.max(1));
            stats.searches += 1;
            stats.expansions += u64::from(outcome.expansions);
            match outcome.result {
                PathResult::Found { .. } => {
                    stats.found += 1;
                    stats.path_nodes += path.len() as u64;
                }
                PathResult::Unreachable => stats.unreachable += 1,
                PathResult::BudgetExceeded => stats.budget_exceeded += 1,
            }
            deliver(request, outcome, path);
        }
        self.pending.drain(..done);
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        rng::{SystemRng, below},
        space::{ChunkedGrid3, Grid2, Point, Pos3, Tile, TileKind},
        topology::{Grid2Topology, LayeredWalkTopology, dijkstra, tests::random_layered},
    };

    fn assert_valid_path<T: Topology>(topology: &T, path: &[T::Node], cost: u32) {
        let mut total = 0;
        let mut out = Vec::new();
        for pair in path.windows(2) {
            topology.neighbors(pair[0], &mut out);
            let step = out.iter().find(|(n, _)| *n == pair[1]).map(|(_, c)| *c);
            total += step.expect("consecutive path nodes must be neighbors");
        }
        assert_eq!(total, cost);
    }

    #[test]
    fn astar_costs_match_dijkstra_on_random_layered_worlds() {
        let mut astar = AStar::with_capacity(0, 0);
        let mut path = Vec::new();
        for seed in 0..16 {
            let grid = random_layered(seed, 11, 9, 5);
            let topology = LayeredWalkTopology::new(&grid);
            let nodes: Vec<_> = grid.iter_logical().map(|(p, _)| p).collect();
            let mut rng = SystemRng::new(seed, 31);
            for _ in 0..6 {
                let start = nodes[below(rng.next_u64(), nodes.len() as u64) as usize];
                let dist = dijkstra(&topology, start);
                for &goal in nodes.iter().step_by(3) {
                    let expected = dist[grid.index(goal).unwrap()];
                    let outcome = astar.search(&topology, start, goal, u32::MAX, &mut path);
                    match outcome.result {
                        PathResult::Found { cost } => {
                            assert_eq!(cost, expected);
                            assert_eq!(path.first(), Some(&start));
                            assert_eq!(path.last(), Some(&goal));
                            assert_valid_path(&topology, &path, cost);
                        }
                        PathResult::Unreachable => assert_eq!(expected, u32::MAX),
                        PathResult::BudgetExceeded => unreachable!(),
                    }
                }
            }
        }
    }

    #[test]
    fn astar_costs_match_dijkstra_on_2d_grids() {
        let mut astar = AStar::with_capacity(0, 0);
        let mut path = Vec::new();
        for seed in 0..8 {
            let mut rng = SystemRng::new(seed, 32);
            let mut passable = Grid2::new(17, 13, false).unwrap();
            for value in passable.as_mut_slice() {
                *value = below(rng.next_u64(), 10) < 7;
            }
            let topology = Grid2Topology {
                passable: &passable,
                diagonal: seed % 2 == 0,
            };
            let start = Point { x: 0, y: 0 };
            let dist = dijkstra(&topology, start);
            for y in 0..13 {
                for x in 0..17 {
                    let goal = Point { x, y };
                    let outcome = astar.search(&topology, start, goal, u32::MAX, &mut path);
                    let expected = dist[passable.index(goal).unwrap()];
                    match outcome.result {
                        PathResult::Found { cost } => assert_eq!(cost, expected),
                        _ => assert_eq!(expected, u32::MAX),
                    }
                }
            }
        }
    }

    #[test]
    fn ties_resolve_identically_across_instances_and_stamp_wraps() {
        let grid = ChunkedGrid3::new(12, 12, 1, Tile::new(TileKind::Open, true, 0)).unwrap();
        let topology = LayeredWalkTopology::new(&grid);
        let (start, goal) = (Pos3::new(0, 0, 0), Pos3::new(9, 4, 0));
        let mut fresh = AStar::with_capacity(0, 0);
        let mut reused = AStar::with_capacity(0, 0);
        let mut wrapped = AStar::with_capacity(0, 0);
        let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
        fresh.search(&topology, start, goal, u32::MAX, &mut a);
        for _ in 0..3 {
            reused.search(&topology, goal, start, u32::MAX, &mut b);
        }
        reused.search(&topology, start, goal, u32::MAX, &mut b);
        wrapped.search(&topology, goal, start, u32::MAX, &mut c);
        wrapped.generation = u32::MAX;
        wrapped.search(&topology, start, goal, u32::MAX, &mut c);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a.len(), 10);
    }

    #[test]
    fn expansion_limits_report_budget_exceeded() {
        let grid = ChunkedGrid3::new(40, 40, 1, Tile::new(TileKind::Open, true, 0)).unwrap();
        let topology = LayeredWalkTopology::new(&grid);
        let mut astar = AStar::with_capacity(0, 0);
        let mut path = vec![Pos3::new(1, 1, 1)];
        let (start, goal) = (Pos3::new(0, 0, 0), Pos3::new(39, 39, 0));
        let outcome = astar.search(&topology, start, goal, 5, &mut path);
        assert_eq!(outcome.result, PathResult::BudgetExceeded);
        assert_eq!(outcome.expansions, 5);
        assert!(path.is_empty());
        let full = astar.search(&topology, start, goal, u32::MAX, &mut path);
        assert_eq!(full.result, PathResult::Found { cost: 39 * 14 });
        let blocked = ChunkedGrid3::new(4, 1, 1, Tile::new(TileKind::Solid, false, 0)).unwrap();
        let outcome = astar.search(
            &LayeredWalkTopology::new(&blocked),
            Pos3::new(0, 0, 0),
            Pos3::new(9, 0, 0),
            100,
            &mut path,
        );
        assert_eq!(outcome.result, PathResult::Unreachable);
    }

    #[test]
    fn queue_order_is_independent_of_insertion_order_and_respects_budget() {
        let grid = random_layered(7, 16, 16, 3);
        let topology = LayeredWalkTopology::new(&grid);
        let nodes: Vec<_> = grid
            .iter_logical()
            .filter(|(_, t)| crate::space::standable(**t))
            .map(|(p, _)| p)
            .collect();
        let mut rng = SystemRng::new(3, 33);
        let requests: Vec<(u32, Pos3, Pos3, u64)> = (0..40)
            .map(|key| {
                let a = nodes[below(rng.next_u64(), nodes.len() as u64) as usize];
                let b = nodes[below(rng.next_u64(), nodes.len() as u64) as usize];
                (key * 7 % 41, a, b, u64::from(key % 3))
            })
            .collect();
        let run = |order: &[(u32, Pos3, Pos3, u64)]| {
            let mut queue = PathRequestQueue::with_capacity(64);
            for &(key, a, b, tick) in order {
                queue.push(key, a, b, tick);
            }
            let mut astar = AStar::with_capacity(0, 0);
            let mut path = Vec::new();
            let mut log = Vec::new();
            let mut ticks = Vec::new();
            for tick in 3..40 {
                let stats = queue.process(
                    &mut astar,
                    &topology,
                    tick,
                    60,
                    40,
                    |a, b| a.z == b.z || a.z + 1 == b.z,
                    &mut path,
                    |request, outcome, path| log.push((request.key, outcome, path.to_vec())),
                );
                assert!(stats.expansions <= 100);
                ticks.push((stats.searches, stats.skipped, queue.len()));
            }
            (log, ticks)
        };
        let mut reversed = requests.clone();
        reversed.reverse();
        let (log, ticks) = run(&requests);
        assert_eq!((log.clone(), ticks.clone()), run(&reversed));
        assert_eq!(log.len(), 40);
        assert!(ticks[0].2 > 0, "the budget must defer some requests");
        assert!(log.iter().any(|(_, o, _)| o.expansions == 0));
        let mut keys: Vec<_> = log.iter().map(|(k, _, _)| *k).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 40);
    }

    #[test]
    fn queue_replaces_and_restores_requests() {
        let mut queue = PathRequestQueue::<u32, u32>::with_capacity(4);
        queue.push(2, 0, 1, 5);
        queue.push(1, 0, 2, 5);
        queue.push(2, 3, 4, 6);
        assert_eq!(queue.len(), 2);
        let sorted = queue.sorted().to_vec();
        assert_eq!(sorted[0].key, 1);
        assert_eq!((sorted[1].start, sorted[1].submitted), (3, 6));
        let restored = PathRequestQueue::restore(4, sorted.clone()).unwrap();
        assert_eq!(restored.pending, sorted);
        assert!(PathRequestQueue::restore(4, vec![sorted[0], sorted[0]]).is_none());
        queue.cancel(1);
        assert!(!queue.contains(1) && queue.contains(2));
    }
}
