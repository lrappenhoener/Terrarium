use sim_core::{
    rng::{SystemRng, below, stateless_u64},
    space::{ChunkedGrid2, Point},
};
use sim_host::SimError;

pub(crate) const BIOMASS_CAP: u16 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Terrain {
    Water,
    Grass,
    Jungle,
    Rock,
}

#[derive(Clone, Copy)]
pub(crate) struct Tile {
    pub terrain: Terrain,
    pub biomass: u16,
    pub water_distance: u16,
}

impl Tile {
    pub fn walkable(self) -> bool {
        matches!(self.terrain, Terrain::Grass | Terrain::Jungle)
    }
}

pub(crate) const OFFSETS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

pub(crate) fn neighbors(point: Point) -> impl Iterator<Item = Point> {
    OFFSETS.into_iter().map(move |(x, y)| Point {
        x: point.x + x,
        y: point.y + y,
    })
}

pub(crate) fn generate(seed: u64, size: i32) -> Result<(ChunkedGrid2<Tile>, usize), SimError> {
    let mut world = ChunkedGrid2::new(
        size,
        size,
        Tile {
            terrain: Terrain::Grass,
            biomass: 0,
            water_distance: u16::MAX,
        },
    )
    .ok_or_else(|| SimError::InvalidConfig("could not reserve jungle world".into()))?;
    let mut rng = SystemRng::new(seed, 0);
    let water_x = below(rng.next_u64(), 5) as i32;
    let water_y = below(rng.next_u64(), 5) as i32;
    let mut walkable = 0;

    for index in 0..world.len() {
        let Some(tile) = world.get_index_mut(index) else {
            continue;
        };
        let x = index as i32 % size;
        let y = index as i32 / size;
        let noise = below(stateless_u64(seed, (x / 8) as u64, (y / 8) as u64, 1), 100);
        let patch = below(rng.next_u64(), 100);
        tile.terrain = if (x % 9 == water_x && y % 9 == water_y) || (x == size / 2 && y == size / 2)
        {
            Terrain::Water
        } else if patch < 5 {
            Terrain::Rock
        } else if noise < 36 && patch < 80 {
            Terrain::Jungle
        } else {
            Terrain::Grass
        };
        if tile.walkable() {
            tile.biomass = 500 + (patch as u16 * 5);
            walkable += 1;
        }
    }
    rebuild_water_distances(&mut world, size);
    Ok((world, walkable))
}

pub(crate) fn rebuild_water_distances(world: &mut ChunkedGrid2<Tile>, size: i32) {
    let mut frontier = Vec::with_capacity(world.len());
    for index in 0..world.len() {
        if let Some(tile) = world.get_index_mut(index) {
            tile.water_distance = if tile.terrain == Terrain::Water {
                frontier.push(Point {
                    x: index as i32 % size,
                    y: index as i32 / size,
                });
                0
            } else {
                u16::MAX
            };
        }
    }
    let mut cursor = 0;
    while let Some(&point) = frontier.get(cursor) {
        cursor += 1;
        if let Some(current) = world.get(point) {
            let distance = current.water_distance.saturating_add(1);
            for next in neighbors(point) {
                if let Some(&tile) = world
                    .get(next)
                    .filter(|tile| tile.walkable() && tile.water_distance > distance)
                {
                    world.set(
                        next,
                        Tile {
                            water_distance: distance,
                            ..tile
                        },
                    );
                    frontier.push(next);
                }
            }
        }
    }
}
