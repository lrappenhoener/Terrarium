use sim_core::{
    rng::{SystemRng, below, stateless_u64},
    space::{Grid2, Point},
};
use sim_host::SimError;

pub(crate) const BIOMASS_CAP: u16 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

pub(crate) fn generate(seed: u64, size: i32) -> Result<(Grid2<Tile>, usize), SimError> {
    let mut world = Grid2::new(
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
    let mut frontier = Vec::with_capacity(world.as_slice().len());
    for (index, tile) in world.as_mut_slice().iter_mut().enumerate() {
        let x = index as i32 % size;
        let y = index as i32 / size;
        let noise = below(stateless_u64(seed, (x / 8) as u64, (y / 8) as u64, 1), 100);
        let patch = below(rng.next_u64(), 100);
        tile.terrain = if (x % 9 == water_x && y % 9 == water_y) || (x == size / 2 && y == size / 2)
        {
            tile.water_distance = 0;
            frontier.push(Point { x, y });
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
    Ok((world, walkable))
}
