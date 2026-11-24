//! Incremental index of positioned objects by category and chunk.
//!
//! Every object occupies a dense slot (for example the index of its generational ID).
//! Per category and chunk the index keeps a count and an intrusive list, so inserting,
//! moving and removing never allocate after construction. List order depends on the
//! update history and is never observable: `nearest` returns the minimum of
//! `(distance, key)` over all accepted objects, independent of that order.

use crate::{
    space::{CHUNK_X, CHUNK_Y, CHUNK_Z, Pos3},
    topology::layered_heuristic,
};

const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct Entry<K> {
    key: K,
    pos: Pos3,
    category: u8,
    list: u32,
}

/// Search bounds of `nearest_any`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub cells: u32,
    pub candidates: u32,
}

/// Outcome of a nearest-object query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nearest<K> {
    /// Minimum `(distance, key)` among accepted objects with the object's position.
    pub found: Option<(u32, K, Pos3)>,
    /// False if `max_cells` stopped the search before the result was certain.
    pub complete: bool,
    pub cells: u32,
    pub candidates: u32,
}

pub struct CategoryIndex<K> {
    columns: i32,
    rows: i32,
    layers: i32,
    categories: u8,
    heads: Vec<u32>,
    counts: Vec<u32>,
    totals: Vec<u32>,
    next: Vec<u32>,
    prev: Vec<u32>,
    entries: Vec<Option<Entry<K>>>,
}

/// Distance metric of the index: the admissible layered walking estimate. It is
/// nondecreasing in each coordinate difference, so box distances give lower bounds.
pub fn index_distance(a: Pos3, b: Pos3) -> u32 {
    layered_heuristic(a.x.abs_diff(b.x), a.y.abs_diff(b.y), a.z.abs_diff(b.z))
}

fn axis_gap(value: i32, low: i32, size: i32) -> u32 {
    if value < low {
        low.abs_diff(value)
    } else if value >= low + size {
        value.abs_diff(low + size - 1)
    } else {
        0
    }
}

impl<K: Copy + Ord> CategoryIndex<K> {
    pub fn new(width: i32, height: i32, depth: i32, categories: u8, capacity: u32) -> Option<Self> {
        if width <= 0 || height <= 0 || depth <= 0 || categories == 0 {
            return None;
        }
        let columns = (width - 1) / CHUNK_X + 1;
        let rows = (height - 1) / CHUNK_Y + 1;
        let layers = (depth - 1) / CHUNK_Z + 1;
        let lists = (columns as usize)
            .checked_mul(rows as usize)?
            .checked_mul(layers as usize)?
            .checked_mul(usize::from(categories))?;
        Some(Self {
            columns,
            rows,
            layers,
            categories,
            heads: vec![NONE; lists],
            counts: vec![0; lists],
            totals: vec![0; usize::from(categories)],
            next: vec![NONE; capacity as usize],
            prev: vec![NONE; capacity as usize],
            entries: vec![None; capacity as usize],
        })
    }

    fn cell(&self, p: Pos3) -> (i32, i32, i32) {
        (
            (p.x / CHUNK_X).clamp(0, self.columns - 1),
            (p.y / CHUNK_Y).clamp(0, self.rows - 1),
            (p.z / CHUNK_Z).clamp(0, self.layers - 1),
        )
    }

    fn list(&self, (x, y, z): (i32, i32, i32), category: u8) -> usize {
        (((z * self.rows + y) * self.columns + x) as usize) * usize::from(self.categories)
            + usize::from(category)
    }

    pub fn clear(&mut self) {
        self.heads.fill(NONE);
        self.counts.fill(0);
        self.totals.fill(0);
        self.next.fill(NONE);
        self.prev.fill(NONE);
        self.entries.fill(None);
    }

    pub fn count(&self, category: u8) -> u32 {
        self.totals.get(usize::from(category)).copied().unwrap_or(0)
    }

    pub fn get(&self, slot: u32) -> Option<(K, Pos3, u8)> {
        let entry = (*self.entries.get(slot as usize)?)?;
        Some((entry.key, entry.pos, entry.category))
    }

    /// Inserts or replaces the object in `slot`.
    pub fn insert(&mut self, slot: u32, key: K, pos: Pos3, category: u8) -> bool {
        if slot as usize >= self.entries.len() || category >= self.categories {
            return false;
        }
        self.remove(slot);
        let list = self.list(self.cell(pos), category);
        let s = slot as usize;
        let head = self.heads[list];
        self.next[s] = head;
        self.prev[s] = NONE;
        if head != NONE {
            self.prev[head as usize] = slot;
        }
        self.heads[list] = slot;
        self.counts[list] += 1;
        self.totals[usize::from(category)] += 1;
        self.entries[s] = Some(Entry {
            key,
            pos,
            category,
            list: list as u32,
        });
        true
    }

    pub fn remove(&mut self, slot: u32) -> bool {
        let s = slot as usize;
        let Some(entry) = self.entries.get_mut(s).and_then(Option::take) else {
            return false;
        };
        let list = entry.list as usize;
        let (prev, next) = (self.prev[s], self.next[s]);
        if prev == NONE {
            self.heads[list] = next;
        } else {
            self.next[prev as usize] = next;
        }
        if next != NONE {
            self.prev[next as usize] = prev;
        }
        self.next[s] = NONE;
        self.prev[s] = NONE;
        self.counts[list] -= 1;
        self.totals[usize::from(entry.category)] -= 1;
        true
    }

    /// Moves an indexed object to a new position.
    pub fn relocate(&mut self, slot: u32, pos: Pos3) -> bool {
        match self.get(slot) {
            Some((key, _, category)) => self.insert(slot, key, pos, category),
            None => false,
        }
    }

    /// Whether any indexed object of any category lies exactly at `pos`.
    pub fn any_at(&self, pos: Pos3) -> bool {
        self.count_at(pos, 1) > 0
    }

    /// Objects of all categories exactly at `pos`, counting at most `limit`.
    pub fn count_at(&self, pos: Pos3, limit: u32) -> u32 {
        let cell = self.cell(pos);
        let mut count = 0;
        for category in 0..self.categories {
            let mut slot = self.heads[self.list(cell, category)];
            while slot != NONE && count < limit {
                if self.entries[slot as usize].is_some_and(|e| e.pos == pos) {
                    count += 1;
                }
                slot = self.next[slot as usize];
            }
        }
        count
    }

    /// Calls `visit` for every object of `category` exactly at `pos`.
    pub fn for_each_at(&self, pos: Pos3, category: u8, mut visit: impl FnMut(K)) {
        if category >= self.categories {
            return;
        }
        let mut slot = self.heads[self.list(self.cell(pos), category)];
        while slot != NONE {
            if let Some(entry) = self.entries[slot as usize]
                && entry.pos == pos
            {
                visit(entry.key);
            }
            slot = self.next[slot as usize];
        }
    }

    /// Nearest accepted object of `category` by `(index_distance, key)`. Cells are
    /// visited in Chebyshev rings around `from`; the search stops once the lower
    /// bound of the next ring exceeds the best distance, so a complete result equals
    /// an exhaustive search. At most `max_cells` cells are visited.
    pub fn nearest(
        &self,
        from: Pos3,
        category: u8,
        max_cells: u32,
        accept: impl FnMut(K, Pos3) -> bool,
    ) -> Nearest<K> {
        if category >= 64 || category >= self.categories {
            return Nearest {
                found: None,
                complete: true,
                cells: 0,
                candidates: 0,
            };
        }
        let limits = Limits {
            cells: max_cells,
            candidates: u32::MAX,
        };
        self.nearest_any(from, 1 << category, limits, accept)
    }

    /// Like `nearest` over every category in the bit mask `categories` (categories
    /// below 64). Besides the cells, `limits.candidates` bounds the inspected objects;
    /// hitting either limit returns an incomplete result.
    pub fn nearest_any(
        &self,
        from: Pos3,
        categories: u64,
        limits: Limits,
        mut accept: impl FnMut(K, Pos3) -> bool,
    ) -> Nearest<K> {
        let mut result = Nearest {
            found: None,
            complete: true,
            cells: 0,
            candidates: 0,
        };
        let wanted: u64 = (0..self.categories.min(64))
            .filter(|&c| categories & (1 << c) != 0 && self.count(c) > 0)
            .fold(0, |mask, c| mask | 1 << c);
        if wanted == 0 {
            return result;
        }
        let max_cells = limits.cells;
        let (cx, cy, cz) = self.cell(from);
        let max_ring = cx
            .max(self.columns - 1 - cx)
            .max(cy)
            .max(self.rows - 1 - cy)
            .max(cz)
            .max(self.layers - 1 - cz);
        let origin = Pos3::new(0, 0, 0);
        for ring in 0..=max_ring {
            if ring > 0
                && let Some((best, _, _)) = result.found
            {
                let lower = index_distance(origin, Pos3::new((ring - 1) * CHUNK_X + 1, 0, 0))
                    .min(index_distance(
                        origin,
                        Pos3::new(0, (ring - 1) * CHUNK_Y + 1, 0),
                    ))
                    .min(index_distance(
                        origin,
                        Pos3::new(0, 0, (ring - 1) * CHUNK_Z + 1),
                    ));
                if lower > best {
                    break;
                }
            }
            for z in cz - ring..=cz + ring {
                if !(0..self.layers).contains(&z) {
                    continue;
                }
                for y in cy - ring..=cy + ring {
                    if !(0..self.rows).contains(&y) {
                        continue;
                    }
                    let face = (z - cz).abs() == ring || (y - cy).abs() == ring;
                    let step = if face || ring == 0 {
                        1
                    } else {
                        2 * ring as usize
                    };
                    for x in (cx - ring..=cx + ring).step_by(step) {
                        if !(0..self.columns).contains(&x) {
                            continue;
                        }
                        let occupied = (0..self.categories.min(64)).any(|c| {
                            wanted & (1 << c) != 0 && self.counts[self.list((x, y, z), c)] != 0
                        });
                        if !occupied {
                            continue;
                        }
                        if result.cells >= max_cells {
                            result.complete = false;
                            return result;
                        }
                        result.cells += 1;
                        let gap = index_distance(
                            origin,
                            Pos3::new(
                                axis_gap(from.x, x * CHUNK_X, CHUNK_X) as i32,
                                axis_gap(from.y, y * CHUNK_Y, CHUNK_Y) as i32,
                                axis_gap(from.z, z * CHUNK_Z, CHUNK_Z) as i32,
                            ),
                        );
                        if result.found.is_some_and(|(best, _, _)| gap > best) {
                            continue;
                        }
                        for category in 0..self.categories.min(64) {
                            if wanted & (1 << category) == 0 {
                                continue;
                            }
                            let mut slot = self.heads[self.list((x, y, z), category)];
                            while slot != NONE {
                                if let Some(entry) = self.entries[slot as usize] {
                                    if result.candidates >= limits.candidates {
                                        result.complete = false;
                                        return result;
                                    }
                                    result.candidates += 1;
                                    let distance = index_distance(from, entry.pos);
                                    let better = result
                                        .found
                                        .is_none_or(|(d, k, _)| (distance, entry.key) < (d, k));
                                    if better && accept(entry.key, entry.pos) {
                                        result.found = Some((distance, entry.key, entry.pos));
                                    }
                                }
                                slot = self.next[slot as usize];
                            }
                        }
                    }
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{SystemRng, below};

    #[test]
    fn nearest_matches_brute_force_after_random_updates() {
        let (w, h, d) = (70, 50, 20);
        for seed in [1, 2, 3, 42] {
            let mut rng = SystemRng::new(seed, 71);
            let capacity = 400;
            let mut index: CategoryIndex<(u32, u32)> =
                CategoryIndex::new(w, h, d, 3, capacity).unwrap();
            type Model = Option<((u32, u32), Pos3, u8)>;
            let mut model: Vec<Model> = vec![None; capacity as usize];
            let random_pos = |rng: &mut SystemRng| {
                Pos3::new(
                    below(rng.next_u64(), w as u64) as i32,
                    below(rng.next_u64(), h as u64) as i32,
                    below(rng.next_u64(), d as u64) as i32,
                )
            };
            for step in 0..3000u32 {
                let slot = below(rng.next_u64(), u64::from(capacity)) as u32;
                match below(rng.next_u64(), 4) {
                    0 | 1 => {
                        let key = (slot, step);
                        let pos = random_pos(&mut rng);
                        let category = below(rng.next_u64(), 3) as u8;
                        assert!(index.insert(slot, key, pos, category));
                        model[slot as usize] = Some((key, pos, category));
                    }
                    2 => {
                        let pos = random_pos(&mut rng);
                        let moved = index.relocate(slot, pos);
                        assert_eq!(moved, model[slot as usize].is_some());
                        if let Some(entry) = &mut model[slot as usize] {
                            entry.1 = pos;
                        }
                    }
                    _ => {
                        assert_eq!(index.remove(slot), model[slot as usize].take().is_some());
                    }
                }
                if step % 7 == 0 {
                    let from = random_pos(&mut rng);
                    let category = below(rng.next_u64(), 3) as u8;
                    let parity = below(rng.next_u64(), 3);
                    let accept = |key: (u32, u32), pos: Pos3| {
                        !(u64::from(key.1) + parity).is_multiple_of(3) && pos.z % 4 != 1
                    };
                    let expected = model
                        .iter()
                        .flatten()
                        .filter(|(key, pos, c)| *c == category && accept(*key, *pos))
                        .map(|&(key, pos, _)| (index_distance(from, pos), key, pos))
                        .min_by_key(|&(d, key, _)| (d, key));
                    let result = index.nearest(from, category, u32::MAX, accept);
                    assert!(result.complete);
                    assert_eq!(result.found, expected, "seed {seed} step {step}");
                    assert_eq!(
                        index.count(category) as usize,
                        model.iter().flatten().filter(|e| e.2 == category).count()
                    );
                }
            }
        }
    }

    #[test]
    fn cell_limit_reports_incomplete_searches() {
        let mut index: CategoryIndex<u32> = CategoryIndex::new(64, 64, 8, 1, 8).unwrap();
        index.insert(0, 0, Pos3::new(63, 63, 0), 0);
        index.insert(1, 1, Pos3::new(0, 0, 0), 0);
        let limited = index.nearest(Pos3::new(63, 63, 0), 0, 1, |key, _| key == 1);
        assert!(!limited.complete && limited.found.is_none());
        let full = index.nearest(Pos3::new(63, 63, 0), 0, u32::MAX, |key, _| key == 1);
        assert_eq!(full.found, Some((882, 1, Pos3::new(0, 0, 0))));
        assert!(index.any_at(Pos3::new(63, 63, 0)) && !index.any_at(Pos3::new(62, 63, 0)));
        assert!(!index.insert(8, 0, Pos3::new(0, 0, 0), 0));
        assert!(!index.insert(0, 0, Pos3::new(0, 0, 0), 1));
    }
}

#[cfg(test)]
mod any_tests {
    use super::*;
    use crate::rng::{SystemRng, below};

    #[test]
    fn nearest_any_matches_brute_force_over_category_sets() {
        let (w, h, d) = (60, 60, 16);
        let capacity = 500u32;
        for seed in [3, 5, 8] {
            let mut rng = SystemRng::new(seed, 13);
            let mut index: CategoryIndex<u32> = CategoryIndex::new(w, h, d, 6, capacity).unwrap();
            let mut model: Vec<Option<(Pos3, u8)>> = vec![None; capacity as usize];
            for step in 0..4000u32 {
                let slot = below(rng.next_u64(), u64::from(capacity)) as u32;
                if below(rng.next_u64(), 4) == 0 {
                    index.remove(slot);
                    model[slot as usize] = None;
                } else {
                    let pos = Pos3::new(
                        below(rng.next_u64(), w as u64) as i32,
                        below(rng.next_u64(), h as u64) as i32,
                        below(rng.next_u64(), d as u64) as i32,
                    );
                    let category = below(rng.next_u64(), 6) as u8;
                    index.insert(slot, slot, pos, category);
                    model[slot as usize] = Some((pos, category));
                }
                if step % 11 == 0 {
                    let from = Pos3::new(
                        below(rng.next_u64(), w as u64) as i32,
                        below(rng.next_u64(), h as u64) as i32,
                        below(rng.next_u64(), d as u64) as i32,
                    );
                    let mask = below(rng.next_u64(), 64);
                    let accept = |key: u32, _: Pos3| !key.is_multiple_of(5);
                    let expected = model
                        .iter()
                        .enumerate()
                        .filter_map(|(k, e)| e.map(|(p, c)| (k as u32, p, c)))
                        .filter(|&(k, p, c)| mask & (1 << c) != 0 && accept(k, p))
                        .map(|(k, p, _)| (index_distance(from, p), k, p))
                        .min_by_key(|&(dist, k, _)| (dist, k));
                    let unlimited = Limits {
                        cells: u32::MAX,
                        candidates: u32::MAX,
                    };
                    let result = index.nearest_any(from, mask, unlimited, accept);
                    assert!(result.complete);
                    assert_eq!(result.found, expected, "seed {seed} step {step}");
                    let tight = Limits {
                        cells: u32::MAX,
                        candidates: 3,
                    };
                    let limited = index.nearest_any(from, mask, tight, accept);
                    assert!(limited.candidates <= 3);
                    if limited.complete {
                        assert_eq!(limited.found, expected);
                    }
                }
            }
            let mut at = Vec::new();
            let probe = model.iter().flatten().next().map(|e| e.0).unwrap();
            for category in 0..6 {
                index.for_each_at(probe, category, |k| at.push(k));
            }
            at.sort_unstable();
            let expected: Vec<u32> = (0..capacity)
                .filter(|&k| model[k as usize].is_some_and(|(p, _)| p == probe))
                .collect();
            assert_eq!(at, expected);
        }
    }
}
