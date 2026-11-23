//! Seeded integer world generation: flat land, a mountain, rock layers and ore veins.

use sim_core::{
    rng::{SystemRng, below, stateless_u64},
    space::{ChunkedGrid3, Pos3, Tile, TileKind},
};
use sim_host::SimError;

use crate::FortressConfig;

pub(crate) const SOIL: u16 = 0;
const NOISE_CELL: i64 = 8;

/// Smooth integer value noise in `-amplitude..=amplitude`.
fn value_noise(seed: u64, x: i32, y: i32, salt: u64, amplitude: i64) -> i64 {
    let lattice = |gx: i64, gy: i64| {
        below(
            stateless_u64(seed, gx as u64, gy as u64, salt),
            (2 * amplitude + 1) as u64,
        ) as i64
            - amplitude
    };
    let (x, y) = (i64::from(x), i64::from(y));
    let (gx, gy) = (x.div_euclid(NOISE_CELL), y.div_euclid(NOISE_CELL));
    let (fx, fy) = (x.rem_euclid(NOISE_CELL), y.rem_euclid(NOISE_CELL));
    let n = NOISE_CELL;
    let sum = lattice(gx, gy) * (n - fx) * (n - fy)
        + lattice(gx + 1, gy) * fx * (n - fy)
        + lattice(gx, gy + 1) * (n - fx) * fy
        + lattice(gx + 1, gy + 1) * fx * fy;
    sum.div_euclid(n * n)
}

/// Standing level of each column: solid below, floor at the level, air above.
pub(crate) fn surface(seed: u64, config: &FortressConfig, x: i32, y: i32) -> i32 {
    let dx = i64::from(x - config.mountain_center_x);
    let dy = i64::from(y - config.mountain_center_y);
    let r2 = i64::from(config.mountain_radius).pow(2);
    let d2 = dx * dx + dy * dy;
    let mut level = i64::from(config.ground_level);
    if d2 < r2 {
        let bump = i64::from(config.mountain_height) * (r2 - d2) / r2;
        let rough = value_noise(seed, x, y, 1, 3);
        level += (bump + rough * bump.min(4) / 4).max(0);
    }
    level.clamp(1, i64::from(config.world_depth - 2)) as i32
}

fn rock_layer(seed: u64, config: &FortressConfig, rocks: &[u16], p: Pos3) -> u16 {
    let wobble = value_noise(seed, p.x, p.y, 2, 2);
    let top = i64::from(config.ground_level + config.mountain_height);
    let depth = (top - i64::from(p.z) + wobble).max(0);
    let count = rocks.len() as i64;
    let layer = (depth * count / (top + 1)).clamp(0, count - 1);
    rocks[layer as usize]
}

pub(crate) fn generate(seed: u64, config: &FortressConfig) -> Result<ChunkedGrid3<Tile>, SimError> {
    let (size, depth) = (config.world_size_xy, config.world_depth);
    let mut world = ChunkedGrid3::new(size, size, depth, Tile::new(TileKind::Open, false, SOIL))
        .ok_or_else(|| SimError::InvalidConfig("could not reserve world".into()))?;
    let rocks: Vec<u16> = (1..config.materials.len() as u16)
        .filter(|&m| !config.materials[usize::from(m)].ore)
        .collect();
    let ores: Vec<u16> = (1..config.materials.len() as u16)
        .filter(|&m| config.materials[usize::from(m)].ore)
        .collect();
    for y in 0..size {
        for x in 0..size {
            let level = surface(seed, config, x, y);
            let soil = 2 + value_noise(seed, x, y, 3, 1);
            for z in 0..depth {
                let p = Pos3::new(x, y, z);
                let tile = if z < level {
                    let material = if i64::from(level - 1 - z) < soil {
                        SOIL
                    } else {
                        rock_layer(seed, config, &rocks, p)
                    };
                    Tile::new(TileKind::Solid, false, material)
                } else {
                    Tile::new(TileKind::Open, z == level, SOIL)
                };
                world.set(p, tile);
            }
        }
    }
    if !ores.is_empty() {
        let mut rng = SystemRng::new(seed, 2);
        let radius = i64::from(config.mountain_radius).max(2);
        for _ in 0..config.ore_veins {
            let offset =
                |rng: &mut SystemRng| below(rng.next_u64(), radius as u64) as i64 - radius / 2;
            let mut p = Pos3::new(
                (i64::from(config.mountain_center_x) + offset(&mut rng)) as i32,
                (i64::from(config.mountain_center_y) + offset(&mut rng)) as i32,
                1 + below(
                    rng.next_u64(),
                    (config.ground_level + config.mountain_height / 2).max(2) as u64,
                ) as i32,
            );
            let ore = ores[below(rng.next_u64(), ores.len() as u64) as usize];
            for _ in 0..config.ore_vein_length {
                if let Some(&tile) = world.get(p)
                    && tile.kind == TileKind::Solid
                    && tile.material != SOIL
                {
                    world.set(p, Tile::new(TileKind::Solid, false, ore));
                }
                let (dx, dy, dz) = match below(rng.next_u64(), 10) {
                    0..3 => (1, 0, 0),
                    3..5 => (-1, 0, 0),
                    5..7 => (0, 1, 0),
                    7..9 => (0, -1, 0),
                    _ if below(rng.next_u64(), 2) == 0 => (0, 0, 1),
                    _ => (0, 0, -1),
                };
                let next = p.offset(dx, dy, dz);
                if world.in_bounds(next) && next.z > 0 {
                    p = next;
                }
            }
        }
    }
    world.clear_changes();
    Ok(world)
}
