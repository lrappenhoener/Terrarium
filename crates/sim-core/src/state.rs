//! Canonical state hashing for simulation values.
//!
//! Encodings are fixed: `u8`, `bool` and fieldless enums use one byte, `u16`, `i16`,
//! `u32` and `i32` are widened to four little-endian bytes, 64-bit values use eight
//! bytes, `usize` is hashed as `u64`. `Option` writes a presence flag before the value.
//! Tuples and arrays hash their elements in order without a length; `Vec` and slices
//! prefix their length. Widening `u16` keeps the encoding of existing reference hashes.

pub use sim_host::StateHasher;

pub trait StateHash {
    fn state_hash(&self, hash: &mut StateHasher);
}

impl StateHash for u8 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u8(*self);
    }
}

impl StateHash for bool {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_bool(*self);
    }
}

impl StateHash for u16 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u32(u32::from(*self));
    }
}

impl StateHash for i16 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u32(i32::from(*self) as u32);
    }
}

impl StateHash for u32 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u32(*self);
    }
}

impl StateHash for i32 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u32(*self as u32);
    }
}

impl StateHash for u64 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u64(*self);
    }
}

impl StateHash for i64 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u64(*self as u64);
    }
}

impl StateHash for usize {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_usize(*self);
    }
}

impl StateHash for u128 {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u128(*self);
    }
}

impl<M> StateHash for crate::entity::TypedId<M> {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_u64(self.to_u64());
    }
}

impl<T: StateHash> StateHash for Option<T> {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_bool(self.is_some());
        if let Some(value) = self {
            value.state_hash(hash);
        }
    }
}

impl<T: StateHash, const N: usize> StateHash for [T; N] {
    fn state_hash(&self, hash: &mut StateHasher) {
        for value in self {
            value.state_hash(hash);
        }
    }
}

impl<T: StateHash> StateHash for [T] {
    fn state_hash(&self, hash: &mut StateHasher) {
        hash.write_usize(self.len());
        for value in self {
            value.state_hash(hash);
        }
    }
}

impl<T: StateHash> StateHash for Vec<T> {
    fn state_hash(&self, hash: &mut StateHasher) {
        self.as_slice().state_hash(hash);
    }
}

impl<T: StateHash + ?Sized> StateHash for &T {
    fn state_hash(&self, hash: &mut StateHasher) {
        (**self).state_hash(hash);
    }
}

macro_rules! tuples {
    ($(($($name:ident),+)),+) => {$(
        impl<$($name: StateHash),+> StateHash for ($($name,)+) {
            #[allow(non_snake_case)]
            fn state_hash(&self, hash: &mut StateHasher) {
                let ($($name,)+) = self;
                $($name.state_hash(hash);)+
            }
        }
    )+};
}

tuples!((A), (A, B), (A, B, C), (A, B, C, D), (A, B, C, D, E));

/// Hashes `Copy` fieldless enums by their `u8` discriminant.
#[macro_export]
macro_rules! state_hash_enum {
    ($($type:ty),+ $(,)?) => {$(
        impl $crate::state::StateHash for $type {
            fn state_hash(&self, hash: &mut $crate::state::StateHasher) {
                hash.write_u8(*self as u8);
            }
        }
    )+};
}

/// Hashes the listed struct fields in the listed order. The exhaustive destructuring
/// fails to compile when a field is missing, so the list stays complete.
#[macro_export]
macro_rules! state_hash_struct {
    ($type:ident { $($field:ident),+ $(,)? }) => {
        impl $crate::state::StateHash for $type {
            fn state_hash(&self, hash: &mut $crate::state::StateHasher) {
                let $type { $($field),+ } = self;
                $($crate::state::StateHash::state_hash($field, hash);)+
            }
        }
    };
}

/// Convenience for a single value.
pub fn hash_value<T: StateHash + ?Sized>(value: &T, hash: &mut StateHasher) {
    value.state_hash(hash);
}
