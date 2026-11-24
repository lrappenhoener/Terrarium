//! Hierarchical pathfinding (HPA*) over the chunks of a layered walk grid.
//!
//! Entrances are walk steps across chunk faces: orthogonal steps across lateral
//! faces, grouped into runs per level with the middle step as representative, and
//! single stair steps across horizontal faces. Each chunk caches the costs between
//! its entrance tiles, computed by searches confined to the chunk. Caches are
//! derived data: a face is valid while the versions of its two chunks are unchanged,
//! a chunk matrix while its own version and its entrance set are unchanged. Every
//! cached value is a pure function of the current tiles, so a cache hit equals a
//! recomputation. Diagonal and ramp steps across chunk faces are not entrances; the
//! caller falls back to plain A* when the abstract search fails.

use std::{cmp::Reverse, collections::BinaryHeap};

use super::{PathResult, SearchOutcome};
use crate::{
    space::{
        CHUNK_X, CHUNK_Y, CHUNK_Z, ChunkId, ChunkedGrid3, ORTHOGONAL_COST, Pos3, Tile, TileKind,
        VERTICAL_COST, standable, walk_neighbors,
    },
    topology::layered_heuristic,
};

const NODE_CAP: usize = 96;
const FACE_CAP: usize = 64;
const LOCAL: usize = (CHUNK_X * CHUNK_Y * CHUNK_Z) as usize;
const NO_EDGE: u16 = u16::MAX;

/// Diagnostic counters, excluded from state hashes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HpaStats {
    pub searches: u64,
    pub local_hits: u64,
    pub face_rebuilds: u64,
    pub chunk_rebuilds: u64,
    pub chunk_hits: u64,
    pub abstract_expansions: u64,
    pub local_expansions: u64,
    pub refine_expansions: u64,
}

struct Face {
    valid: bool,
    versions: (u32, u32),
    /// `(low side, high side, cost)` in ascending order.
    entrances: Vec<(Pos3, Pos3, u32)>,
}

struct ChunkCache {
    valid: bool,
    version: u32,
    nodes: Vec<Pos3>,
    matrix: Vec<u16>,
}

/// Search confined to one chunk with chunk-local dense indices.
struct Local {
    low: Pos3,
    high: Pos3,
    dist: Vec<u32>,
    parent: Vec<u16>,
    stamp: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u16)>>,
    neighbors: Vec<(Pos3, u32)>,
}

impl Local {
    fn new() -> Self {
        Self {
            low: Pos3::new(0, 0, 0),
            high: Pos3::new(0, 0, 0),
            dist: vec![0; LOCAL],
            parent: vec![0; LOCAL],
            stamp: vec![0; LOCAL],
            generation: 0,
            heap: BinaryHeap::with_capacity(LOCAL * 8),
            neighbors: Vec::with_capacity(32),
        }
    }

    fn index(&self, p: Pos3) -> Option<u16> {
        let inside = (self.low.x..self.high.x).contains(&p.x)
            && (self.low.y..self.high.y).contains(&p.y)
            && (self.low.z..self.high.z).contains(&p.z);
        inside.then(|| {
            (((p.z - self.low.z) * CHUNK_Y + (p.y - self.low.y)) * CHUNK_X + (p.x - self.low.x))
                as u16
        })
    }

    fn pos(&self, index: u16) -> Pos3 {
        let i = i32::from(index);
        Pos3::new(
            self.low.x + i % CHUNK_X,
            self.low.y + i / CHUNK_X % CHUNK_Y,
            self.low.z + i / (CHUNK_X * CHUNK_Y),
        )
    }

    fn cost(&self, p: Pos3) -> Option<u32> {
        let i = self.index(p)? as usize;
        (self.stamp[i] == self.generation).then_some(self.dist[i])
    }

    /// Dijkstra from `start` inside the bounds, stopping when `target` is settled.
    /// Returns the number of settled tiles.
    fn run(
        &mut self,
        grid: &ChunkedGrid3<Tile>,
        bounds: (Pos3, Pos3),
        start: Pos3,
        target: Option<Pos3>,
    ) -> u64 {
        (self.low, self.high) = bounds;
        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.stamp.fill(0);
                1
            }
        };
        self.heap.clear();
        let Some(origin) = self.index(start) else {
            return 0;
        };
        let generation = self.generation;
        self.dist[origin as usize] = 0;
        self.parent[origin as usize] = origin;
        self.stamp[origin as usize] = generation;
        self.heap.push(Reverse((0, origin)));
        let mut settled = 0;
        while let Some(Reverse((cost, index))) = self.heap.pop() {
            if cost > self.dist[index as usize] {
                continue;
            }
            settled += 1;
            let p = self.pos(index);
            if Some(p) == target {
                break;
            }
            self.neighbors.clear();
            walk_neighbors(grid, p, &mut self.neighbors);
            for i in 0..self.neighbors.len() {
                let (next, step) = self.neighbors[i];
                let Some(j) = self.index(next) else { continue };
                let candidate = cost + step;
                let slot = j as usize;
                if self.stamp[slot] != generation || candidate < self.dist[slot] {
                    self.stamp[slot] = generation;
                    self.dist[slot] = candidate;
                    self.parent[slot] = index;
                    self.heap.push(Reverse((candidate, j)));
                }
            }
        }
        settled
    }

    /// Appends the path to `target` from the last run, excluding its start.
    fn append_path(&self, target: Pos3, out: &mut Vec<Pos3>) {
        let Some(mut index) = self.index(target) else {
            return;
        };
        let begin = out.len();
        while self.parent[index as usize] != index {
            out.push(self.pos(index));
            index = self.parent[index as usize];
        }
        out[begin..].reverse();
    }
}

pub struct HierarchicalPathfinder {
    columns: i32,
    rows: i32,
    layers: i32,
    faces: Vec<Face>,
    chunks: Vec<ChunkCache>,
    chunk_ids: Vec<ChunkId>,
    local: Local,
    g: Vec<u32>,
    parent: Vec<u32>,
    seen: Vec<u32>,
    closed: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32, Pos3)>>,
    start_costs: Vec<(Pos3, u32)>,
    goal_costs: Vec<(Pos3, u32)>,
    scratch_nodes: Vec<Pos3>,
    route: Vec<Pos3>,
    edges: Vec<(Pos3, u32)>,
    /// Disables caching: every query recomputes all faces and chunks it touches.
    pub use_cache: bool,
    pub stats: HpaStats,
}

impl HierarchicalPathfinder {
    pub fn new(grid: &ChunkedGrid3<Tile>, open_capacity: u32) -> Self {
        let columns = (grid.width() - 1) / CHUNK_X + 1;
        let rows = (grid.height() - 1) / CHUNK_Y + 1;
        let layers = (grid.depth() - 1) / CHUNK_Z + 1;
        let count = (columns * rows * layers) as usize;
        let mut chunk_ids = Vec::with_capacity(count);
        for z in 0..layers {
            for y in 0..rows {
                for x in 0..columns {
                    let origin = Pos3::new(x * CHUNK_X, y * CHUNK_Y, z * CHUNK_Z);
                    chunk_ids.push(grid.chunk_of(origin).unwrap_or(0));
                }
            }
        }
        let len = grid.len();
        Self {
            columns,
            rows,
            layers,
            faces: (0..count * 3)
                .map(|_| Face {
                    valid: false,
                    versions: (0, 0),
                    entrances: Vec::with_capacity(FACE_CAP),
                })
                .collect(),
            chunks: (0..count)
                .map(|_| ChunkCache {
                    valid: false,
                    version: 0,
                    nodes: Vec::with_capacity(NODE_CAP),
                    matrix: Vec::with_capacity(NODE_CAP * NODE_CAP),
                })
                .collect(),
            chunk_ids,
            local: Local::new(),
            g: vec![0; len],
            parent: vec![0; len],
            seen: vec![0; len],
            closed: vec![0; len],
            generation: 0,
            heap: BinaryHeap::with_capacity(open_capacity as usize * 8 + 64),
            start_costs: Vec::with_capacity(NODE_CAP),
            goal_costs: Vec::with_capacity(NODE_CAP),
            scratch_nodes: Vec::with_capacity(NODE_CAP * 2),
            route: Vec::with_capacity(4096),
            edges: Vec::with_capacity(NODE_CAP + 8),
            use_cache: true,
            stats: HpaStats::default(),
        }
    }

    fn cell(&self, p: Pos3) -> (i32, i32, i32) {
        (p.x / CHUNK_X, p.y / CHUNK_Y, p.z / CHUNK_Z)
    }

    fn cell_index(&self, (x, y, z): (i32, i32, i32)) -> Option<usize> {
        ((0..self.columns).contains(&x)
            && (0..self.rows).contains(&y)
            && (0..self.layers).contains(&z))
        .then(|| ((z * self.rows + y) * self.columns + x) as usize)
    }

    fn bounds(&self, grid: &ChunkedGrid3<Tile>, cell: usize) -> (Pos3, Pos3) {
        grid.chunk_bounds(self.chunk_ids[cell])
            .unwrap_or((Pos3::new(0, 0, 0), Pos3::new(0, 0, 0)))
    }

    fn invalidate_all(&mut self) {
        for face in &mut self.faces {
            face.valid = false;
        }
        for chunk in &mut self.chunks {
            chunk.valid = false;
        }
    }

    /// Face toward the `+axis` neighbor of `cell`.
    fn ensure_face(&mut self, grid: &ChunkedGrid3<Tile>, cell: usize, axis: usize) {
        let (cx, cy, cz) = (
            cell as i32 % self.columns,
            cell as i32 / self.columns % self.rows,
            cell as i32 / (self.columns * self.rows),
        );
        let next = match axis {
            0 => (cx + 1, cy, cz),
            1 => (cx, cy + 1, cz),
            _ => (cx, cy, cz + 1),
        };
        let face_index = cell * 3 + axis;
        let Some(other) = self.cell_index(next) else {
            self.faces[face_index].entrances.clear();
            self.faces[face_index].valid = true;
            return;
        };
        let versions = (
            grid.chunk_version(self.chunk_ids[cell]),
            grid.chunk_version(self.chunk_ids[other]),
        );
        let face = &self.faces[face_index];
        if face.valid && face.versions == versions {
            return;
        }
        self.stats.face_rebuilds += 1;
        let (low, high) = self.bounds(grid, cell);
        let standable_at = |p: Pos3| grid.get(p).is_some_and(|t| standable(*t));
        let entrances = &mut self.faces[face_index].entrances;
        entrances.clear();
        if axis < 2 {
            let edge = if axis == 0 { high.x - 1 } else { high.y - 1 };
            let span = if axis == 0 {
                low.y..high.y
            } else {
                low.x..high.x
            };
            for z in low.z..high.z {
                let mut run: Option<(i32, i32)> = None;
                for s in span.clone().chain(std::iter::once(i32::MIN)) {
                    let crossing = s != i32::MIN && {
                        let (a, b) = if axis == 0 {
                            (Pos3::new(edge, s, z), Pos3::new(edge + 1, s, z))
                        } else {
                            (Pos3::new(s, edge, z), Pos3::new(s, edge + 1, z))
                        };
                        standable_at(a) && standable_at(b)
                    };
                    match (crossing, run) {
                        (true, None) => run = Some((s, s)),
                        (true, Some((first, _))) => run = Some((first, s)),
                        (false, Some((first, last))) => {
                            let mid = first + (last - first) / 2;
                            let (a, b) = if axis == 0 {
                                (Pos3::new(edge, mid, z), Pos3::new(edge + 1, mid, z))
                            } else {
                                (Pos3::new(mid, edge, z), Pos3::new(mid, edge + 1, z))
                            };
                            if entrances.len() < FACE_CAP {
                                entrances.push((a, b, ORTHOGONAL_COST));
                            }
                            run = None;
                        }
                        (false, None) => {}
                    }
                }
            }
        } else {
            let z = high.z - 1;
            for y in low.y..high.y {
                for x in low.x..high.x {
                    let (a, b) = (Pos3::new(x, y, z), Pos3::new(x, y, z + 1));
                    let up = grid.get(a).is_some_and(|t| {
                        matches!(t.kind, TileKind::StairUp | TileKind::StairUpDown)
                    });
                    let down = grid.get(b).is_some_and(|t| {
                        matches!(t.kind, TileKind::StairDown | TileKind::StairUpDown)
                    });
                    if up && down && entrances.len() < FACE_CAP {
                        entrances.push((a, b, VERTICAL_COST));
                    }
                }
            }
        }
        let face = &mut self.faces[face_index];
        face.valid = true;
        face.versions = versions;
    }

    /// Visits the six faces of a cell as `(face index, cell is the low side)`.
    fn faces_of(&self, cell: usize, mut visit: impl FnMut(usize, bool)) {
        let (cx, cy, cz) = (
            cell as i32 % self.columns,
            cell as i32 / self.columns % self.rows,
            cell as i32 / (self.columns * self.rows),
        );
        for axis in 0..3 {
            visit(cell * 3 + axis, true);
            let previous = match axis {
                0 => (cx - 1, cy, cz),
                1 => (cx, cy - 1, cz),
                _ => (cx, cy, cz - 1),
            };
            if let Some(other) = self.cell_index(previous) {
                visit(other * 3 + axis, false);
            }
        }
    }

    fn ensure_chunk(&mut self, grid: &ChunkedGrid3<Tile>, cell: usize) {
        let mut faces = [(usize::MAX, false); 6];
        let mut count = 0;
        self.faces_of(cell, |face, low| {
            faces[count] = (face, low);
            count += 1;
        });
        for &(face, _) in &faces[..count] {
            self.ensure_face(grid, face / 3, face % 3);
        }
        self.scratch_nodes.clear();
        for &(face, low) in &faces[..count] {
            for &(a, b, _) in &self.faces[face].entrances {
                self.scratch_nodes.push(if low { a } else { b });
            }
        }
        self.scratch_nodes.sort_unstable();
        self.scratch_nodes.dedup();
        self.scratch_nodes.truncate(NODE_CAP);
        let version = grid.chunk_version(self.chunk_ids[cell]);
        let cache = &self.chunks[cell];
        if cache.valid && cache.version == version && cache.nodes == self.scratch_nodes {
            self.stats.chunk_hits += 1;
            return;
        }
        self.stats.chunk_rebuilds += 1;
        let bounds = self.bounds(grid, cell);
        let cache = &mut self.chunks[cell];
        cache.nodes.clear();
        cache.nodes.extend_from_slice(&self.scratch_nodes);
        let n = cache.nodes.len();
        cache.matrix.clear();
        cache.matrix.resize(n * n, NO_EDGE);
        for i in 0..n {
            let from = self.chunks[cell].nodes[i];
            self.local.run(grid, bounds, from, None);
            for j in 0..n {
                let to = self.chunks[cell].nodes[j];
                if let Some(cost) = self.local.cost(to) {
                    self.chunks[cell].matrix[i * n + j] = u16::try_from(cost).unwrap_or(NO_EDGE);
                }
            }
        }
        let cache = &mut self.chunks[cell];
        cache.valid = true;
        cache.version = version;
    }

    /// Abstract neighbors of an entrance tile: intra-chunk edges, then face crossings.
    fn entrance_edges(&mut self, grid: &ChunkedGrid3<Tile>, node: Pos3) {
        let Some(cell) = self.cell_index(self.cell(node)) else {
            return;
        };
        self.ensure_chunk(grid, cell);
        let cache = &self.chunks[cell];
        if let Ok(i) = cache.nodes.binary_search(&node) {
            let n = cache.nodes.len();
            for j in 0..n {
                let cost = cache.matrix[i * n + j];
                if j != i && cost != NO_EDGE {
                    self.edges.push((cache.nodes[j], u32::from(cost)));
                }
            }
        }
        let mut faces = [(usize::MAX, false); 6];
        let mut count = 0;
        self.faces_of(cell, |face, low| {
            faces[count] = (face, low);
            count += 1;
        });
        for &(face, low) in &faces[..count] {
            for &(a, b, cost) in &self.faces[face].entrances {
                if low && a == node {
                    self.edges.push((b, cost));
                } else if !low && b == node {
                    self.edges.push((a, cost));
                }
            }
        }
    }

    /// Searches with at most `limit` abstract and local expansions. The path runs
    /// from start to goal inclusive. Refinement is not limited by `limit`, as the
    /// abstract costs already guarantee its success.
    pub fn search(
        &mut self,
        grid: &ChunkedGrid3<Tile>,
        start: Pos3,
        goal: Pos3,
        limit: u32,
        path: &mut Vec<Pos3>,
    ) -> SearchOutcome {
        path.clear();
        self.stats.searches += 1;
        if !self.use_cache {
            self.invalidate_all();
        }
        let unreachable = |expansions| SearchOutcome {
            result: PathResult::Unreachable,
            expansions,
        };
        let (Some(start_index), Some(goal_index)) = (grid.index(start), grid.index(goal)) else {
            return unreachable(0);
        };
        if !grid.get(start).is_some_and(|t| standable(*t))
            || !grid.get(goal).is_some_and(|t| standable(*t))
        {
            return unreachable(0);
        }
        if start == goal {
            path.push(start);
            return SearchOutcome {
                result: PathResult::Found { cost: 0 },
                expansions: 0,
            };
        }
        let (Some(start_cell), Some(goal_cell)) = (
            self.cell_index(self.cell(start)),
            self.cell_index(self.cell(goal)),
        ) else {
            return unreachable(0);
        };
        let mut expansions: u64 = 0;
        if start_cell == goal_cell {
            let bounds = self.bounds(grid, start_cell);
            expansions += self.local.run(grid, bounds, start, Some(goal));
            if let Some(cost) = self.local.cost(goal) {
                self.stats.local_hits += 1;
                self.stats.local_expansions += expansions;
                path.push(start);
                self.local.append_path(goal, path);
                return SearchOutcome {
                    result: PathResult::Found { cost },
                    expansions: expansions as u32,
                };
            }
        }
        self.ensure_chunk(grid, start_cell);
        self.ensure_chunk(grid, goal_cell);
        let bounds = self.bounds(grid, start_cell);
        expansions += self.local.run(grid, bounds, start, None);
        self.start_costs.clear();
        for &node in &self.chunks[start_cell].nodes {
            if let Some(cost) = self.local.cost(node) {
                self.start_costs.push((node, cost));
            }
        }
        let bounds = self.bounds(grid, goal_cell);
        expansions += self.local.run(grid, bounds, goal, None);
        self.goal_costs.clear();
        for &node in &self.chunks[goal_cell].nodes {
            if let Some(cost) = self.local.cost(node) {
                self.goal_costs.push((node, cost));
            }
        }
        self.stats.local_expansions += expansions;

        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.seen.fill(0);
                self.closed.fill(0);
                1
            }
        };
        let generation = self.generation;
        self.heap.clear();
        self.g[start_index] = 0;
        self.parent[start_index] = start_index as u32;
        self.seen[start_index] = generation;
        let h = layered_heuristic(
            start.x.abs_diff(goal.x),
            start.y.abs_diff(goal.y),
            start.z.abs_diff(goal.z),
        );
        self.heap.push(Reverse((h, h, start)));
        let mut abstract_expansions = 0u64;
        let mut found = false;
        while let Some(Reverse((_, _, node))) = self.heap.pop() {
            let Some(index) = grid.index(node) else {
                continue;
            };
            if self.closed[index] == generation {
                continue;
            }
            if node == goal {
                found = true;
                break;
            }
            if expansions + abstract_expansions >= u64::from(limit) {
                self.stats.abstract_expansions += abstract_expansions;
                return SearchOutcome {
                    result: PathResult::BudgetExceeded,
                    expansions: (expansions + abstract_expansions).min(u64::from(u32::MAX)) as u32,
                };
            }
            self.closed[index] = generation;
            abstract_expansions += 1;
            self.edges.clear();
            if node == start {
                self.edges.extend_from_slice(&self.start_costs);
            }
            self.entrance_edges(grid, node);
            if self.cell_index(self.cell(node)) == Some(goal_cell)
                && let Ok(at) = self.goal_costs.binary_search_by(|e| e.0.cmp(&node))
            {
                self.edges.push((goal, self.goal_costs[at].1));
            }
            let base = self.g[index];
            for i in 0..self.edges.len() {
                let (next, step) = self.edges[i];
                let Some(j) = grid.index(next) else { continue };
                if self.closed[j] == generation {
                    continue;
                }
                let candidate = base.saturating_add(step);
                if self.seen[j] != generation || candidate < self.g[j] {
                    self.seen[j] = generation;
                    self.g[j] = candidate;
                    self.parent[j] = index as u32;
                    let h = layered_heuristic(
                        next.x.abs_diff(goal.x),
                        next.y.abs_diff(goal.y),
                        next.z.abs_diff(goal.z),
                    );
                    self.heap
                        .push(Reverse((candidate.saturating_add(h), h, next)));
                }
            }
        }
        self.stats.abstract_expansions += abstract_expansions;
        let total = (expansions + abstract_expansions).min(u64::from(u32::MAX)) as u32;
        if !found {
            return unreachable(total);
        }
        let cost = self.g[goal_index];
        self.route.clear();
        let mut index = goal_index;
        while index != start_index {
            self.route.push(grid.pos(index).unwrap_or(start));
            index = self.parent[index] as usize;
        }
        self.route.push(start);
        self.route.reverse();
        path.push(start);
        for i in 1..self.route.len() {
            let (from, to) = (self.route[i - 1], self.route[i]);
            let same = self.cell(from) == self.cell(to);
            if same {
                let cell = self.cell_index(self.cell(from)).unwrap_or(0);
                let bounds = self.bounds(grid, cell);
                self.stats.refine_expansions += self.local.run(grid, bounds, from, Some(to));
                self.local.append_path(to, path);
            } else {
                path.push(to);
            }
        }
        SearchOutcome {
            result: PathResult::Found { cost },
            expansions: total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        path::AStar,
        rng::{SystemRng, below},
        topology::{LayeredWalkTopology, Topology},
    };

    fn world(seed: u64) -> ChunkedGrid3<Tile> {
        let mut rng = SystemRng::new(seed, 61);
        let (w, h, d) = (40, 36, 12);
        let mut grid = ChunkedGrid3::new(w, h, d, Tile::new(TileKind::Solid, false, 0)).unwrap();
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    let roll = below(rng.next_u64(), 100);
                    let tile = match roll {
                        0..22 => Tile::new(TileKind::Solid, false, 0),
                        22..88 => Tile::new(TileKind::Open, true, 0),
                        88..91 => Tile::new(TileKind::Open, false, 0),
                        91..99 => Tile::new(TileKind::StairUpDown, false, 0),
                        _ => Tile::new(TileKind::Ramp, false, 0),
                    };
                    grid.set(Pos3::new(x, y, z), tile);
                }
            }
        }
        grid.clear_changes();
        grid
    }

    fn valid_path(grid: &ChunkedGrid3<Tile>, path: &[Pos3], cost: u32) {
        let topology = LayeredWalkTopology::new(grid);
        let mut out = Vec::new();
        let mut total = 0;
        for pair in path.windows(2) {
            topology.neighbors(pair[0], &mut out);
            total += out
                .iter()
                .find(|(n, _)| *n == pair[1])
                .map(|(_, c)| *c)
                .expect("path steps are walk moves");
        }
        assert_eq!(total, cost);
    }

    #[test]
    fn cached_and_uncached_searches_return_identical_paths_after_changes() {
        for seed in 0..3 {
            let mut grid = world(seed);
            let mut cached = HierarchicalPathfinder::new(&grid, 50_000);
            let mut fresh = HierarchicalPathfinder::new(&grid, 50_000);
            fresh.use_cache = false;
            let mut astar = AStar::with_capacity(grid.len(), 1024);
            let mut rng = SystemRng::new(seed, 62);
            let standing: Vec<Pos3> = grid
                .iter_logical()
                .filter(|(_, t)| standable(**t))
                .map(|(p, _)| p)
                .collect();
            let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
            let mut found = 0;
            for round in 0..12 {
                for _ in 0..12 {
                    let start = standing[below(rng.next_u64(), standing.len() as u64) as usize];
                    let goal = standing[below(rng.next_u64(), standing.len() as u64) as usize];
                    let x = cached.search(&grid, start, goal, 200_000, &mut a);
                    let y = fresh.search(&grid, start, goal, 200_000, &mut b);
                    assert_eq!(x, y, "round {round}: {start:?} -> {goal:?}");
                    assert_eq!(a, b);
                    if let PathResult::Found { cost } = x.result {
                        found += 1;
                        valid_path(&grid, &a, cost);
                        let reference = astar.search(
                            &LayeredWalkTopology::new(&grid),
                            start,
                            goal,
                            u32::MAX,
                            &mut c,
                        );
                        let PathResult::Found { cost: optimal } = reference.result else {
                            panic!("A* must find what HPA finds");
                        };
                        assert!(cost >= optimal);
                    }
                }
                for _ in 0..15 {
                    let p = Pos3::new(
                        below(rng.next_u64(), 40) as i32,
                        below(rng.next_u64(), 36) as i32,
                        below(rng.next_u64(), 12) as i32,
                    );
                    let tile = match below(rng.next_u64(), 3) {
                        0 => Tile::new(TileKind::Solid, false, 1),
                        1 => Tile::new(TileKind::Open, true, 1),
                        _ => Tile::new(TileKind::StairUpDown, false, 1),
                    };
                    grid.set(p, tile);
                }
            }
            assert!(found > 30, "seed {seed} found {found}");
            assert!(cached.stats.chunk_hits > 0 && cached.stats.chunk_rebuilds > 0);
        }
    }

    #[test]
    fn open_floor_paths_cross_chunks_with_near_optimal_cost() {
        let grid = ChunkedGrid3::new(64, 64, 1, Tile::new(TileKind::Open, true, 0)).unwrap();
        let mut hpa = HierarchicalPathfinder::new(&grid, 50_000);
        let mut path = Vec::new();
        let outcome = hpa.search(
            &grid,
            Pos3::new(1, 2, 0),
            Pos3::new(60, 50, 0),
            100_000,
            &mut path,
        );
        let PathResult::Found { cost } = outcome.result else {
            panic!("open floor must be traversable");
        };
        valid_path(&grid, &path, cost);
        let optimal = layered_heuristic(59, 48, 0);
        assert!(
            cost >= optimal && cost <= optimal * 13 / 10,
            "{cost} vs {optimal}"
        );
        let limited = hpa.search(
            &grid,
            Pos3::new(1, 2, 0),
            Pos3::new(60, 50, 0),
            10,
            &mut path,
        );
        assert_eq!(limited.result, PathResult::BudgetExceeded);
    }
}
