//! Frame updates from the simulation thread to the UI thread.
//!
//! The mailbox holds at most one pending frame. The simulation thread merges into
//! it: changed chunks accumulate, entity lists are replaced by the newest ones.
//! The UI swaps the pending frame with its spare frame, so chunk buffers and lists
//! are reused instead of allocated per update.

use std::sync::Mutex;

use fortress_sim::view::{ItemView, JobView, ResidentView};
use sim_core::space::{Pos3, Tile};

#[derive(Debug, Clone, Default)]
pub struct ChunkTiles {
    pub low: Pos3,
    pub high: Pos3,
    /// Tiles inside `low..high` in level, row, column order.
    pub tiles: Vec<Tile>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub tick: u64,
    pub speed: &'static str,
    pub tps: f64,
    pub p99_us: f64,
    pub residents: usize,
    pub items: usize,
    pub open_jobs: usize,
}

#[derive(Debug, Default)]
pub struct Frame {
    pub dims: (i32, i32, i32),
    pub stats: Stats,
    /// Pool of chunk buffers indexed by chunk ID; valid for IDs in `dirty`.
    pub chunks: Vec<ChunkTiles>,
    pub dirty: Vec<u32>,
    pub dirty_mark: Vec<bool>,
    /// Number of changes per chunk since the session start, for the heatmap.
    pub chunk_changes: Vec<u32>,
    pub residents: Vec<ResidentView>,
    pub items: Vec<ItemView>,
    pub jobs: Vec<JobView>,
    pub stockpiles: Vec<Pos3>,
    pub selected_path: Vec<Pos3>,
    /// Debug regions of one level, by `y * width + x`.
    pub regions: Option<(i32, Vec<Option<u32>>)>,
    pub description: Option<Vec<(String, String)>>,
    pub messages: Vec<String>,
    pub finished: bool,
    /// Fluid chunk pool and dirty list, like the tile chunks.
    pub fluid_chunks: Vec<ChunkFluids>,
    pub fluid_dirty: Vec<u32>,
    pub fluid_dirty_mark: Vec<bool>,
    /// Absolute temperatures of one level, by `y * width + x`.
    pub temperatures: Option<(i32, Vec<i32>)>,
    /// Per chunk: fluid awake, heat active.
    pub activity: Vec<(bool, bool)>,
    pub support_pending: Vec<Pos3>,
    /// Plants: position, content kind, growth, ripe.
    pub plants: Vec<(Pos3, u16, u32, bool)>,
    pub farms: Vec<fortress_sim::FarmPlot>,
    pub workshops: Vec<fortress_sim::Workshop>,
    pub zones: Vec<fortress_sim::Zone>,
    /// Zone ID per stockpile tile, aligned with `stockpiles`.
    pub stockpile_zones: Vec<u32>,
    pub orders: Vec<fortress_sim::WorkOrder>,
    pub season: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkFluids {
    pub low: Pos3,
    pub high: Pos3,
    pub values: Vec<u8>,
}

impl Frame {
    pub fn mark_dirty(&mut self, chunk: u32) {
        let index = chunk as usize;
        if self.dirty_mark.len() <= index {
            self.dirty_mark.resize(index + 1, false);
        }
        if !std::mem::replace(&mut self.dirty_mark[index], true) {
            self.dirty.push(chunk);
        }
    }

    pub fn clear_dirty(&mut self) {
        for &chunk in &self.dirty {
            self.dirty_mark[chunk as usize] = false;
        }
        self.dirty.clear();
        for &chunk in &self.fluid_dirty {
            self.fluid_dirty_mark[chunk as usize] = false;
        }
        self.fluid_dirty.clear();
        self.messages.clear();
        self.description = None;
    }

    fn chunk_buffer(&mut self, chunk: u32) -> &mut ChunkTiles {
        let index = chunk as usize;
        if self.chunks.len() <= index {
            self.chunks.resize_with(index + 1, ChunkTiles::default);
        }
        &mut self.chunks[index]
    }

    /// Copies a chunk's fluids into the pool and marks them dirty.
    pub fn store_fluids(
        &mut self,
        chunk: u32,
        low: Pos3,
        high: Pos3,
        values: impl Iterator<Item = u8>,
    ) {
        let index = chunk as usize;
        if self.fluid_chunks.len() <= index {
            self.fluid_chunks
                .resize_with(index + 1, ChunkFluids::default);
            self.fluid_dirty_mark.resize(index + 1, false);
        }
        let buffer = &mut self.fluid_chunks[index];
        buffer.low = low;
        buffer.high = high;
        buffer.values.clear();
        buffer.values.extend(values);
        if !std::mem::replace(&mut self.fluid_dirty_mark[index], true) {
            self.fluid_dirty.push(chunk);
        }
    }

    /// Copies a chunk into the pool and marks it dirty.
    pub fn store_chunk(
        &mut self,
        chunk: u32,
        low: Pos3,
        high: Pos3,
        tiles: impl Iterator<Item = Tile>,
    ) {
        let buffer = self.chunk_buffer(chunk);
        buffer.low = low;
        buffer.high = high;
        buffer.tiles.clear();
        buffer.tiles.extend(tiles);
        self.mark_dirty(chunk);
    }
}

#[derive(Default)]
pub struct Mailbox {
    pending: Mutex<Frame>,
}

impl Mailbox {
    /// Merges into the pending frame.
    pub fn publish(&self, update: impl FnOnce(&mut Frame)) {
        if let Ok(mut frame) = self.pending.lock() {
            update(&mut frame);
        }
    }

    /// Swaps the pending frame with `spare`, which must have been cleared.
    pub fn take(&self, spare: &mut Frame) {
        if let Ok(mut frame) = self.pending.lock() {
            std::mem::swap(&mut *frame, spare);
            // Keep accumulated chunk-change counts and the frame layout in the mailbox.
            frame.dims = spare.dims;
            frame.chunk_changes.clone_from(&spare.chunk_changes);
            frame.finished = spare.finished;
        }
    }
}

/// UI-side copy of the world, updated only for changed chunks.
pub struct Mirror {
    pub dims: (i32, i32, i32),
    tiles: Vec<Tile>,
    fluids: Vec<u8>,
    pub original: Vec<Tile>,
    /// Bounds of every chunk received so far, by chunk ID.
    pub bounds: Vec<Option<(Pos3, Pos3)>>,
}

impl Mirror {
    pub fn new(dims: (i32, i32, i32), original: Vec<Tile>) -> Self {
        let len = (dims.0 * dims.1 * dims.2) as usize;
        let fill = original.first().copied().unwrap_or(Tile::new(
            sim_core::space::TileKind::Open,
            false,
            0,
        ));
        Self {
            dims,
            tiles: if original.len() == len {
                original.clone()
            } else {
                vec![fill; len]
            },
            original,
            bounds: Vec::new(),
            fluids: vec![0; len],
        }
    }

    fn index(&self, p: Pos3) -> Option<usize> {
        let (w, h, d) = self.dims;
        ((0..w).contains(&p.x) && (0..h).contains(&p.y) && (0..d).contains(&p.z))
            .then(|| ((p.z * h + p.y) * w + p.x) as usize)
    }

    pub fn get(&self, p: Pos3) -> Option<Tile> {
        self.index(p).map(|i| self.tiles[i])
    }

    /// Visible fluid level 0 to 7 and whether it is magma.
    pub fn fluid(&self, p: Pos3) -> (u8, bool) {
        self.index(p).map_or((0, false), |i| {
            let value = self.fluids[i];
            ((value & 0x7f).div_ceil(16), value & 0x80 != 0)
        })
    }

    /// Whether the tile differs from the generated world, for example a built wall.
    pub fn is_built(&self, p: Pos3) -> bool {
        self.index(p)
            .is_some_and(|i| self.original.get(i).is_some_and(|t| *t != self.tiles[i]))
    }

    pub fn apply(&mut self, frame: &Frame) {
        for &chunk in &frame.fluid_dirty {
            let Some(buffer) = frame.fluid_chunks.get(chunk as usize) else {
                continue;
            };
            let mut values = buffer.values.iter();
            for z in buffer.low.z..buffer.high.z {
                for y in buffer.low.y..buffer.high.y {
                    for x in buffer.low.x..buffer.high.x {
                        if let (Some(i), Some(v)) = (self.index(Pos3::new(x, y, z)), values.next())
                        {
                            self.fluids[i] = *v;
                        }
                    }
                }
            }
        }
        for &chunk in &frame.dirty {
            let Some(buffer) = frame.chunks.get(chunk as usize) else {
                continue;
            };
            let (low, high) = (buffer.low, buffer.high);
            if self.bounds.len() <= chunk as usize {
                self.bounds.resize(chunk as usize + 1, None);
            }
            self.bounds[chunk as usize] = Some((low, high));
            let mut tiles = buffer.tiles.iter();
            for z in low.z..high.z {
                for y in low.y..high.y {
                    for x in low.x..high.x {
                        if let (Some(i), Some(tile)) =
                            (self.index(Pos3::new(x, y, z)), tiles.next())
                        {
                            self.tiles[i] = *tile;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::space::TileKind;

    #[test]
    fn mailbox_coalesces_chunks_and_replaces_lists() {
        let mailbox = Mailbox::default();
        let rock = Tile::new(TileKind::Solid, false, 1);
        let floor = Tile::new(TileKind::Open, true, 0);
        mailbox.publish(|f| {
            f.store_chunk(
                2,
                Pos3::new(0, 0, 0),
                Pos3::new(2, 1, 1),
                [rock, rock].into_iter(),
            );
            f.stats.tick = 1;
        });
        mailbox.publish(|f| {
            f.store_chunk(
                2,
                Pos3::new(0, 0, 0),
                Pos3::new(2, 1, 1),
                [floor, rock].into_iter(),
            );
            f.store_chunk(
                0,
                Pos3::new(2, 0, 0),
                Pos3::new(3, 1, 1),
                [floor].into_iter(),
            );
            f.stats.tick = 2;
        });
        let mut frame = Frame::default();
        mailbox.take(&mut frame);
        assert_eq!(frame.dirty, [2, 0]);
        assert_eq!(frame.stats.tick, 2);
        let mut mirror = Mirror::new((3, 1, 1), vec![rock; 3]);
        mirror.apply(&frame);
        assert_eq!(mirror.get(Pos3::new(0, 0, 0)), Some(floor));
        assert_eq!(mirror.get(Pos3::new(2, 0, 0)), Some(floor));
        assert!(mirror.is_built(Pos3::new(0, 0, 0)) && !mirror.is_built(Pos3::new(1, 0, 0)));
        frame.clear_dirty();
        let mut again = frame;
        mailbox.take(&mut again);
        assert!(again.dirty.is_empty(), "nothing new was published");
    }
}
