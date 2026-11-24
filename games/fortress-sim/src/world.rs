//! Seeded integer world generation: flat land, a mountain, rock layers and ore veins.

use sim_core::{
    rng::{SystemRng, below, stateless_u64},
    space::{ChunkedGrid3, Pos3, Tile, TileKind},
};
use sim_host::SimError;

use crate::{FortressConfig, content::GameContent};

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

pub(crate) fn generate(
    seed: u64,
    config: &FortressConfig,
    content: &GameContent,
) -> Result<ChunkedGrid3<Tile>, SimError> {
    let (size, depth) = (config.world_size_xy, config.world_depth);
    let soil_material = content.soil;
    let mut world = ChunkedGrid3::new(
        size,
        size,
        depth,
        Tile::new(TileKind::Open, false, soil_material),
    )
    .ok_or_else(|| SimError::InvalidConfig("could not reserve world".into()))?;
    let rocks = &content.rocks;
    let ores = &content.ores;
    for y in 0..size {
        for x in 0..size {
            let level = surface(seed, config, x, y);
            let soil = 2 + value_noise(seed, x, y, 3, 1);
            for z in 0..depth {
                let p = Pos3::new(x, y, z);
                let tile = if z < level {
                    let material = if i64::from(level - 1 - z) < soil {
                        soil_material
                    } else {
                        rock_layer(seed, config, rocks, p)
                    };
                    Tile::new(TileKind::Solid, false, material)
                } else {
                    Tile::new(TileKind::Open, z == level, soil_material)
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
                    && tile.material != soil_material
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
    carve_physics(config, content, &mut world);
    world.clear_changes();
    Ok(world)
}

/// Standing level of the river bed and the x range of the river at the east edge.
pub(crate) fn river(config: &FortressConfig) -> Option<(std::ops::Range<i32>, i32)> {
    let p = &config.physics;
    (p.river && p.river_width > 0).then(|| {
        let end = config.world_size_xy - 2;
        ((end - p.river_width).max(0)..end, config.ground_level - 1)
    })
}

fn carve_physics(config: &FortressConfig, content: &GameContent, world: &mut ChunkedGrid3<Tile>) {
    let p = &config.physics;
    let soil_material = content.soil;
    {
        let aquifer = content.aquifer;
        let r2 = i64::from(p.aquifer_radius).pow(2);
        for z in p.aquifer_z_low..=p.aquifer_z_high {
            for y in 0..config.world_size_xy {
                for x in 0..config.world_size_xy {
                    let dx = i64::from(x - p.aquifer_center_x);
                    let dy = i64::from(y - p.aquifer_center_y);
                    let at = Pos3::new(x, y, z);
                    if dx * dx + dy * dy <= r2
                        && world.get(at).is_some_and(|t| {
                            t.kind == TileKind::Solid && t.material != soil_material
                        })
                    {
                        world.set(at, Tile::new(TileKind::Solid, false, aquifer));
                    }
                }
            }
        }
    }
    if let Some((xs, bed)) = river(config) {
        for y in 0..config.world_size_xy {
            for x in xs.clone() {
                world.set(
                    Pos3::new(x, y, bed),
                    Tile::new(TileKind::Open, true, soil_material),
                );
                world.set(
                    Pos3::new(x, y, bed + 1),
                    Tile::new(TileKind::Open, false, soil_material),
                );
            }
        }
    }
    if p.magma_sea {
        for y in 0..config.world_size_xy {
            for x in 0..config.world_size_xy {
                world.set(
                    Pos3::new(x, y, 0),
                    Tile::new(TileKind::Open, true, soil_material).with_impassable(true),
                );
            }
        }
    }
}

/// Fluid byte: units in bits 0 to 6 (16 per level), magma in bit 7.
pub(crate) const MAGMA: u8 = 0x80;

/// Initial fluids: the river bed holds water, the bottom level is a magma sea.
pub(crate) fn generate_fluids(config: &FortressConfig) -> Option<ChunkedGrid3<u8>> {
    let size = config.world_size_xy;
    let mut fluid = ChunkedGrid3::new(size, size, config.world_depth, 0u8)?;
    if let Some((xs, bed)) = river(config) {
        for y in 0..size {
            for x in xs.clone() {
                fluid.set(
                    Pos3::new(x, y, bed),
                    config.physics.river_initial_level * 16,
                );
            }
        }
    }
    if config.physics.magma_sea {
        for y in 0..size {
            for x in 0..size {
                fluid.set(Pos3::new(x, y, 0), MAGMA | (7 * 16));
            }
        }
    }
    fluid.clear_changes();
    Some(fluid)
}

/// Manually maintained generator version. Every change to the generated world
/// increments it, so stored worlds cannot be combined with a different generator.
pub(crate) const GENERATOR_VERSION: u32 = 3;

/// Hash over the generator version, its internal constants and all generator inputs.
pub(crate) fn generator_fingerprint(config: &FortressConfig, content: &GameContent) -> (u32, u64) {
    let mut hash = sim_host::StateHasher::new();
    hash.write_bytes(b"fortress-generator");
    hash.write_u32(GENERATOR_VERSION);
    hash.write_u64(NOISE_CELL as u64);
    for value in [
        config.world_size_xy,
        config.world_depth,
        config.ground_level,
        config.mountain_center_x,
        config.mountain_center_y,
        config.mountain_radius,
        config.mountain_height,
    ] {
        hash.write_u32(value as u32);
    }
    hash.write_u32(config.ore_veins);
    hash.write_u32(config.ore_vein_length);
    let p = &config.physics;
    hash.write_bool(p.river);
    hash.write_bool(p.magma_sea);
    for value in [
        p.river_width,
        p.aquifer_center_x,
        p.aquifer_center_y,
        p.aquifer_radius,
        p.aquifer_z_low,
        p.aquifer_z_high,
    ] {
        hash.write_u32(value as u32);
    }
    hash.write_u8(p.river_initial_level);
    // Materials, strata and veins come from the content.
    hash.write_u64(content.fingerprint);
    (GENERATOR_VERSION, hash.finish())
}
