//! Support and cave-ins.
//!
//! Rules:
//! - Elements are solid tiles and bridge floors: floors above a non-solid tile.
//!   Floors on solid ground are the top of the tile below and need no support.
//! - Solid tiles connect to solid neighbors in six directions. Bridge floors connect
//!   horizontally to bridge floors and solid tiles, in both directions.
//! - Elements on the two lowest levels are anchors: level 0 holds the magma sea,
//!   level 1 is the bedrock resting on it.
//! - A component without an anchor is unsupported. Digging and collapses queue the
//!   neighbors of removed elements, building queues the new element.
//!
//! An unsupported tile falls straight down until it lands on a supported solid
//! tile or floor; the tile becomes open and a stone of its material appears where
//! it lands. Residents on the fall path die, items on the path move to the landing
//! tile. The resulting tile changes go through the normal sorted application.

use sim_core::{
    entity::EntityCommand,
    space::{ChunkedGrid3, Pos3, Tile, TileKind},
    support::SupportGraph,
};

use crate::{
    FortressSim, ItemKind, SOURCE_EVENT,
    types::{Death, ItemLocation, Op},
};

pub(crate) struct WorldSupport<'a>(pub &'a ChunkedGrid3<Tile>);

impl WorldSupport<'_> {
    fn solid(&self, p: Pos3) -> bool {
        self.0.get(p).is_some_and(|t| t.kind == TileKind::Solid)
    }

    fn bridge(&self, p: Pos3) -> bool {
        p.z > 0
            && self
                .0
                .get(p)
                .is_some_and(|t| t.kind != TileKind::Solid && t.floor())
            && !self.solid(p.offset(0, 0, -1))
    }
}

impl SupportGraph for WorldSupport<'_> {
    fn is_element(&self, p: Pos3) -> bool {
        self.solid(p) || self.bridge(p)
    }

    fn is_anchor(&self, p: Pos3) -> bool {
        p.z <= 1 && self.is_element(p)
    }

    fn neighbors(&self, p: Pos3, out: &mut Vec<Pos3>) {
        out.clear();
        let solid = self.solid(p);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let q = p.offset(dx, dy, 0);
            if self.solid(q) || self.bridge(q) {
                out.push(q);
            }
        }
        if solid {
            for dz in [1, -1] {
                let q = p.offset(0, 0, dz);
                if self.solid(q) {
                    out.push(q);
                }
            }
        }
    }

    fn index(&self, p: Pos3) -> Option<usize> {
        self.0.index(p)
    }

    fn node_count(&self) -> usize {
        self.0.len()
    }
}

impl FortressSim {
    /// Queues support checks for tile changes applied by the simulation.
    pub(crate) fn note_tile_change(&mut self, p: Pos3, old: Tile, new: Tile) {
        let graph = WorldSupport(&self.world);
        let was = old.kind == TileKind::Solid || (old.floor() && p.z > 0);
        let is = graph.is_element(p);
        if was && !is {
            self.support.note_removed(&graph, p);
        }
        if is && (!was || new.kind != old.kind) {
            self.support.note_added(&graph, p);
        }
    }

    pub(crate) fn support_step(&mut self) {
        let budget = self.config.physics.support_budget;
        let limit = self.config.physics.support_check_limit;
        let mut out = std::mem::take(&mut self.scratch.unsupported);
        let before = self.support.stats.units;
        self.support
            .run(&WorldSupport(&self.world), budget, limit, &mut out);
        self.diag.support_last_units = self.support.stats.units - before;
        self.scratch.unsupported = out;
    }

    pub(crate) fn collapse_step(&mut self) {
        if self.scratch.unsupported.is_empty() {
            return;
        }
        self.counters.collapses += 1;
        self.scratch.fluid_tiles.clear();
        for i in 0..self.scratch.unsupported.len() {
            let p = self.scratch.unsupported[i];
            let Some(&tile) = self.world.get(p) else {
                continue;
            };
            let falling = |q: Pos3| self.scratch.unsupported.binary_search(&q).is_ok();
            let below_solid = self
                .world
                .get(p.offset(0, 0, -1))
                .is_some_and(|t| t.kind == TileKind::Solid)
                && !falling(p.offset(0, 0, -1));
            let mut open = Tile::new(TileKind::Open, below_solid, tile.material);
            if tile.kind != TileKind::Solid {
                open = Tile::new(tile.kind, below_solid, tile.material);
            }
            self.scratch.fluid_tiles.push((p, open));
            // Fall until a supported solid tile or floor stops the material.
            let mut landing = p;
            loop {
                let below = landing.offset(0, 0, -1);
                let stops_on = |q: Pos3| {
                    self.world.get(q).is_some_and(|t| t.kind == TileKind::Solid) && !falling(q)
                };
                if landing.z == 0 || stops_on(below) {
                    break;
                }
                if landing != p
                    && self.world.get(landing).is_some_and(|t| t.floor())
                    && !falling(landing)
                {
                    break;
                }
                landing = below;
            }
            self.scratch.fall_paths.push((p, landing, tile.material));
            self.counters.collapsed_tiles += 1;
        }
        for i in 0..self.scratch.fall_paths.len() {
            let (top, landing, material) = self.scratch.fall_paths[i];
            for z in landing.z..=top.z {
                let q = Pos3::new(top.x, top.y, z);
                for slot in 0..self.rcols.len() {
                    if self.rcols.position[slot] == q
                        && let Some(id) = self.residents.id(slot as u32)
                    {
                        self.kill(id, Death::Collapse);
                    }
                }
                if q != landing {
                    for slot in 0..self.icols.len() {
                        if self.icols.location[slot] == ItemLocation::Ground(q)
                            && let Some(id) = self.items.id(slot as u32)
                        {
                            self.icols.location[slot] = ItemLocation::Ground(landing);
                            self.index_item(id);
                        }
                    }
                }
            }
            self.ops.push(
                (SOURCE_EVENT, u64::MAX - 2),
                self.world.index(top).unwrap_or(0) as u32,
                EntityCommand::Spawn(Op::SpawnItem {
                    at: landing,
                    kind: ItemKind::Stone,
                    material,
                }),
            );
        }
        self.scratch.fall_paths.clear();
        self.scratch.fluid_tiles.sort_unstable_by_key(|&(p, _)| p);
        for i in 0..self.scratch.fluid_tiles.len() {
            let (p, tile) = self.scratch.fluid_tiles[i];
            self.world.set(p, tile);
        }
        self.scratch.fluid_tiles.clear();
        self.scratch.unsupported.clear();
    }
}
