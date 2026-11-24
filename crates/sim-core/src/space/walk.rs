//! Generic tile model and movement rules for walkable layered worlds.
//!
//! Positions outside the grid count as solid. Movement is symmetric: every rule
//! that allows a step from `a` to `b` also allows the step from `b` to `a`.

use serde::{Deserialize, Serialize};

use super::{ChunkedGrid3, Pos3};
use crate::state::{StateHash, StateHasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum TileKind {
    Solid,
    Open,
    StairUp,
    StairDown,
    StairUpDown,
    Ramp,
    Door,
}

/// A layered tile. `material` is opaque to `sim-core`; games assign its meaning.
/// `flags` holds the floor (bit 0) and a temporary impassable marker (bit 1), for
/// example set by a game while a liquid fills the tile. Without the marker the
/// byte equals the former `floor: bool` in hashes and Postcard encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub kind: TileKind,
    flags: u8,
    pub material: u16,
}

const FLOOR: u8 = 1;
const IMPASSABLE: u8 = 2;

impl Tile {
    pub const fn new(kind: TileKind, floor: bool, material: u16) -> Self {
        Self {
            kind,
            flags: if floor { FLOOR } else { 0 },
            material,
        }
    }

    pub const fn floor(self) -> bool {
        self.flags & FLOOR != 0
    }

    pub const fn with_floor(self, floor: bool) -> Self {
        Self {
            flags: if floor {
                self.flags | FLOOR
            } else {
                self.flags & !FLOOR
            },
            ..self
        }
    }

    /// Whether walking onto the tile is temporarily impossible.
    pub const fn impassable(self) -> bool {
        self.flags & IMPASSABLE != 0
    }

    pub const fn with_impassable(self, impassable: bool) -> Self {
        Self {
            flags: if impassable {
                self.flags | IMPASSABLE
            } else {
                self.flags & !IMPASSABLE
            },
            ..self
        }
    }
}

impl StateHash for Tile {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u8(self.kind as u8);
        hash.write_u8(self.flags);
        self.material.state_hash(hash);
    }
}

pub trait TileSource {
    fn tile(&self, p: Pos3) -> Option<Tile>;
}

impl TileSource for ChunkedGrid3<Tile> {
    fn tile(&self, p: Pos3) -> Option<Tile> {
        self.get(p).copied()
    }
}

const fn climbs_up(kind: TileKind) -> bool {
    matches!(kind, TileKind::StairUp | TileKind::StairUpDown)
}

const fn climbs_down(kind: TileKind) -> bool {
    matches!(kind, TileKind::StairDown | TileKind::StairUpDown)
}

/// A unit can stand on a non-solid, passable tile with a floor, a stair or a ramp.
pub const fn standable(tile: Tile) -> bool {
    !matches!(tile.kind, TileKind::Solid)
        && !tile.impassable()
        && (tile.floor()
            || matches!(
                tile.kind,
                TileKind::StairUp | TileKind::StairDown | TileKind::StairUpDown | TileKind::Ramp
            ))
}

pub fn is_solid(source: &impl TileSource, p: Pos3) -> bool {
    source
        .tile(p)
        .is_none_or(|tile| tile.kind == TileKind::Solid)
}

fn standable_at(source: &impl TileSource, p: Pos3) -> bool {
    source.tile(p).is_some_and(standable)
}

/// Excavates a solid tile into open floor. Neighboring floors are unaffected.
pub const fn dig(tile: Tile) -> Tile {
    Tile::new(TileKind::Open, true, tile.material)
}

/// Orthogonal offsets in deterministic order: north, west, east, south.
pub const ORTHOGONAL: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
const DIAGONAL: [(i32, i32); 4] = [(-1, -1), (1, -1), (-1, 1), (1, 1)];

pub const ORTHOGONAL_COST: u32 = 10;
pub const DIAGONAL_COST: u32 = 14;
pub const VERTICAL_COST: u32 = 20;

/// Appends all moves from `p` with their costs in a fixed order: orthogonal,
/// diagonal, stair up, stair down, ramp up, ramp down. Nothing is appended if `p`
/// is not standable.
pub fn walk_neighbors(source: &impl TileSource, p: Pos3, out: &mut Vec<(Pos3, u32)>) {
    let Some(here) = source.tile(p).filter(|tile| standable(*tile)) else {
        return;
    };
    for (dx, dy) in ORTHOGONAL {
        let next = p.offset(dx, dy, 0);
        if standable_at(source, next) {
            out.push((next, ORTHOGONAL_COST));
        }
    }
    for (dx, dy) in DIAGONAL {
        let next = p.offset(dx, dy, 0);
        if standable_at(source, next)
            && !is_solid(source, p.offset(dx, 0, 0))
            && !is_solid(source, p.offset(0, dy, 0))
        {
            out.push((next, DIAGONAL_COST));
        }
    }
    let above = p.offset(0, 0, 1);
    let below = p.offset(0, 0, -1);
    if climbs_up(here.kind) && source.tile(above).is_some_and(|t| climbs_down(t.kind)) {
        out.push((above, VERTICAL_COST));
    }
    if climbs_down(here.kind) && source.tile(below).is_some_and(|t| climbs_up(t.kind)) {
        out.push((below, VERTICAL_COST));
    }
    if here.kind == TileKind::Ramp && !is_solid(source, above) {
        for (dx, dy) in ORTHOGONAL {
            let next = p.offset(dx, dy, 1);
            if standable_at(source, next) {
                out.push((next, VERTICAL_COST));
            }
        }
    }
    for (dx, dy) in ORTHOGONAL {
        let ramp = p.offset(dx, dy, -1);
        if source.tile(ramp).is_some_and(|t| t.kind == TileKind::Ramp)
            && !is_solid(source, p.offset(dx, dy, 0))
        {
            out.push((ramp, VERTICAL_COST));
        }
    }
}
