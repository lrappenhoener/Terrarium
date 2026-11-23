//! Layered grids stored in Morton-ordered chunks with per-chunk change tracking.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::state::{StateHash, StateHasher};

/// A tile position. Ordering is logical: by level, then row, then column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pos3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos3 {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Offsets with wrapping arithmetic. Results outside the grid fail `in_bounds`.
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x.wrapping_add(dx),
            y: self.y.wrapping_add(dy),
            z: self.z.wrapping_add(dz),
        }
    }

    pub fn chebyshev(self, other: Self) -> u32 {
        self.x
            .abs_diff(other.x)
            .max(self.y.abs_diff(other.y))
            .max(self.z.abs_diff(other.z))
    }
}

impl Ord for Pos3 {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.z, self.y, self.x).cmp(&(other.z, other.y, other.x))
    }
}

impl PartialOrd for Pos3 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl StateHash for Pos3 {
    fn state_hash(&self, hash: &mut StateHasher) {
        self.x.state_hash(hash);
        self.y.state_hash(hash);
        self.z.state_hash(hash);
    }
}

pub const CHUNK_X: i32 = 16;
pub const CHUNK_Y: i32 = 16;
pub const CHUNK_Z: i32 = 8;
const CHUNK_VOLUME: usize = (CHUNK_X * CHUNK_Y * CHUNK_Z) as usize;
const _: () =
    assert!(CHUNK_X.count_ones() == 1 && CHUNK_Y.count_ones() == 1 && CHUNK_Z.count_ones() == 1);

/// Storage index of a chunk. Chunk IDs ascend in 3D Morton order.
pub type ChunkId = u32;

/// Interleaves three nonnegative 21-bit coordinates in Morton order.
pub fn morton3(x: u32, y: u32, z: u32) -> u64 {
    fn spread(value: u32) -> u64 {
        let mut x = u64::from(value) & 0x1f_ffff;
        x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
        x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
        x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
        x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
        (x | (x << 2)) & 0x1249_2492_4924_9249
    }
    spread(x) | (spread(y) << 1) | (spread(z) << 2)
}

/// A box of tiles with padded chunks in Morton order. Every `set` increments the
/// chunk's version and marks the chunk as changed until `take_changed_chunks`.
/// Versions and change marks describe write history, not logical state.
pub struct ChunkedGrid3<T> {
    width: i32,
    height: i32,
    depth: i32,
    columns: usize,
    rows: usize,
    chunk_at: Vec<ChunkId>,
    origins: Vec<Pos3>,
    values: Vec<T>,
    versions: Vec<u32>,
    changed: Vec<bool>,
    changed_list: Vec<ChunkId>,
}

impl<T: Clone> ChunkedGrid3<T> {
    pub fn new(width: i32, height: i32, depth: i32, value: T) -> Option<Self> {
        if width <= 0 || height <= 0 || depth <= 0 {
            return None;
        }
        let columns = (width - 1) / CHUNK_X + 1;
        let rows = (height - 1) / CHUNK_Y + 1;
        let layers = (depth - 1) / CHUNK_Z + 1;
        let count = (columns as usize)
            .checked_mul(rows as usize)?
            .checked_mul(layers as usize)?;
        u32::try_from(count).ok()?;
        let padded = count.checked_mul(CHUNK_VOLUME)?;
        let mut origins = Vec::new();
        origins.try_reserve_exact(count).ok()?;
        for z in 0..layers {
            for y in 0..rows {
                for x in 0..columns {
                    origins.push(Pos3::new(x, y, z));
                }
            }
        }
        origins.sort_unstable_by_key(|p| morton3(p.x as u32, p.y as u32, p.z as u32));
        let mut chunk_at = vec![0; count];
        for (id, origin) in origins.iter_mut().enumerate() {
            chunk_at[(origin.z as usize * rows as usize + origin.y as usize) * columns as usize
                + origin.x as usize] = id as ChunkId;
            *origin = Pos3::new(origin.x * CHUNK_X, origin.y * CHUNK_Y, origin.z * CHUNK_Z);
        }
        let mut values = Vec::new();
        values.try_reserve_exact(padded).ok()?;
        values.resize(padded, value);
        Some(Self {
            width,
            height,
            depth,
            columns: columns as usize,
            rows: rows as usize,
            chunk_at,
            origins,
            values,
            versions: vec![0; count],
            changed: vec![false; count],
            changed_list: Vec::with_capacity(count),
        })
    }
}

impl<T> ChunkedGrid3<T> {
    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn depth(&self) -> i32 {
        self.depth
    }

    /// Number of real tiles, excluding padding.
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize * self.depth as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn in_bounds(&self, p: Pos3) -> bool {
        (0..self.width).contains(&p.x)
            && (0..self.height).contains(&p.y)
            && (0..self.depth).contains(&p.z)
    }

    /// Dense logical index in level, row, column order.
    pub fn index(&self, p: Pos3) -> Option<usize> {
        self.in_bounds(p).then(|| {
            (p.z as usize * self.height as usize + p.y as usize) * self.width as usize
                + p.x as usize
        })
    }

    pub fn pos(&self, index: usize) -> Option<Pos3> {
        (index < self.len()).then(|| {
            let width = self.width as usize;
            let height = self.height as usize;
            Pos3::new(
                (index % width) as i32,
                (index / width % height) as i32,
                (index / (width * height)) as i32,
            )
        })
    }

    fn chunk_unchecked(&self, p: Pos3) -> ChunkId {
        let cx = (p.x / CHUNK_X) as usize;
        let cy = (p.y / CHUNK_Y) as usize;
        let cz = (p.z / CHUNK_Z) as usize;
        self.chunk_at[(cz * self.rows + cy) * self.columns + cx]
    }

    fn physical(&self, p: Pos3) -> usize {
        let chunk = self.chunk_unchecked(p) as usize;
        let lx = (p.x & (CHUNK_X - 1)) as usize;
        let ly = (p.y & (CHUNK_Y - 1)) as usize;
        let lz = (p.z & (CHUNK_Z - 1)) as usize;
        chunk * CHUNK_VOLUME + (lz * CHUNK_Y as usize + ly) * CHUNK_X as usize + lx
    }

    pub fn get(&self, p: Pos3) -> Option<&T> {
        self.in_bounds(p).then(|| &self.values[self.physical(p)])
    }

    /// Writes a value and records the change for the chunk.
    pub fn set(&mut self, p: Pos3, value: T) -> bool {
        if !self.in_bounds(p) {
            return false;
        }
        let index = self.physical(p);
        self.values[index] = value;
        let chunk = self.chunk_unchecked(p);
        self.mark(chunk);
        true
    }

    fn mark(&mut self, chunk: ChunkId) {
        let id = chunk as usize;
        self.versions[id] = self.versions[id].wrapping_add(1);
        if !std::mem::replace(&mut self.changed[id], true) {
            self.changed_list.push(chunk);
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.origins.len()
    }

    pub fn chunk_of(&self, p: Pos3) -> Option<ChunkId> {
        self.in_bounds(p).then(|| self.chunk_unchecked(p))
    }

    /// Lowest corner of the chunk, which may extend past the grid.
    pub fn chunk_origin(&self, chunk: ChunkId) -> Option<Pos3> {
        self.origins.get(chunk as usize).copied()
    }

    /// Inclusive lower and exclusive upper corner, clipped to the grid.
    pub fn chunk_bounds(&self, chunk: ChunkId) -> Option<(Pos3, Pos3)> {
        let origin = self.chunk_origin(chunk)?;
        Some((
            origin,
            Pos3::new(
                (origin.x + CHUNK_X).min(self.width),
                (origin.y + CHUNK_Y).min(self.height),
                (origin.z + CHUNK_Z).min(self.depth),
            ),
        ))
    }

    pub fn chunk_version(&self, chunk: ChunkId) -> u32 {
        self.versions.get(chunk as usize).copied().unwrap_or(0)
    }

    /// Visits the real tiles of a chunk in level, row, column order.
    pub fn for_each_in_chunk(&self, chunk: ChunkId, mut visit: impl FnMut(Pos3, &T)) {
        let Some((low, high)) = self.chunk_bounds(chunk) else {
            return;
        };
        let base = chunk as usize * CHUNK_VOLUME;
        for z in low.z..high.z {
            for y in low.y..high.y {
                let row = base + (((z - low.z) * CHUNK_Y + (y - low.y)) * CHUNK_X) as usize;
                for x in low.x..high.x {
                    visit(Pos3::new(x, y, z), &self.values[row + (x - low.x) as usize]);
                }
            }
        }
    }

    /// Real tiles in logical order, independent of the chunk layout.
    pub fn iter_logical(&self) -> impl Iterator<Item = (Pos3, &T)> {
        (0..self.depth).flat_map(move |z| {
            (0..self.height).flat_map(move |y| {
                (0..self.width).map(move |x| {
                    let p = Pos3::new(x, y, z);
                    (p, &self.values[self.physical(p)])
                })
            })
        })
    }

    /// Moves the chunks changed since the last call into `out` in Morton order.
    pub fn take_changed_chunks(&mut self, out: &mut Vec<ChunkId>) {
        out.clear();
        out.append(&mut self.changed_list);
        out.sort_unstable();
        for &chunk in out.iter() {
            self.changed[chunk as usize] = false;
        }
    }

    pub fn has_changes(&self) -> bool {
        !self.changed_list.is_empty()
    }

    pub fn clear_changes(&mut self) {
        for chunk in self.changed_list.drain(..) {
            self.changed[chunk as usize] = false;
        }
    }
}
