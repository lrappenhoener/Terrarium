//! Stockpile zones and transport.
//!
//! Each stockpile tile belongs to a zone with category and material filters, a
//! priority and a container option. Transport is demand-driven: a staggered scan
//! proposes loose items and items in lower-priority zones, and a haul job is created
//! only if a destination with room exists, at most `hauls_per_tick` per tick and
//! `max_open_hauls` open at once. Ingredients reach workshops through their
//! production jobs, which carry them themselves.
//!
//! A haul picks its destination when it is assigned: higher priority first, then
//! distance, then position. If an empty bulk container and other haulable items lie
//! near the item, the hauler reserves container, items and destination together and
//! carries them all at once; the filled container then stands on the stockpile.

use sim_core::{
    jobs::{JobId, JobState, job_distance},
    space::{Pos3, standable},
};

use crate::{
    FortressSim, HAUL_PRIORITY, ItemId, ResKey, ResidentId, Route, STOCKPILE_CAPACITY, box_range,
    box_volume,
    types::{ItemLocation, JobKind, Task, Zone},
};

pub(crate) const ALL_CATEGORIES: u64 = u64::MAX;

/// Chebyshev distance in tiles.
fn tiles_apart(a: Pos3, b: Pos3) -> u32 {
    a.x.abs_diff(b.x)
        .max(a.y.abs_diff(b.y))
        .max(a.z.abs_diff(b.z))
}
/// Stage of `Task::BulkHaul` that chooses the next item.
pub(crate) const BULK_PLAN: u8 = 3;

impl FortressSim {
    pub(crate) fn zone_index(&self, id: u32) -> Option<usize> {
        self.zones.binary_search_by_key(&id, |z| z.id).ok()
    }

    pub(crate) fn slot_index(&self, p: Pos3) -> Option<usize> {
        self.stockpiles.binary_search(&p).ok()
    }

    fn zone_of_slot(&self, slot: usize) -> Option<&Zone> {
        self.zone_index(self.slot_zone[slot])
            .map(|i| &self.zones[i])
    }

    /// Whether `zone` accepts the item in column slot `islot`.
    fn zone_accepts(&self, zone: &Zone, islot: usize) -> bool {
        let kind = self.icols.kind[islot].0;
        let ancestors = self
            .content
            .categories
            .get(usize::from(self.content.category_of(kind)))
            .map_or(0, |c| c.ancestors);
        ancestors & zone.categories != 0
            && (zone.material_count == 0
                || zone
                    .materials()
                    .binary_search(&self.icols.material[islot])
                    .is_ok())
    }

    fn is_container(&self, islot: usize) -> bool {
        self.content
            .item(self.icols.kind[islot].0)
            .is_some_and(|d| d.capacity > 0)
    }

    /// Updates the derived ground position of an item and the stockpile counts.
    pub(crate) fn track_ground(&mut self, id: ItemId, now: Option<Pos3>) {
        let Some(entry) = self.ground_at.get_mut(id.index as usize) else {
            return;
        };
        let before = std::mem::replace(entry, now);
        if before == now {
            return;
        }
        if let Some(i) = before.and_then(|p| self.slot_index(p)) {
            self.slot_items[i] -= 1;
        }
        if let Some(i) = now.and_then(|p| self.slot_index(p)) {
            self.slot_items[i] += 1;
        }
    }

    /// Items lying directly on `p`, by their derived ground positions.
    fn count_ground_items(&self, p: Pos3) -> u32 {
        let mut count = 0;
        for category in 0..self.content.categories.len() as u8 {
            self.item_index.for_each_at(p, category, |id| {
                if self.ground_at.get(id.index as usize) == Some(&Some(p)) {
                    count += 1;
                }
            });
        }
        count
    }

    /// The unreserved container with room on `p`, lowest ID first.
    pub(crate) fn slot_container(&self, p: Pos3) -> Option<ItemId> {
        let mut best: Option<ItemId> = None;
        for category in 0..self.content.categories.len() as u8 {
            self.item_index.for_each_at(p, category, |id| {
                let Some(s) = self.islot(id) else { return };
                let room = self
                    .content
                    .item(self.icols.kind[s].0)
                    .is_some_and(|d| self.contents.count(id.index) < u32::from(d.capacity));
                if self.icols.location[s] == ItemLocation::Ground(p)
                    && room
                    && !self.reservations.is_reserved(ResKey::Item(id))
                    && best.is_none_or(|b| id < b)
                {
                    best = Some(id);
                }
            });
        }
        best
    }

    fn slot_takes(&self, slot: usize, zone: &Zone) -> bool {
        let p = self.stockpiles[slot];
        !self.reservations.is_reserved(ResKey::Slot(p))
            && (self.slot_items[slot] == 0 || (zone.containers && self.slot_container(p).is_some()))
    }

    /// Best stockpile tile for an item at `from`: accepted, with room, reachable,
    /// in a zone of higher priority than the item's current zone; by priority, then
    /// distance, then position.
    pub(crate) fn destination(&self, item: ItemId, from: Pos3) -> Option<Pos3> {
        let islot = self.islot(item)?;
        let floor = self
            .slot_index(from)
            .filter(|_| self.icols.location[islot] == ItemLocation::Ground(from))
            .and_then(|s| self.zone_of_slot(s))
            .map(|z| i16::from(z.priority));
        let mut best: Option<(std::cmp::Reverse<u8>, u32, Pos3)> = None;
        for slot in 0..self.stockpiles.len() {
            let Some(zone) = self.zone_of_slot(slot) else {
                continue;
            };
            let p = self.stockpiles[slot];
            if floor.is_some_and(|f| i16::from(zone.priority) <= f)
                || !self.zone_accepts(zone, islot)
                || (self.is_container(islot) && self.slot_items[slot] != 0)
            {
                continue;
            }
            let key = (std::cmp::Reverse(zone.priority), job_distance(from, p), p);
            if best.is_some_and(|b| key >= b) || !self.slot_takes(slot, zone) {
                continue;
            }
            if self.connectivity.reachable(from, p) {
                best = Some(key);
            }
        }
        best.map(|(_, _, p)| p)
    }

    /// Phase work of the lifecycle: proposes staggered items for transport and
    /// creates capped haul jobs where a destination exists.
    pub(crate) fn plan_transport(&mut self) {
        let interval = self.config.haul_scan_interval;
        let mut candidates = std::mem::take(&mut self.scratch.candidates);
        candidates.clear();
        for slot in 0..self.icols.len() {
            if let Some(id) = self.items.id(slot as u32)
                && self.icols.haul[slot].is_none()
                && matches!(self.icols.location[slot], ItemLocation::Ground(_))
                && sim_core::schedule::staggered(self.tick, interval, u64::from(id.index))
            {
                candidates.push(id);
            }
        }
        if candidates.is_empty() {
            self.scratch.candidates = candidates;
            return;
        }
        candidates.sort_unstable();
        let targets = &mut self.scratch.job_positions;
        targets.clear();
        targets.extend(
            self.jobs
                .iter()
                .filter(|j| matches!(j.data, JobKind::Build(_)) && j.state != JobState::Done)
                .map(|j| j.pos),
        );
        targets.sort_unstable();
        let mut open = self
            .jobs
            .iter()
            .filter(|j| matches!(j.data, JobKind::Haul(_)) && j.state != JobState::Done)
            .count() as u32;
        let mut created = 0;
        for &id in &candidates {
            if open >= self.config.max_open_hauls || created >= self.config.hauls_per_tick {
                self.diag.hauls_deferred += 1;
                continue;
            }
            let Some(ItemLocation::Ground(p)) = self.islot(id).map(|s| self.icols.location[s])
            else {
                continue;
            };
            if self.reservations.is_reserved(ResKey::Item(id)) {
                continue;
            }
            let clearing = self.scratch.job_positions.binary_search(&p).is_ok();
            if !clearing && self.destination(id, p).is_none() {
                continue;
            }
            if let Some(job) = self.jobs.add(HAUL_PRIORITY, p, JobKind::Haul(id))
                && let Some(slot) = self.islot(id)
            {
                self.icols.haul[slot] = Some(job);
                open += 1;
                created += 1;
            }
        }
        self.scratch.candidates = candidates;
    }

    /// Delivers a carried item onto a stockpile tile, into a container there if the
    /// zone allows it, otherwise onto the ground.
    pub(crate) fn deliver(&mut self, slot: usize, item: ItemId, at: Pos3) {
        let container = self
            .slot_index(at)
            .and_then(|s| self.zone_of_slot(s))
            .is_some_and(|z| z.containers)
            .then(|| self.slot_container(at))
            .flatten();
        if let Some(container) = container
            && self.put_into(item, container)
        {
            self.rcols.carried[slot] = None;
            return;
        }
        self.drop_item(slot, item, at);
    }

    /// Chooses an empty bulk container and further items near `from` that `dest`
    /// accepts; returns the container and writes the extra items to `out`.
    pub(crate) fn plan_bulk(
        &self,
        lead: ItemId,
        from: Pos3,
        dest: Pos3,
        out: &mut Vec<ItemId>,
    ) -> Option<ItemId> {
        out.clear();
        let radius = self.config.bulk_radius;
        if radius == 0 || self.content.bulk_categories == 0 {
            return None;
        }
        let dest_slot = self.slot_index(dest)?;
        // The container itself needs the tile.
        if self.slot_items[dest_slot] != 0 {
            return None;
        }
        let zone = self.zone_of_slot(dest_slot)?;
        let islot = self.islot(lead)?;
        if self.is_container(islot) {
            return None;
        }
        let limits = self.search_limits();
        let container = self
            .item_index
            .nearest_any(from, self.content.bulk_categories, limits, |id, p| {
                self.islot(id).is_some_and(|s| {
                    self.content
                        .item(self.icols.kind[s].0)
                        .is_some_and(|d| d.bulk)
                        && self.icols.location[s] == ItemLocation::Ground(p)
                        && self.contents.count(id.index) == 0
                }) && tiles_apart(from, p) <= radius
                    && !self.reservations.is_reserved(ResKey::Item(id))
                    && self.connectivity.reachable(from, p)
            })
            .found?
            .1;
        let capacity = self
            .islot(container)
            .and_then(|s| self.content.item(self.icols.kind[s].0))
            .map_or(0, |d| usize::from(d.capacity));
        while out.len() + 1 < capacity {
            let found = self
                .item_index
                .nearest_any(from, zone.categories, limits, |id, p| {
                    id != lead
                        && id != container
                        && !out.contains(&id)
                        && tiles_apart(from, p) <= radius
                        && !self.is_stockpile(p)
                        && self.islot(id).is_some_and(|s| {
                            self.icols.location[s] == ItemLocation::Ground(p)
                                && !self.is_container(s)
                                && self.zone_accepts(zone, s)
                                && self.icols.haul[s].is_none_or(|job| {
                                    self.jobs
                                        .get(job)
                                        .is_some_and(|j| j.state == JobState::Open)
                                })
                        })
                        && !self.reservations.is_reserved(ResKey::Item(id))
                        && self.connectivity.reachable(from, p)
                })
                .found;
            match found {
                Some((_, id, _)) => out.push(id),
                None => break,
            }
        }
        (!out.is_empty()).then_some(container)
    }

    /// Advances a bulk haul; returns true when the container is delivered.
    pub(crate) fn work_bulk(&mut self, slot: usize, id: ResidentId, task: Task) -> bool {
        let Task::BulkHaul {
            job,
            container,
            item,
            slot: dest,
            stage,
        } = task
        else {
            return false;
        };
        let at = self.rcols.position[slot];
        let lead = match self.jobs.get(job).map(|j| j.data) {
            Some(JobKind::Haul(lead)) => lead,
            _ => {
                self.scratch.abort.push(id);
                return false;
            }
        };
        let set = |sim: &mut Self, item, stage| {
            sim.rcols.task[slot] = Task::BulkHaul {
                job,
                container,
                item,
                slot: dest,
                stage,
            };
            sim.rcols.route[slot] = Route::NONE;
        };
        match stage {
            0 => {
                if self.item_ground(container) != Some(at) {
                    self.scratch.abort.push(id);
                    return false;
                }
                self.pick_up(slot, id, container);
                set(self, None, BULK_PLAN);
                false
            }
            1 => {
                if let Some(item) = item {
                    if self.item_ground(item) == Some(at) && self.put_into(item, container) {
                        if item != lead {
                            self.remove_haul(item);
                        }
                    } else {
                        self.reservations.release(ResKey::Item(item));
                    }
                }
                set(self, None, BULK_PLAN);
                false
            }
            2 => {
                if let Some(carried) = self.rcols.carried[slot].take() {
                    self.place_on_ground(carried, dest);
                }
                let moved = self.contents.count(container.index);
                if let Some(s) = self.islot(lead) {
                    self.icols.haul[s] = None;
                }
                self.complete_job(job);
                self.counters.hauled += u64::from(moved);
                self.counters.completed_jobs += 1;
                true
            }
            _ => {
                let next = self
                    .reservations
                    .iter()
                    .filter(|r| r.holder == id)
                    .filter_map(|r| match r.key {
                        ResKey::Item(i) if i != container => Some(i),
                        _ => None,
                    })
                    .filter(|&i| {
                        self.islot(i).is_some_and(|s| {
                            matches!(self.icols.location[s], ItemLocation::Ground(_))
                        })
                    })
                    .min();
                match next {
                    Some(next) => set(self, Some(next), 1),
                    None => set(self, None, 2),
                }
                false
            }
        }
    }

    /// Designates a zone over the standable, free tiles of a box; returns the
    /// number of rejected tiles.
    pub(crate) fn designate_stockpile(&mut self, from: Pos3, to: Pos3) -> u64 {
        let (low, high) = box_range(from, to);
        let volume = box_volume(low, high);
        if volume > STOCKPILE_CAPACITY as u64 {
            return 1;
        }
        let zone = self.next_zone;
        let mut rejected = 0;
        let mut added = 0;
        for p in (low.z..=high.z).flat_map(|z| {
            (low.y..=high.y).flat_map(move |y| (low.x..=high.x).map(move |x| Pos3::new(x, y, z)))
        }) {
            let at = self.stockpiles.partition_point(|&q| q < p);
            if self.stockpiles.get(at) == Some(&p) {
                continue;
            }
            // A tile holds at most one item, so stacked tiles are not designated.
            if self.world.get(p).is_some_and(|t| standable(*t))
                && !self.is_workshop_tile(p)
                && self.farms.binary_search_by_key(&p, |f| f.pos).is_err()
                && self.count_ground_items(p) < 2
                && self.stockpiles.len() < STOCKPILE_CAPACITY
            {
                let count = self.count_ground_items(p);
                self.stockpiles.insert(at, p);
                self.slot_zone.insert(at, zone);
                self.slot_items.insert(at, count);
                added += 1;
            } else {
                rejected += 1;
            }
        }
        if added > 0 && self.zones.len() < STOCKPILE_CAPACITY {
            self.zones.push(Zone {
                id: zone,
                categories: ALL_CATEGORIES,
                materials: [0; crate::types::ZONE_MATERIALS],
                material_count: 0,
                priority: 1,
                containers: true,
            });
            self.next_zone += 1;
        }
        rejected
    }

    /// Removes the stockpile tiles inside a box and zones left without tiles.
    pub(crate) fn cancel_stockpiles(&mut self, inside: impl Fn(Pos3) -> bool) {
        let mut write = 0;
        for read in 0..self.stockpiles.len() {
            if !inside(self.stockpiles[read]) {
                self.stockpiles[write] = self.stockpiles[read];
                self.slot_zone[write] = self.slot_zone[read];
                self.slot_items[write] = self.slot_items[read];
                write += 1;
            }
        }
        self.stockpiles.truncate(write);
        self.slot_zone.truncate(write);
        self.slot_items.truncate(write);
        let slot_zone = &self.slot_zone;
        self.zones.retain(|z| slot_zone.contains(&z.id));
    }

    /// Applies `ConfigureStockpile`; false for unknown zones or IDs.
    pub(crate) fn configure_stockpile(
        &mut self,
        zone: u32,
        categories: &[String],
        materials: &[String],
        priority: u8,
        containers: bool,
    ) -> bool {
        let Some(i) = self.zone_index(zone) else {
            return false;
        };
        let mut mask = 0u64;
        for id in categories {
            let Some(c) = crate::GameContent::id_of(&self.content.ids.categories, id) else {
                return false;
            };
            mask |= self.content.descendants(c as u8);
        }
        if materials.len() > crate::types::ZONE_MATERIALS {
            return false;
        }
        let mut list = [u16::MAX; crate::types::ZONE_MATERIALS];
        for (slot, id) in list.iter_mut().zip(materials) {
            match crate::GameContent::id_of(&self.content.ids.materials, id) {
                Some(m) => *slot = m,
                None => return false,
            }
        }
        list.sort_unstable();
        let mut count = 0;
        for k in 0..list.len() {
            if list[k] != u16::MAX && (count == 0 || list[count - 1] != list[k]) {
                list[count] = list[k];
                count += 1;
            }
        }
        list[count..].fill(0);
        let z = &mut self.zones[i];
        z.materials = list;
        z.material_count = count as u8;
        z.categories = if categories.is_empty() {
            ALL_CATEGORIES
        } else {
            mask
        };
        z.priority = priority.min(9);
        z.containers = containers;
        true
    }

    /// Releases the haul job of an aborted bulk haul; the lead item may now lie
    /// inside the dropped container.
    pub(crate) fn drop_bulk_job(&mut self, job: JobId) {
        if let Some(JobKind::Haul(lead)) = self.jobs.get(job).map(|j| j.data) {
            if let Some(s) = self.islot(lead) {
                self.icols.haul[s] = None;
            }
            self.jobs.remove(job);
        }
    }
}
