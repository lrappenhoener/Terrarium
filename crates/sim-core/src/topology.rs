//! Search graphs with integer costs, deterministic neighbor order and dense indices.

use crate::space::{
    ChunkedGrid3, DIAGONAL_COST, Grid2, ORTHOGONAL_COST, Point, Pos3, Tile, VERTICAL_COST,
    walk_neighbors,
};

pub trait Topology {
    type Node: Copy + Eq + Ord + core::fmt::Debug;
    /// Replaces `out` with the neighbors and integer costs in deterministic order.
    fn neighbors(&self, node: Self::Node, out: &mut Vec<(Self::Node, u32)>);
    /// Admissible (never overestimating) integer estimate.
    fn heuristic(&self, from: Self::Node, to: Self::Node) -> u32;
    /// Dense index below `node_count` for node-indexed arrays.
    fn node_index(&self, node: Self::Node) -> usize;
    fn node_count(&self) -> usize;
}

/// Octile distance with costs 10 and 14.
pub fn octile(dx: u32, dy: u32) -> u32 {
    let (low, high) = if dx < dy { (dx, dy) } else { (dy, dx) };
    ORTHOGONAL_COST
        .saturating_mul(high)
        .saturating_add((DIAGONAL_COST - ORTHOGONAL_COST).saturating_mul(low))
}

/// Lower bound for layered walking. Every level change costs 20, and a ramp can
/// cover one orthogonal step (worth at most 10 of the octile bound) at the same time.
/// The plain sum `octile + 20 * dz` would overestimate ramp moves.
pub fn layered_heuristic(dx: u32, dy: u32, dz: u32) -> u32 {
    VERTICAL_COST
        .saturating_mul(dz)
        .saturating_add(octile(dx, dy).saturating_sub(ORTHOGONAL_COST.saturating_mul(dz)))
}

/// Walking over a layered tile grid with the rules of `space::walk`.
pub struct LayeredWalkTopology<'a> {
    pub grid: &'a ChunkedGrid3<Tile>,
}

impl<'a> LayeredWalkTopology<'a> {
    pub fn new(grid: &'a ChunkedGrid3<Tile>) -> Self {
        Self { grid }
    }
}

impl Topology for LayeredWalkTopology<'_> {
    type Node = Pos3;

    fn neighbors(&self, node: Pos3, out: &mut Vec<(Pos3, u32)>) {
        out.clear();
        walk_neighbors(self.grid, node, out);
    }

    fn heuristic(&self, from: Pos3, to: Pos3) -> u32 {
        layered_heuristic(
            from.x.abs_diff(to.x),
            from.y.abs_diff(to.y),
            from.z.abs_diff(to.z),
        )
    }

    fn node_index(&self, node: Pos3) -> usize {
        self.grid.index(node).unwrap_or(usize::MAX)
    }

    fn node_count(&self) -> usize {
        self.grid.len()
    }
}

/// Passable cells of a 2D grid. Diagonal moves never cut blocked corners.
pub struct Grid2Topology<'a> {
    pub passable: &'a Grid2<bool>,
    pub diagonal: bool,
}

impl Grid2Topology<'_> {
    fn open(&self, point: Point) -> bool {
        self.passable.get(point).copied().unwrap_or(false)
    }
}

impl Topology for Grid2Topology<'_> {
    type Node = Point;

    fn neighbors(&self, node: Point, out: &mut Vec<(Point, u32)>) {
        out.clear();
        if !self.open(node) {
            return;
        }
        let at = |dx: i32, dy: i32| Point {
            x: node.x.wrapping_add(dx),
            y: node.y.wrapping_add(dy),
        };
        for (dx, dy) in crate::space::ORTHOGONAL {
            if self.open(at(dx, dy)) {
                out.push((at(dx, dy), ORTHOGONAL_COST));
            }
        }
        if self.diagonal {
            for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                if self.open(at(dx, dy)) && self.open(at(dx, 0)) && self.open(at(0, dy)) {
                    out.push((at(dx, dy), DIAGONAL_COST));
                }
            }
        }
    }

    fn heuristic(&self, from: Point, to: Point) -> u32 {
        let dx = from.x.abs_diff(to.x);
        let dy = from.y.abs_diff(to.y);
        if self.diagonal {
            octile(dx, dy)
        } else {
            ORTHOGONAL_COST.saturating_mul(dx.saturating_add(dy))
        }
    }

    fn node_index(&self, node: Point) -> usize {
        self.passable.index(node).unwrap_or(usize::MAX)
    }

    fn node_count(&self) -> usize {
        self.passable.as_slice().len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("edge endpoint {0} is outside the graph")]
    NodeOutOfRange(u32),
    #[error("position count differs from node count")]
    PositionCount,
    #[error("edge {from}->{to} is cheaper than its scaled straight-line distance")]
    InadmissibleEdge { from: u32, to: u32 },
}

/// Directed graph in compressed sparse row form, such as a road network.
/// With positions, the heuristic is `scale * floor(euclidean distance)`, and every
/// edge must cost at least `scale` times its straight-line length.
pub struct GraphTopology {
    offsets: Vec<u32>,
    targets: Vec<u32>,
    costs: Vec<u32>,
    positions: Vec<Point>,
    scale: u32,
}

impl GraphTopology {
    pub fn new(node_count: u32, edges: &[(u32, u32, u32)]) -> Result<Self, GraphError> {
        Self::build(node_count, edges, Vec::new(), 0)
    }

    pub fn with_positions(
        edges: &[(u32, u32, u32)],
        positions: Vec<Point>,
        scale: u32,
    ) -> Result<Self, GraphError> {
        let count = u32::try_from(positions.len()).map_err(|_| GraphError::PositionCount)?;
        Self::build(count, edges, positions, scale)
    }

    fn build(
        node_count: u32,
        edges: &[(u32, u32, u32)],
        positions: Vec<Point>,
        scale: u32,
    ) -> Result<Self, GraphError> {
        if !positions.is_empty() && positions.len() != node_count as usize {
            return Err(GraphError::PositionCount);
        }
        let mut sorted = edges.to_vec();
        sorted.sort_unstable();
        let mut offsets = vec![0u32; node_count as usize + 1];
        for &(from, to, cost) in &sorted {
            for node in [from, to] {
                if node >= node_count {
                    return Err(GraphError::NodeOutOfRange(node));
                }
            }
            if let (Some(a), Some(b)) = (positions.get(from as usize), positions.get(to as usize)) {
                let required = u128::from(scale).pow(2) * a.distance_squared(*b);
                if u128::from(cost).pow(2) < required {
                    return Err(GraphError::InadmissibleEdge { from, to });
                }
            }
            offsets[from as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        Ok(Self {
            offsets,
            targets: sorted.iter().map(|&(_, to, _)| to).collect(),
            costs: sorted.iter().map(|&(_, _, cost)| cost).collect(),
            positions,
            scale,
        })
    }
}

impl Topology for GraphTopology {
    type Node = u32;

    fn neighbors(&self, node: u32, out: &mut Vec<(u32, u32)>) {
        out.clear();
        let Some(range) = self
            .offsets
            .get(node as usize)
            .zip(self.offsets.get(node as usize + 1))
        else {
            return;
        };
        let range = *range.0 as usize..*range.1 as usize;
        out.extend(
            self.targets[range.clone()]
                .iter()
                .copied()
                .zip(self.costs[range].iter().copied()),
        );
    }

    fn heuristic(&self, from: u32, to: u32) -> u32 {
        match (
            self.positions.get(from as usize),
            self.positions.get(to as usize),
        ) {
            (Some(a), Some(b)) => {
                let root = a.distance_squared(*b).isqrt();
                u32::try_from(root.saturating_mul(u128::from(self.scale))).unwrap_or(u32::MAX)
            }
            _ => 0,
        }
    }

    fn node_index(&self, node: u32) -> usize {
        node as usize
    }

    fn node_count(&self) -> usize {
        self.offsets.len() - 1
    }
}

/// Unbounded reference Dijkstra for tests only.
#[cfg(test)]
pub(crate) fn dijkstra<T: Topology>(topology: &T, start: T::Node) -> Vec<u32> {
    use std::{cmp::Reverse, collections::BinaryHeap};
    let mut dist = vec![u32::MAX; topology.node_count()];
    let mut heap = BinaryHeap::new();
    let mut out = Vec::new();
    dist[topology.node_index(start)] = 0;
    heap.push(Reverse((0u32, start)));
    while let Some(Reverse((cost, node))) = heap.pop() {
        if cost > dist[topology.node_index(node)] {
            continue;
        }
        topology.neighbors(node, &mut out);
        for &(next, step) in &out {
            let candidate = cost + step;
            let slot = &mut dist[topology.node_index(next)];
            if candidate < *slot {
                *slot = candidate;
                heap.push(Reverse((candidate, next)));
            }
        }
    }
    dist
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        rng::{SystemRng, below},
        space::TileKind,
    };

    pub(crate) fn random_layered(seed: u64, w: i32, h: i32, d: i32) -> ChunkedGrid3<Tile> {
        use TileKind::*;
        let mut rng = SystemRng::new(seed, 21);
        let mut grid = ChunkedGrid3::new(w, h, d, Tile::new(Solid, false, 0)).unwrap();
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    let roll = below(rng.next_u64(), 100);
                    let kind = match roll {
                        0..30 => Solid,
                        30..75 => Open,
                        75..80 => StairUp,
                        80..85 => StairDown,
                        85..92 => StairUpDown,
                        92..97 => Ramp,
                        _ => Door,
                    };
                    let floor = below(rng.next_u64(), 10) < 8;
                    grid.set(Pos3::new(x, y, z), Tile::new(kind, floor, 0));
                }
            }
        }
        grid
    }

    fn check_admissible<T: Topology>(topology: &T, nodes: &[T::Node]) -> usize {
        let mut checked = 0;
        let mut first = Vec::new();
        let mut second = Vec::new();
        for &node in nodes {
            topology.neighbors(node, &mut first);
            topology.neighbors(node, &mut second);
            assert_eq!(first, second);
        }
        for &source in nodes.iter().step_by(7) {
            let dist = dijkstra(topology, source);
            for &target in nodes {
                let d = dist[topology.node_index(target)];
                if d != u32::MAX {
                    assert!(
                        topology.heuristic(source, target) <= d,
                        "h({source:?}, {target:?}) = {} > {d}",
                        topology.heuristic(source, target)
                    );
                    checked += 1;
                }
            }
        }
        checked
    }

    #[test]
    fn layered_heuristic_never_overestimates() {
        for seed in 0..12 {
            let grid = random_layered(seed, 9, 8, 5);
            let topology = LayeredWalkTopology::new(&grid);
            let nodes: Vec<_> = grid.iter_logical().map(|(p, _)| p).collect();
            assert!(check_admissible(&topology, &nodes) > 100);
        }
    }

    #[test]
    fn the_specified_sum_heuristic_overestimates_ramps() {
        let mut grid = ChunkedGrid3::new(2, 1, 2, Tile::new(TileKind::Open, false, 0)).unwrap();
        grid.set(Pos3::new(0, 0, 0), Tile::new(TileKind::Ramp, false, 0));
        grid.set(Pos3::new(1, 0, 1), Tile::new(TileKind::Open, true, 0));
        let topology = LayeredWalkTopology::new(&grid);
        let dist = dijkstra(&topology, Pos3::new(0, 0, 0));
        let actual = dist[grid.index(Pos3::new(1, 0, 1)).unwrap()];
        assert_eq!(actual, 20);
        assert_eq!(octile(1, 0) + 20, 30);
        assert_eq!(layered_heuristic(1, 0, 1), 20);
    }

    #[test]
    fn grid2_heuristics_never_overestimate() {
        for seed in 0..8 {
            let mut rng = SystemRng::new(seed, 22);
            let mut passable = Grid2::new(13, 11, false).unwrap();
            for value in passable.as_mut_slice() {
                *value = below(rng.next_u64(), 10) < 7;
            }
            let nodes: Vec<_> = (0..11)
                .flat_map(|y| (0..13).map(move |x| Point { x, y }))
                .collect();
            for diagonal in [false, true] {
                let topology = Grid2Topology {
                    passable: &passable,
                    diagonal,
                };
                assert!(check_admissible(&topology, &nodes) > 50);
            }
        }
    }

    #[test]
    fn graph_heuristic_never_overestimates_and_neighbors_are_sorted() {
        for seed in 0..8 {
            let mut rng = SystemRng::new(seed, 23);
            let positions: Vec<_> = (0..30)
                .map(|_| Point {
                    x: below(rng.next_u64(), 50) as i32,
                    y: below(rng.next_u64(), 50) as i32,
                })
                .collect();
            let mut edges = Vec::new();
            for _ in 0..90 {
                let a = below(rng.next_u64(), 30) as u32;
                let b = below(rng.next_u64(), 30) as u32;
                let length = positions[a as usize]
                    .distance_squared(positions[b as usize])
                    .isqrt() as u32
                    + 1;
                edges.push((a, b, 3 * length + below(rng.next_u64(), 5) as u32));
            }
            let mut shuffled = edges.clone();
            shuffled.reverse();
            let graph = GraphTopology::with_positions(&edges, positions.clone(), 3).unwrap();
            let other = GraphTopology::with_positions(&shuffled, positions, 3).unwrap();
            let nodes: Vec<u32> = (0..30).collect();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for &node in &nodes {
                graph.neighbors(node, &mut a);
                other.neighbors(node, &mut b);
                assert_eq!(a, b);
                assert!(a.windows(2).all(|pair| pair[0] <= pair[1]));
            }
            check_admissible(&graph, &nodes);
        }
        let positions = vec![Point { x: 0, y: 0 }, Point { x: 10, y: 0 }];
        assert_eq!(
            GraphTopology::with_positions(&[(0, 1, 5)], positions, 1).err(),
            Some(GraphError::InadmissibleEdge { from: 0, to: 1 })
        );
        assert_eq!(
            GraphTopology::new(2, &[(0, 2, 1)]).err(),
            Some(GraphError::NodeOutOfRange(2))
        );
    }
}
