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
