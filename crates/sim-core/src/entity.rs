use serde::{Deserialize, Serialize};
use sim_host::StateHasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId {
    pub index: u32,
    pub generation: u32,
}

impl EntityId {
    pub const fn to_u64(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    pub const fn from_u64(value: u64) -> Self {
        Self {
            index: value as u32,
            generation: (value >> 32) as u32,
        }
    }
}

struct Slot {
    generation: u32,
    alive: bool,
}

pub struct EntityAllocator {
    slots: Vec<Slot>,
    free: Vec<u32>,
    capacity: u32,
    len: usize,
}

impl EntityAllocator {
    pub fn with_capacity(capacity: u32) -> Self {
        Self {
            slots: Vec::with_capacity(capacity as usize),
            free: Vec::with_capacity(capacity as usize),
            capacity,
            len: 0,
        }
    }

    pub fn create(&mut self) -> Option<EntityId> {
        let index = match self.free.pop() {
            Some(index) => index,
            None if self.slots.len() < self.capacity as usize => {
                let index = self.slots.len() as u32;
                self.slots.push(Slot {
                    generation: 0,
                    alive: false,
                });
                index
            }
            None => return None,
        };
        let slot = &mut self.slots[index as usize];
        slot.alive = true;
        self.len += 1;
        Some(EntityId {
            index,
            generation: slot.generation,
        })
    }

    pub fn destroy(&mut self, id: EntityId) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        let slot = &mut self.slots[id.index as usize];
        slot.alive = false;
        self.len -= 1;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(id.index);
        }
        true
    }

    pub fn is_alive(&self, id: EntityId) -> bool {
        self.slots
            .get(id.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == id.generation)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.alive)
            .map(|(index, slot)| EntityId {
                index: index as u32,
                generation: slot.generation,
            })
    }

    /// Hashes ID history. The caller hashes allocation capacity as configuration.
    pub fn hash_state(&self, hash: &mut StateHasher) {
        hash.write_usize(self.slots.len());
        for slot in &self.slots {
            hash.write_u32(slot.generation);
        }
        hash.write_usize(self.free.len());
        for &index in &self.free {
            hash.write_u32(index);
        }
    }
}

pub struct DenseStorage<T> {
    slots: Vec<Option<(EntityId, T)>>,
}

impl<T> DenseStorage<T> {
    pub fn with_capacity(capacity: u32) -> Self {
        let mut slots = Vec::with_capacity(capacity as usize);
        slots.resize_with(capacity as usize, || None);
        Self { slots }
    }

    pub fn insert(&mut self, id: EntityId, value: T) -> bool {
        let Some(slot) = self.slots.get_mut(id.index as usize) else {
            return false;
        };
        if slot
            .as_ref()
            .is_some_and(|(current, _)| current.generation > id.generation)
        {
            return false;
        }
        *slot = Some((id, value));
        true
    }

    pub fn get(&self, id: EntityId) -> Option<&T> {
        let (current, value) = self.slots.get(id.index as usize)?.as_ref()?;
        (*current == id).then_some(value)
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut T> {
        let (current, value) = self.slots.get_mut(id.index as usize)?.as_mut()?;
        (*current == id).then_some(value)
    }

    pub fn remove(&mut self, id: EntityId) -> Option<T> {
        self.get(id)?;
        self.slots[id.index as usize].take().map(|(_, value)| value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.slots
            .iter()
            .filter_map(|slot| slot.as_ref().map(|(id, value)| (*id, value)))
    }
}

pub struct SparseStorage<T> {
    indices: Vec<usize>,
    dense: Vec<(EntityId, T)>,
}

impl<T> SparseStorage<T> {
    pub fn with_capacity(capacity: u32) -> Self {
        Self {
            indices: vec![usize::MAX; capacity as usize],
            dense: Vec::with_capacity(capacity as usize),
        }
    }

    pub fn insert(&mut self, id: EntityId, value: T) -> bool {
        let Some(index) = self.indices.get_mut(id.index as usize) else {
            return false;
        };
        if let Some((current, slot)) = self.dense.get_mut(*index) {
            if current.generation > id.generation {
                return false;
            }
            (*current, *slot) = (id, value);
        } else {
            *index = self.dense.len();
            self.dense.push((id, value));
        }
        true
    }

    pub fn get(&self, id: EntityId) -> Option<&T> {
        let (current, value) = self.dense.get(*self.indices.get(id.index as usize)?)?;
        (*current == id).then_some(value)
    }

    pub fn remove(&mut self, id: EntityId) -> Option<T> {
        self.get(id)?;
        let index = std::mem::replace(&mut self.indices[id.index as usize], usize::MAX);
        let (_, value) = self.dense.swap_remove(index);
        if let Some((swapped, _)) = self.dense.get(index) {
            self.indices[swapped.index as usize] = index;
        }
        Some(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.dense.iter().map(|(id, value)| (*id, value))
    }
}

pub enum EntityCommand<T> {
    Spawn(T),
    Despawn(EntityId),
}

pub struct EntityCommandBuffer<T> {
    entries: Vec<(EntityId, u32, EntityCommand<T>)>,
}

impl<T> EntityCommandBuffer<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// The caller assigns a unique ordinal per source, independent of collection order.
    pub fn push(&mut self, source: EntityId, ordinal: u32, command: EntityCommand<T>) {
        self.entries.push((source, ordinal, command));
    }

    pub fn apply(&mut self, mut apply: impl FnMut(EntityCommand<T>)) {
        self.entries
            .sort_unstable_by_key(|(source, ordinal, _)| (*source, *ordinal));
        for (_, _, command) in self.entries.drain(..) {
            apply(command);
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct AllocatorState {
    generations: Vec<u32>,
    free: Vec<u32>,
}

/// Maps stable generational IDs to dense, transient component slots.
pub struct EntityTable {
    allocator: EntityAllocator,
    by_index: Vec<u32>,
    by_slot: Vec<EntityId>,
}

impl EntityTable {
    pub fn with_capacity(capacity: u32) -> Self {
        Self {
            allocator: EntityAllocator::with_capacity(capacity),
            by_index: vec![u32::MAX; capacity as usize],
            by_slot: Vec::with_capacity(capacity as usize),
        }
    }

    pub fn create(&mut self) -> Option<EntityId> {
        let id = self.allocator.create()?;
        self.by_index[id.index as usize] = self.by_slot.len() as u32;
        self.by_slot.push(id);
        Some(id)
    }

    /// Removes the ID and returns the slot to swap-remove from component columns.
    pub fn destroy(&mut self, id: EntityId) -> Option<u32> {
        let slot = self.slot(id)?;
        self.allocator.destroy(id);
        self.by_slot.swap_remove(slot as usize);
        self.by_index[id.index as usize] = u32::MAX;
        if let Some(&swapped) = self.by_slot.get(slot as usize) {
            self.by_index[swapped.index as usize] = slot;
        }
        Some(slot)
    }

    pub fn slot(&self, id: EntityId) -> Option<u32> {
        let &slot = self.by_index.get(id.index as usize)?;
        (self.by_slot.get(slot as usize) == Some(&id)).then_some(slot)
    }

    pub fn id(&self, slot: u32) -> Option<EntityId> {
        self.by_slot.get(slot as usize).copied()
    }

    pub fn is_alive(&self, id: EntityId) -> bool {
        self.slot(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.by_slot.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_slot.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.by_slot.iter().copied()
    }

    pub fn iter_by_id(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.allocator.iter()
    }

    pub fn hash_state(&self, hash: &mut StateHasher) {
        self.allocator.hash_state(hash);
    }

    pub fn snapshot_state(&self) -> AllocatorState {
        AllocatorState {
            generations: self
                .allocator
                .slots
                .iter()
                .map(|slot| slot.generation)
                .collect(),
            free: self.allocator.free.clone(),
        }
    }

    /// Restores generational history and derives dense slots from live IDs.
    pub fn restore_state(
        capacity: u32,
        state: AllocatorState,
        live: impl IntoIterator<Item = EntityId>,
    ) -> Option<Self> {
        if state.generations.len() > capacity as usize || state.free.len() > state.generations.len()
        {
            return None;
        }
        let mut table = Self::with_capacity(capacity);
        let mut free = vec![false; state.generations.len()];
        for index in state.free {
            let available = free.get_mut(index as usize)?;
            if std::mem::replace(available, true) || state.generations[index as usize] == 0 {
                return None;
            }
            table.allocator.free.push(index);
        }
        table
            .allocator
            .slots
            .extend(state.generations.into_iter().map(|generation| Slot {
                generation,
                alive: false,
            }));
        for id in live {
            let index = id.index as usize;
            let slot = table.allocator.slots.get_mut(index)?;
            if slot.alive || slot.generation != id.generation || free[index] {
                return None;
            }
            slot.alive = true;
            table.by_index[index] = table.by_slot.len() as u32;
            table.by_slot.push(id);
        }
        if table
            .allocator
            .slots
            .iter()
            .zip(free)
            .any(|(slot, free)| !slot.alive && !free && slot.generation != u32::MAX)
        {
            return None;
        }
        table.allocator.len = table.by_slot.len();
        Some(table)
    }

    /// Reorders slots using a validated destination-to-source permutation.
    pub fn apply_permutation(&mut self, permutation: &SlotPermutation) -> bool {
        if !permutation.apply(&mut self.by_slot) {
            return false;
        }
        for (slot, id) in self.by_slot.iter().enumerate() {
            self.by_index[id.index as usize] = slot as u32;
        }
        true
    }
}

/// Preallocated swaps for a bijection from destination slots to source slots.
pub struct SlotPermutation {
    destinations: Vec<usize>,
    swaps: Vec<(usize, usize)>,
    valid: bool,
}

impl SlotPermutation {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            destinations: Vec::with_capacity(capacity),
            swaps: Vec::with_capacity(capacity),
            valid: false,
        }
    }

    /// Compiles and validates a permutation once so it can be applied to every column.
    pub fn prepare(&mut self, sources: &[usize]) -> bool {
        self.valid = false;
        self.destinations.clear();
        self.destinations.resize(sources.len(), usize::MAX);
        self.swaps.clear();
        for (destination, &source) in sources.iter().enumerate() {
            let Some(slot) = self.destinations.get_mut(source) else {
                return false;
            };
            if *slot != usize::MAX {
                return false;
            }
            *slot = destination;
        }
        for slot in 0..sources.len() {
            while self.destinations[slot] != slot {
                let other = self.destinations[slot];
                self.destinations.swap(slot, other);
                self.swaps.push((slot, other));
            }
        }
        self.valid = true;
        true
    }

    pub fn matches(&self, len: usize) -> bool {
        self.valid && self.destinations.len() == len
    }

    pub fn apply<T>(&self, values: &mut [T]) -> bool {
        if !self.matches(values.len()) {
            return false;
        }
        for &(a, b) in &self.swaps {
            values.swap(a, b);
        }
        true
    }
}

/// Declares dense component columns that share the same lifecycle and permutation.
#[macro_export]
macro_rules! entity_columns {
    ($vis:vis struct $name:ident { $($field:ident : $type:ty),+ $(,)? }) => {
        $vis struct $name { $( $field: Vec<$type>, )+ }

        #[allow(dead_code)] // Groups expose the same operations regardless of which callers need them.
        impl $name {
            $vis fn with_capacity(capacity: usize) -> Self {
                Self { $($field: Vec::with_capacity(capacity),)+ }
            }

            $vis fn push(&mut self, $($field: $type),+) {
                $(self.$field.push($field);)+
            }

            $vis fn swap_remove(&mut self, slot: u32) -> Option<($($type,)+)> {
                let slot = slot as usize;
                if slot >= self.len() { return None; }
                Some(($(self.$field.swap_remove(slot),)+))
            }

            $vis fn len(&self) -> usize {
                let lengths = [$(self.$field.len()),+];
                debug_assert!(lengths.iter().all(|len| *len == lengths[0]));
                lengths[0]
            }

            $vis fn is_empty(&self) -> bool { self.len() == 0 }

            $vis fn apply_permutation(&mut self, permutation: &$crate::entity::SlotPermutation) -> bool {
                if !permutation.matches(self.len()) { return false; }
                $(permutation.apply(&mut self.$field);)+
                true
            }
        }
    };
}
