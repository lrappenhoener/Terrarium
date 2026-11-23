//! Contracts shared by headless simulations and their runners.

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SimError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invariant violation: {0}")]
    InvariantViolation(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimMetrics {
    pub active_entities: usize,
    pub custom_gauges: Vec<(String, f64)>,
}

pub trait Simulation: Sized {
    type Config: DeserializeOwned + Default + Clone;
    type Command: Serialize + DeserializeOwned;

    fn new(seed: u64, config: Self::Config) -> Result<Self, SimError>;
    /// Applies commands in order, advances one discrete tick, and increments
    /// [`Self::tick`] by exactly one.
    fn step(&mut self, commands: &[Self::Command]);
    fn tick(&self) -> u64;
    fn is_finished(&self) -> bool;
    fn metrics(&self) -> SimMetrics;
    /// Returns a deterministic hash of all state that can affect the future.
    fn state_hash(&self) -> u64;
    fn check_invariants(&self) -> Result<(), SimError>;
}

/// FNV-1a over bytes, with little-endian encoding for multibyte values.
#[derive(Debug, Clone)]
pub struct StateHasher(u64);

impl StateHasher {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    pub const fn new() -> Self {
        Self(Self::OFFSET)
    }

    pub fn write_u8(&mut self, value: u8) {
        self.0 = (self.0 ^ u64::from(value)).wrapping_mul(Self::PRIME);
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_u128(&mut self, value: u128) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub fn write_f64(&mut self, value: f64) {
        let bits = if value == 0.0 {
            0
        } else if value.is_nan() {
            0x7ff8000000000000
        } else {
            value.to_bits()
        };
        self.write_u64(bits);
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u8(byte);
        }
    }

    pub const fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::StateHasher;

    #[test]
    fn fnv1a_vectors() {
        let mut hash = StateHasher::new();
        assert_eq!(hash.finish(), 0xcbf29ce484222325);
        hash.write_bytes(b"a");
        assert_eq!(hash.finish(), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn canonicalizes_zeros_and_nans() {
        fn hash(value: f64) -> u64 {
            let mut hash = StateHasher::new();
            hash.write_f64(value);
            hash.finish()
        }

        assert_eq!(hash(-0.0), hash(0.0));
        assert_eq!(
            hash(f64::from_bits(0x7ff0000000000001)),
            hash(f64::from_bits(0xfff8000000001234)),
        );
        assert_ne!(hash(f64::NAN), hash(f64::INFINITY));
    }

    #[test]
    fn encodes_primitives_in_little_endian() {
        let mut hash = StateHasher::new();
        hash.write_u8(0);
        hash.write_bool(true);
        hash.write_u32(0x05040302);
        hash.write_u64(0x0d0c0b0a09080706);
        hash.write_u128(0x1d1c1b1a191817161514131211100f0e);
        hash.write_usize(0x1f1e);

        let mut bytes = StateHasher::new();
        bytes.write_bytes(&(0..32).collect::<Vec<_>>());
        bytes.write_bytes(&[0; 6]);
        assert_eq!(hash.finish(), bytes.finish());
    }
}
