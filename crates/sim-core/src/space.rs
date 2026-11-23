use crate::entity::EntityId;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    /// Each difference has at most 32 bits, so two squared differences fit in u128.
    pub fn distance_squared(self, other: Self) -> u128 {
        let dx = i64::from(self.x).abs_diff(i64::from(other.x)) as u128;
        let dy = i64::from(self.y).abs_diff(i64::from(other.y)) as u128;
        dx * dx + dy * dy
    }
}

pub struct Grid2<T> {
    width: i32,
    height: i32,
    values: Vec<T>,
}

impl<T: Clone> Grid2<T> {
    pub fn new(width: i32, height: i32, value: T) -> Option<Self> {
        let len = area(width, height)?;
        let mut values = Vec::new();
        values.try_reserve_exact(len).ok()?;
        values.resize(len, value);
        Some(Self {
            width,
            height,
            values,
        })
    }
}

impl<T> Grid2<T> {
    pub fn in_bounds(&self, point: Point) -> bool {
        (0..self.width).contains(&point.x) && (0..self.height).contains(&point.y)
    }

    pub fn index(&self, point: Point) -> Option<usize> {
        self.in_bounds(point)
            .then(|| point.y as usize * self.width as usize + point.x as usize)
    }

    pub fn get(&self, point: Point) -> Option<&T> {
        self.values.get(self.index(point)?)
    }

    pub fn set(&mut self, point: Point, value: T) -> bool {
        let Some(index) = self.index(point) else {
            return false;
        };
        self.values[index] = value;
        true
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values
    }
}

fn area(width: i32, height: i32) -> Option<usize> {
    if width <= 0 || height <= 0 {
        return None;
    }
    (width as usize).checked_mul(height as usize)
}

/// Stable query identity and the associated slot in the current snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpatialHit {
    pub id: EntityId,
    pub slot: u32,
}

pub struct SpatialIndex {
    width: i32,
    height: i32,
    cell_size: i32,
    columns: usize,
    categories: usize,
    offsets: Vec<usize>,
    cursors: Vec<usize>,
    entries: Vec<(SpatialHit, Point)>,
}

impl SpatialIndex {
    pub fn new(
        width: i32,
        height: i32,
        cell_size: i32,
        categories: u8,
        capacity: usize,
    ) -> Option<Self> {
        if cell_size <= 0 || width <= 0 || height <= 0 || !(1..=64).contains(&categories) {
            return None;
        }
        let columns = (width - 1) / cell_size + 1;
        let rows = (height - 1) / cell_size + 1;
        let categories = categories as usize;
        let cells = area(columns, rows)?.checked_mul(categories)?;
        Some(Self {
            width,
            height,
            cell_size,
            columns: columns as usize,
            categories,
            offsets: vec![0; cells.checked_add(1)?],
            cursors: vec![0; cells],
            entries: Vec::with_capacity(capacity),
        })
    }

    fn cell(&self, point: Point) -> Option<usize> {
        ((0..self.width).contains(&point.x) && (0..self.height).contains(&point.y)).then(|| {
            (point.y / self.cell_size) as usize * self.columns + (point.x / self.cell_size) as usize
        })
    }

    fn cell_category(&self, point: Point, category: u8) -> Option<usize> {
        let category = category as usize;
        (category < self.categories).then_some(self.cell(point)? * self.categories + category)
    }

    /// Both passes must enumerate the same entries. The counts bound the cursors.
    pub fn rebuild(&mut self, entries: impl Iterator<Item = (SpatialHit, Point, u8)> + Clone) {
        self.offsets.fill(0);
        for (_, point, category) in entries.clone() {
            if let Some(cell) = self.cell_category(point, category) {
                self.offsets[cell + 1] += 1;
            }
        }
        for i in 1..self.offsets.len() {
            self.offsets[i] += self.offsets[i - 1];
        }
        let len = self.offsets[self.cursors.len()];
        self.entries.resize(
            len,
            (
                SpatialHit {
                    id: EntityId::from_u64(0),
                    slot: 0,
                },
                Point { x: 0, y: 0 },
            ),
        );
        let cells = self.cursors.len();
        self.cursors.copy_from_slice(&self.offsets[..cells]);
        for (id, point, category) in entries {
            if let Some(cell) = self.cell_category(point, category) {
                self.entries[self.cursors[cell]] = (id, point);
                self.cursors[cell] += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn query_radius(
        &self,
        center: Point,
        radius: u32,
        category_mask: u64,
        out: &mut Vec<SpatialHit>,
    ) -> usize {
        out.clear();
        let radius = i64::from(radius);
        let min_x = (i64::from(center.x) - radius).max(0);
        let max_x = (i64::from(center.x) + radius).min(i64::from(self.width - 1));
        let min_y = (i64::from(center.y) - radius).max(0);
        let max_y = (i64::from(center.y) + radius).min(i64::from(self.height - 1));
        if min_x > max_x || min_y > max_y {
            return 0;
        }
        let size = i64::from(self.cell_size);
        let squared = radius as u128 * radius as u128;
        let mut candidates = 0;
        for row in min_y / size..=max_y / size {
            for col in min_x / size..=max_x / size {
                let start = (row as usize * self.columns + col as usize) * self.categories;
                let mut mask = category_mask & (u64::MAX >> (64 - self.categories));
                while mask != 0 {
                    let cell = start + mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    let entries = &self.entries[self.offsets[cell]..self.offsets[cell + 1]];
                    candidates += entries.len();
                    for &(hit, point) in entries {
                        if center.distance_squared(point) <= squared {
                            out.push(hit);
                        }
                    }
                }
            }
        }
        out.sort_unstable_by_key(|hit| hit.id);
        candidates
    }
}

pub const CHUNK_SIZE: i32 = 32;

/// Interleaves two nonnegative coordinates in Morton order.
pub fn morton_code(x: u32, y: u32) -> u64 {
    fn spread(x: u32) -> u64 {
        let mut x = u64::from(x);
        x = (x | (x << 16)) & 0x0000ffff0000ffff;
        x = (x | (x << 8)) & 0x00ff00ff00ff00ff;
        x = (x | (x << 4)) & 0x0f0f0f0f0f0f0f0f;
        x = (x | (x << 2)) & 0x3333333333333333;
        (x | (x << 1)) & 0x5555555555555555
    }
    spread(x) | (spread(y) << 1)
}

/// A rectangular grid with padded 32×32 blocks stored in Morton order.
/// Public indices and the default iteration retain logical row order.
pub struct ChunkedGrid2<T> {
    width: i32,
    height: i32,
    columns: usize,
    len: usize,
    blocks: Vec<(i32, i32)>,
    block_at: Vec<usize>,
    values: Vec<T>,
}

impl<T: Clone> ChunkedGrid2<T> {
    pub fn new(width: i32, height: i32, value: T) -> Option<Self> {
        let len = area(width, height)?;
        let columns = (width - 1) / CHUNK_SIZE + 1;
        let rows = (height - 1) / CHUNK_SIZE + 1;
        let count = area(columns, rows)?;
        let padded = count.checked_mul((CHUNK_SIZE * CHUNK_SIZE) as usize)?;
        let mut blocks = Vec::new();
        let mut block_at = Vec::new();
        let mut values = Vec::new();
        blocks.try_reserve_exact(count).ok()?;
        block_at.try_reserve_exact(count).ok()?;
        values.try_reserve_exact(padded).ok()?;
        for y in 0..rows {
            for x in 0..columns {
                blocks.push((x, y));
            }
        }
        blocks.sort_unstable_by_key(|&(x, y)| morton_code(x as u32, y as u32));
        block_at.resize(count, 0);
        for (block, &(x, y)) in blocks.iter().enumerate() {
            block_at[y as usize * columns as usize + x as usize] = block;
        }
        values.resize(padded, value);
        Some(Self {
            width,
            height,
            columns: columns as usize,
            len,
            blocks,
            block_at,
            values,
        })
    }
}

impl<T> ChunkedGrid2<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn in_bounds(&self, point: Point) -> bool {
        (0..self.width).contains(&point.x) && (0..self.height).contains(&point.y)
    }

    pub fn index(&self, point: Point) -> Option<usize> {
        self.in_bounds(point)
            .then(|| point.y as usize * self.width as usize + point.x as usize)
    }

    fn physical(&self, point: Point) -> usize {
        let size = CHUNK_SIZE as usize;
        let x = point.x as usize;
        let y = point.y as usize;
        let block = self.block_at[(y / size) * self.columns + x / size];
        (block * size + y % size) * size + x % size
    }

    pub fn get(&self, point: Point) -> Option<&T> {
        self.in_bounds(point)
            .then(|| &self.values[self.physical(point)])
    }

    pub fn get_mut(&mut self, point: Point) -> Option<&mut T> {
        let index = self.in_bounds(point).then(|| self.physical(point))?;
        self.values.get_mut(index)
    }

    pub fn set(&mut self, point: Point, value: T) -> bool {
        if !self.in_bounds(point) {
            return false;
        }
        let index = self.physical(point);
        self.values[index] = value;
        true
    }

    pub fn get_index(&self, index: usize) -> Option<&T> {
        let width = self.width as usize;
        (index < self.len).then(|| {
            &self.values[self.physical(Point {
                x: (index % width) as i32,
                y: (index / width) as i32,
            })]
        })
    }

    pub fn get_index_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        let width = self.width as usize;
        let physical = self.physical(Point {
            x: (index % width) as i32,
            y: (index / width) as i32,
        });
        self.values.get_mut(physical)
    }

    /// Real cells, excluding block padding, in logical row order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.height).flat_map(move |y| {
            (0..self.width).map(move |x| &self.values[self.physical(Point { x, y })])
        })
    }

    /// Visits real row segments in physical block order. The first index is logical.
    pub fn for_each_block_row_mut(&mut self, mut visit: impl FnMut(usize, &mut [T])) {
        let size = CHUNK_SIZE as usize;
        for (&(bx, by), block) in self
            .blocks
            .iter()
            .zip(self.values.chunks_exact_mut(size * size))
        {
            let x = bx * CHUNK_SIZE;
            let y = by * CHUNK_SIZE;
            let width = (self.width - x).min(CHUNK_SIZE) as usize;
            let height = (self.height - y).min(CHUNK_SIZE) as usize;
            for (offset, row) in block.chunks_exact_mut(size).take(height).enumerate() {
                visit(
                    (y as usize + offset) * self.width as usize + x as usize,
                    &mut row[..width],
                );
            }
        }
    }
}
