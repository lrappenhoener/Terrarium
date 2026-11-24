//! Connected regions of standable tiles with targeted updates.
//!
//! Region IDs are derived data. They are neither hashed nor saved and are rebuilt
//! after loading. Behavior may only depend on `reachable`, never on ID values.

use crate::space::{ChunkId, ChunkedGrid3, Pos3, Tile, TileSource, standable, walk_neighbors};

const NONE: u32 = u32::MAX;

/// Diagnostic counters, excluded from state hashes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectivityStats {
    /// Updates that found at least one changed tile.
    pub updates: u64,
    pub changed_tiles: u64,
    pub merges: u64,
    pub flood_fills: u64,
    pub flood_tiles: u64,
    pub full_rebuilds: u64,
}

struct Cached<'a> {
    tiles: &'a [Tile],
    width: i32,
    height: i32,
    depth: i32,
}

impl TileSource for Cached<'_> {
    fn tile(&self, p: Pos3) -> Option<Tile> {
        ((0..self.width).contains(&p.x)
            && (0..self.height).contains(&p.y)
            && (0..self.depth).contains(&p.z))
        .then(|| {
            self.tiles[(p.z as usize * self.height as usize + p.y as usize) * self.width as usize
                + p.x as usize]
        })
    }
}

/// Region labels of a layered walk world. `reachable` is a constant-time comparison:
/// union-find parents are flattened after each update.
pub struct Connectivity {
    width: i32,
    height: i32,
    depth: i32,
    region: Vec<u32>,
    parent: Vec<u32>,
    id_limit: usize,
    cache: Vec<Tile>,
    stamp: Vec<u32>,
    generation: u32,
    queue: Vec<u32>,
    seeds: Vec<u32>,
    changed: Vec<u32>,
    affected: Vec<u32>,
    added: Vec<(u32, u32)>,
    old_edges: Vec<(Pos3, u32)>,
    new_edges: Vec<(Pos3, u32)>,
    pub stats: ConnectivityStats,
}

impl Connectivity {
    pub fn new(grid: &ChunkedGrid3<Tile>) -> Self {
        let len = grid.len();
        let id_limit = len / 2 + 1024;
        let mut result = Self {
            width: grid.width(),
            height: grid.height(),
            depth: grid.depth(),
            region: vec![NONE; len],
            parent: Vec::with_capacity(id_limit + 1),
            id_limit,
            cache: grid.iter_logical().map(|(_, tile)| *tile).collect(),
            stamp: vec![0; len],
            generation: 0,
            queue: Vec::with_capacity(len),
            seeds: Vec::with_capacity(1024),
            changed: Vec::with_capacity(4096),
            affected: Vec::with_capacity(4096),
            added: Vec::with_capacity(4096),
            old_edges: Vec::with_capacity(32),
            new_edges: Vec::with_capacity(32),
            stats: ConnectivityStats::default(),
        };
        result.rebuild(grid);
        result
    }

    fn index(&self, p: Pos3) -> Option<usize> {
        ((0..self.width).contains(&p.x)
            && (0..self.height).contains(&p.y)
            && (0..self.depth).contains(&p.z))
        .then(|| {
            (p.z as usize * self.height as usize + p.y as usize) * self.width as usize
                + p.x as usize
        })
    }

    fn pos(&self, index: usize) -> Pos3 {
        let width = self.width as usize;
        let height = self.height as usize;
        Pos3::new(
            (index % width) as i32,
            (index / width % height) as i32,
            (index / (width * height)) as i32,
        )
    }

    /// Whether both positions are standable and connected by walking.
    pub fn reachable(&self, a: Pos3, b: Pos3) -> bool {
        let (Some(a), Some(b)) = (self.index(a), self.index(b)) else {
            return false;
        };
        let (ra, rb) = (self.region[a], self.region[b]);
        ra != NONE && rb != NONE && self.parent[ra as usize] == self.parent[rb as usize]
    }

    pub fn is_standable(&self, p: Pos3) -> bool {
        self.index(p).is_some_and(|i| self.region[i] != NONE)
    }

    /// Region label of a standable tile, for debug overlays only. Labels are
    /// derived, layout-dependent values; behavior must never depend on them.
    pub fn debug_region(&self, p: Pos3) -> Option<u32> {
        let region = self.region[self.index(p)?];
        (region != NONE).then(|| self.parent[region as usize])
    }

    /// Number of allocated region IDs, for diagnostics.
    pub fn id_count(&self) -> usize {
        self.parent.len()
    }

    fn next_generation(&mut self) -> u32 {
        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.stamp.fill(0);
                1
            }
        };
        self.generation
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        id
    }

    fn find(&mut self, mut id: u32) -> u32 {
        while self.parent[id as usize] != id {
            let grand = self.parent[self.parent[id as usize] as usize];
            self.parent[id as usize] = grand;
            id = grand;
        }
        id
    }

    fn union(&mut self, a: u32, b: u32) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[high as usize] = low;
        true
    }

    fn flatten(&mut self) {
        for id in 0..self.parent.len() as u32 {
            let root = self.find(id);
            self.parent[id as usize] = root;
        }
    }

    /// Labels the walk component of `seed` with a fresh ID. Returns visited tiles.
    fn flood(&mut self, grid: &ChunkedGrid3<Tile>, seed: u32, generation: u32) -> u64 {
        let id = self.fresh_id();
        self.queue.clear();
        self.queue.push(seed);
        self.stamp[seed as usize] = generation;
        let mut cursor = 0;
        while let Some(&index) = self.queue.get(cursor) {
            cursor += 1;
            self.region[index as usize] = id;
            self.new_edges.clear();
            walk_neighbors(grid, self.pos(index as usize), &mut self.new_edges);
            for &(next, _) in &self.new_edges {
                if let Some(j) = self.index(next)
                    && self.stamp[j] != generation
                {
                    self.stamp[j] = generation;
                    self.queue.push(j as u32);
                }
            }
        }
        cursor as u64
    }

    /// Recomputes all regions from scratch.
    pub fn rebuild(&mut self, grid: &ChunkedGrid3<Tile>) {
        self.stats.full_rebuilds += 1;
        self.parent.clear();
        self.region.fill(NONE);
        for (index, (_, tile)) in grid.iter_logical().enumerate() {
            self.cache[index] = *tile;
        }
        let generation = self.next_generation();
        for index in 0..self.region.len() {
            if self.stamp[index] != generation && standable(self.cache[index]) {
                self.flood(grid, index as u32, generation);
            }
        }
    }

    /// Applies the tile changes inside `chunks`, as reported by
    /// `ChunkedGrid3::take_changed_chunks`. Opening changes merge regions;
    /// closing changes flood-fill only the regions that lost an edge or a tile.
    pub fn update(&mut self, grid: &ChunkedGrid3<Tile>, chunks: &[ChunkId]) {
        self.changed.clear();
        for &chunk in chunks {
            grid.for_each_in_chunk(chunk, |p, tile| {
                let index = (p.z as usize * self.height as usize + p.y as usize)
                    * self.width as usize
                    + p.x as usize;
                if self.cache[index] != *tile {
                    self.changed.push(index as u32);
                }
            });
        }
        if self.changed.is_empty() {
            return;
        }
        self.stats.updates += 1;
        self.stats.changed_tiles += self.changed.len() as u64;

        // Every edge whose existence depends on a changed tile has an endpoint
        // within Chebyshev distance one of that tile.
        self.affected.clear();
        for i in 0..self.changed.len() {
            let p = self.pos(self.changed[i] as usize);
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(j) = self.index(p.offset(dx, dy, dz)) {
                            self.affected.push(j as u32);
                        }
                    }
                }
            }
        }
        self.affected.sort_unstable();
        self.affected.dedup();

        self.seeds.clear();
        self.added.clear();
        let cached = Cached {
            tiles: &self.cache,
            width: self.width,
            height: self.height,
            depth: self.depth,
        };
        for &index in &self.affected {
            let p = {
                let width = self.width as usize;
                let height = self.height as usize;
                let index = index as usize;
                Pos3::new(
                    (index % width) as i32,
                    (index / width % height) as i32,
                    (index / (width * height)) as i32,
                )
            };
            self.old_edges.clear();
            self.new_edges.clear();
            walk_neighbors(&cached, p, &mut self.old_edges);
            walk_neighbors(grid, p, &mut self.new_edges);
            let to_index = |q: Pos3| {
                (q.z as usize * self.height as usize + q.y as usize) * self.width as usize
                    + q.x as usize
            };
            for edge in &self.old_edges {
                if !self.new_edges.contains(edge) {
                    self.seeds.push(index);
                    self.seeds.push(to_index(edge.0) as u32);
                }
            }
            for edge in &self.new_edges {
                if !self.old_edges.contains(edge) {
                    self.added.push((index, to_index(edge.0) as u32));
                }
            }
        }
        for &index in &self.changed {
            let index = index as usize;
            self.cache[index] = grid.tile(self.pos(index)).unwrap_or(self.cache[index]);
        }
        for i in 0..self.changed.len() {
            let index = self.changed[i] as usize;
            let now = standable(self.cache[index]);
            if !now && self.region[index] != NONE {
                self.region[index] = NONE;
            } else if now && self.region[index] == NONE {
                self.region[index] = self.fresh_id();
            }
        }
        for i in 0..self.added.len() {
            let (a, b) = self.added[i];
            let (ra, rb) = (self.region[a as usize], self.region[b as usize]);
            if ra != NONE && rb != NONE && self.union(ra, rb) {
                self.stats.merges += 1;
            }
        }
        if !self.seeds.is_empty() {
            let generation = self.next_generation();
            for i in 0..self.seeds.len() {
                let seed = self.seeds[i];
                if self.stamp[seed as usize] != generation && standable(self.cache[seed as usize]) {
                    self.stats.flood_fills += 1;
                    self.stats.flood_tiles += self.flood(grid, seed, generation);
                }
            }
        }
        if self.parent.len() > self.id_limit {
            self.rebuild(grid);
        }
        self.flatten();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        rng::{SystemRng, below},
        space::TileKind,
    };

    /// Checks that both labelings induce the same partition of standable tiles.
    fn assert_equivalent(grid: &ChunkedGrid3<Tile>, incremental: &Connectivity) {
        let fresh = Connectivity::new(grid);
        let mut forward = std::collections::BTreeMap::new();
        let mut backward = std::collections::BTreeMap::new();
        for index in 0..grid.len() {
            let (a, b) = (incremental.region[index], fresh.region[index]);
            assert_eq!(a == NONE, b == NONE, "standability differs at {index}");
            if a == NONE {
                continue;
            }
            let (a, b) = (incremental.parent[a as usize], fresh.parent[b as usize]);
            assert_eq!(*forward.entry(a).or_insert(b), b, "split region at {index}");
            assert_eq!(
                *backward.entry(b).or_insert(a),
                a,
                "merged region at {index}"
            );
        }
    }

    fn mountain(seed: u64, w: i32, h: i32, d: i32) -> ChunkedGrid3<Tile> {
        let mut rng = SystemRng::new(seed, 41);
        let mut grid = ChunkedGrid3::new(w, h, d, Tile::new(TileKind::Solid, false, 1)).unwrap();
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    if z >= d / 2 || below(rng.next_u64(), 10) < 3 {
                        grid.set(Pos3::new(x, y, z), Tile::new(TileKind::Open, z == d / 2, 0));
                    }
                }
            }
        }
        grid.clear_changes();
        grid
    }

    #[test]
    fn targeted_updates_match_full_recomputation() {
        let mut operations = 0;
        for seed in [1, 2, 3, 42] {
            let (w, h, d) = (20, 18, 10);
            let mut grid = mountain(seed, w, h, d);
            let mut connectivity = Connectivity::new(&grid);
            let mut rng = SystemRng::new(seed, 42);
            let mut chunks = Vec::new();
            for _ in 0..400 {
                for _ in 0..1 + below(rng.next_u64(), 8) {
                    let p = Pos3::new(
                        below(rng.next_u64(), w as u64) as i32,
                        below(rng.next_u64(), h as u64) as i32,
                        below(rng.next_u64(), d as u64) as i32,
                    );
                    let tile = match below(rng.next_u64(), 10) {
                        0..4 => Tile::new(TileKind::Open, true, 0),
                        4..7 => Tile::new(TileKind::Solid, false, 2),
                        7..9 => Tile::new(TileKind::StairUpDown, false, 0),
                        _ => Tile::new(TileKind::Ramp, false, 0),
                    };
                    grid.set(p, tile);
                    operations += 1;
                }
                grid.take_changed_chunks(&mut chunks);
                connectivity.update(&grid, &chunks);
                assert_equivalent(&grid, &connectivity);
            }
            assert!(connectivity.stats.merges > 0 && connectivity.stats.flood_fills > 0);
        }
        assert!(operations > 5000);
    }

    #[test]
    fn walls_split_and_digging_rejoins_regions() {
        let mut grid = ChunkedGrid3::new(5, 1, 1, Tile::new(TileKind::Open, true, 0)).unwrap();
        let mut connectivity = Connectivity::new(&grid);
        let (a, b) = (Pos3::new(0, 0, 0), Pos3::new(4, 0, 0));
        let mut chunks = Vec::new();
        assert!(connectivity.reachable(a, b));
        grid.set(Pos3::new(2, 0, 0), Tile::new(TileKind::Solid, false, 3));
        grid.take_changed_chunks(&mut chunks);
        connectivity.update(&grid, &chunks);
        assert!(!connectivity.reachable(a, b));
        assert!(connectivity.reachable(a, Pos3::new(1, 0, 0)));
        assert!(!connectivity.reachable(a, Pos3::new(2, 0, 0)));
        grid.set(
            Pos3::new(2, 0, 0),
            crate::space::dig(Tile::new(TileKind::Solid, false, 3)),
        );
        grid.take_changed_chunks(&mut chunks);
        connectivity.update(&grid, &chunks);
        assert!(connectivity.reachable(a, b));
        assert_eq!(connectivity.stats.flood_fills, 2);
        assert_eq!(connectivity.stats.merges, 2);
    }
}
