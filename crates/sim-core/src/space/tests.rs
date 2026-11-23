use super::*;
use crate::rng::{SystemRng, below};

fn naive_morton(x: u32, y: u32, z: u32) -> u64 {
    let mut code = 0;
    for bit in 0..21 {
        code |= u64::from((x >> bit) & 1) << (3 * bit);
        code |= u64::from((y >> bit) & 1) << (3 * bit + 1);
        code |= u64::from((z >> bit) & 1) << (3 * bit + 2);
    }
    code
}

#[test]
fn morton3_interleaves_bits() {
    let mut rng = SystemRng::new(5, 1);
    for _ in 0..1000 {
        let [x, y, z] = [0; 3].map(|_| below(rng.next_u64(), 1 << 21) as u32);
        assert_eq!(morton3(x, y, z), naive_morton(x, y, z));
    }
}

#[test]
fn pos3_orders_by_level_row_column() {
    let mut points = vec![
        Pos3::new(5, 0, 1),
        Pos3::new(0, 1, 0),
        Pos3::new(1, 0, 0),
        Pos3::new(0, 0, 1),
    ];
    points.sort();
    assert_eq!(
        points,
        [
            Pos3::new(1, 0, 0),
            Pos3::new(0, 1, 0),
            Pos3::new(0, 0, 1),
            Pos3::new(5, 0, 1)
        ]
    );
}

#[test]
fn chunked_grid3_matches_a_flat_model_and_tracks_changes() {
    let (w, h, d) = (37, 21, 11);
    for seed in [1, 2, 42] {
        let mut rng = SystemRng::new(seed, 3);
        let mut grid = ChunkedGrid3::new(w, h, d, 0u32).unwrap();
        let mut flat = vec![0u32; (w * h * d) as usize];
        let mut changed = std::collections::BTreeSet::new();
        let mut versions = vec![0u32; grid.chunk_count()];
        for step in 1..4000u32 {
            let p = Pos3::new(
                below(rng.next_u64(), (w + 2) as u64) as i32 - 1,
                below(rng.next_u64(), (h + 2) as u64) as i32 - 1,
                below(rng.next_u64(), (d + 2) as u64) as i32 - 1,
            );
            let inside = (0..w).contains(&p.x) && (0..h).contains(&p.y) && (0..d).contains(&p.z);
            assert_eq!(grid.set(p, step), inside);
            if inside {
                let index = ((p.z * h + p.y) * w + p.x) as usize;
                flat[index] = step;
                assert_eq!(grid.index(p), Some(index));
                assert_eq!(grid.pos(index), Some(p));
                let chunk = grid.chunk_of(p).unwrap();
                changed.insert(chunk);
                versions[chunk as usize] += 1;
                let (low, high) = grid.chunk_bounds(chunk).unwrap();
                assert!(low.x <= p.x && p.x < high.x && low.z <= p.z && p.z < high.z);
            }
            assert_eq!(grid.get(p), inside.then(|| &flat[grid.index(p).unwrap()]));
            if step % 97 == 0 {
                let mut out = Vec::new();
                grid.take_changed_chunks(&mut out);
                assert_eq!(out, changed.iter().copied().collect::<Vec<_>>());
                changed.clear();
                grid.take_changed_chunks(&mut out);
                assert!(out.is_empty());
            }
        }
        for (chunk, &version) in versions.iter().enumerate() {
            assert_eq!(grid.chunk_version(chunk as ChunkId), version);
        }
        let logical: Vec<u32> = grid.iter_logical().map(|(_, v)| *v).collect();
        assert_eq!(logical, flat);
        let mut visited = 0;
        for chunk in 0..grid.chunk_count() as ChunkId {
            grid.for_each_in_chunk(chunk, |p, value| {
                visited += 1;
                assert_eq!(grid.chunk_of(p), Some(chunk));
                assert_eq!(*value, flat[grid.index(p).unwrap()]);
            });
        }
        assert_eq!(visited, flat.len());
    }
}

#[test]
fn chunks_follow_morton_order() {
    let grid = ChunkedGrid3::new(64, 64, 32, 0u8).unwrap();
    let codes: Vec<_> = (0..grid.chunk_count() as ChunkId)
        .map(|id| {
            let o = grid.chunk_origin(id).unwrap();
            morton3(
                (o.x / CHUNK_X) as u32,
                (o.y / CHUNK_Y) as u32,
                (o.z / CHUNK_Z) as u32,
            )
        })
        .collect();
    assert!(codes.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn tiles_fit_in_four_bytes() {
    assert!(std::mem::size_of::<Tile>() <= 4);
}

const ROCK: Tile = Tile::new(TileKind::Solid, false, 1);
const AIR: Tile = Tile::new(TileKind::Open, false, 0);
const FLOOR: Tile = Tile::new(TileKind::Open, true, 0);

fn world(w: i32, h: i32, d: i32, fill: Tile) -> ChunkedGrid3<Tile> {
    ChunkedGrid3::new(w, h, d, fill).unwrap()
}

fn moves(grid: &ChunkedGrid3<Tile>, p: Pos3) -> Vec<(Pos3, u32)> {
    let mut out = Vec::new();
    walk_neighbors(grid, p, &mut out);
    out
}

#[test]
fn standability_requires_open_space_and_floor_or_stairs() {
    assert!(!standable(ROCK));
    assert!(!standable(AIR));
    assert!(standable(FLOOR));
    for kind in [
        TileKind::StairUp,
        TileKind::StairDown,
        TileKind::StairUpDown,
        TileKind::Ramp,
    ] {
        assert!(standable(Tile::new(kind, false, 0)));
    }
    assert!(!standable(Tile::new(TileKind::Door, false, 0)));
    assert!(standable(Tile::new(TileKind::Door, true, 0)));
    assert_eq!(dig(ROCK), Tile::new(TileKind::Open, true, 1));
}

#[test]
fn horizontal_moves_cost_ten_and_fourteen_without_corner_cutting() {
    let mut grid = world(3, 3, 1, FLOOR);
    let center = Pos3::new(1, 1, 0);
    let all = moves(&grid, center);
    assert_eq!(all.len(), 8);
    assert_eq!(all.iter().filter(|(_, c)| *c == 10).count(), 4);
    assert_eq!(all.iter().filter(|(_, c)| *c == 14).count(), 4);
    grid.set(Pos3::new(2, 1, 0), ROCK);
    let cut = moves(&grid, center);
    assert!(!cut.iter().any(|(p, _)| *p == Pos3::new(2, 0, 0)));
    assert!(!cut.iter().any(|(p, _)| *p == Pos3::new(2, 2, 0)));
    assert!(cut.iter().any(|(p, _)| *p == Pos3::new(0, 0, 0)));
    // An open tile without floor is not standable but does not block corners.
    grid.set(Pos3::new(2, 1, 0), AIR);
    assert_eq!(moves(&grid, center).len(), 7);
    assert!(moves(&grid, Pos3::new(2, 1, 0)).is_empty());
}

#[test]
fn stairs_connect_only_matching_pairs() {
    use TileKind::*;
    let mut grid = world(1, 1, 2, ROCK);
    let low = Pos3::new(0, 0, 0);
    let high = Pos3::new(0, 0, 1);
    for lower in [StairUp, StairDown, StairUpDown, Open, Ramp] {
        for upper in [StairUp, StairDown, StairUpDown, Open, Ramp] {
            grid.set(low, Tile::new(lower, true, 0));
            grid.set(high, Tile::new(upper, true, 0));
            let up = moves(&grid, low).contains(&(high, 20));
            let down = moves(&grid, high).contains(&(low, 20));
            let expected =
                matches!(lower, StairUp | StairUpDown) && matches!(upper, StairDown | StairUpDown);
            assert_eq!(up, expected, "{lower:?} below {upper:?}");
            assert_eq!(down, expected, "{lower:?} below {upper:?}");
        }
    }
}

#[test]
fn ramps_climb_to_adjacent_tiles_when_the_space_above_is_open() {
    let mut grid = world(3, 1, 2, ROCK);
    let ramp = Pos3::new(1, 0, 0);
    let top = Pos3::new(2, 0, 1);
    grid.set(ramp, Tile::new(TileKind::Ramp, false, 0));
    grid.set(top, FLOOR);
    assert!(!moves(&grid, ramp).contains(&(top, 20)));
    grid.set(Pos3::new(1, 0, 1), AIR);
    assert!(moves(&grid, ramp).contains(&(top, 20)));
    assert!(moves(&grid, top).contains(&(ramp, 20)));
    assert!(
        !moves(&grid, ramp)
            .iter()
            .any(|(p, _)| *p == Pos3::new(0, 0, 1))
    );
}

#[test]
fn random_worlds_have_symmetric_moves() {
    use TileKind::*;
    let kinds = [Solid, Open, StairUp, StairDown, StairUpDown, Ramp, Door];
    for seed in 0..20 {
        let mut rng = SystemRng::new(seed, 11);
        let mut grid = world(6, 5, 4, ROCK);
        for z in 0..4 {
            for y in 0..5 {
                for x in 0..6 {
                    let kind = kinds[below(rng.next_u64(), kinds.len() as u64) as usize];
                    let floor = below(rng.next_u64(), 2) == 0;
                    grid.set(Pos3::new(x, y, z), Tile::new(kind, floor, 0));
                }
            }
        }
        for (p, _) in grid.iter_logical() {
            for (q, cost) in moves(&grid, p) {
                assert!(moves(&grid, q).contains(&(p, cost)), "{p:?} -> {q:?}");
            }
        }
    }
}
