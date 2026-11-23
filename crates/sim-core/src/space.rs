use crate::entity::EntityId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
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
    offsets: Vec<usize>,
    cursors: Vec<usize>,
    entries: Vec<(SpatialHit, Point)>,
}

impl SpatialIndex {
    pub fn new(width: i32, height: i32, cell_size: i32, capacity: usize) -> Option<Self> {
        if cell_size <= 0 || width <= 0 || height <= 0 {
            return None;
        }
        let columns = (width - 1) / cell_size + 1;
        let rows = (height - 1) / cell_size + 1;
        let cells = area(columns, rows)?;
        Some(Self {
            width,
            height,
            cell_size,
            columns: columns as usize,
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

    pub fn rebuild(&mut self, entries: impl Iterator<Item = (SpatialHit, Point)> + Clone) {
        self.offsets.fill(0);
        for (_, point) in entries.clone() {
            if let Some(cell) = self.cell(point) {
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
        for (id, point) in entries {
            if let Some(cell) = self.cell(point) {
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

    pub fn query_radius(&self, center: Point, radius: u32, out: &mut Vec<SpatialHit>) -> usize {
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
                let cell = row as usize * self.columns + col as usize;
                candidates += self.offsets[cell + 1] - self.offsets[cell];
                for &(id, point) in &self.entries[self.offsets[cell]..self.offsets[cell + 1]] {
                    if center.distance_squared(point) <= squared {
                        out.push(id);
                    }
                }
            }
        }
        out.sort_unstable_by_key(|hit| hit.id);
        candidates
    }
}
