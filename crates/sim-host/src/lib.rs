//! Contracts shared by headless simulations and their runners.

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SimError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invariant violation: {0}")]
    InvariantViolation(String),
    #[error("unsupported feature: {0}")]
    Unsupported(&'static str),
    #[error("snapshot/container error: {0}")]
    Snapshot(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimMetrics {
    pub active_entities: usize,
    pub custom_gauges: Vec<(String, f64)>,
}

/// Receives timing boundaries. Simulation behavior must be independent of the probe.
pub trait PhaseProbe {
    fn enter(&mut self, phase: &'static str);
    fn exit(&mut self, phase: &'static str);
}

pub struct NoopProbe;

impl PhaseProbe for NoopProbe {
    fn enter(&mut self, _: &'static str) {}
    fn exit(&mut self, _: &'static str) {}
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

    /// Version of the game's snapshot payload. Zero indicates unsupported snapshots.
    fn snapshot_version() -> u32 {
        0
    }

    /// Oldest payload version `load_snapshot` still accepts. Defaults to the current one.
    fn min_snapshot_version() -> u32 {
        Self::snapshot_version()
    }

    /// Non-fatal notes from loading, such as an accepted legacy payload.
    fn load_warnings(&self) -> Vec<String> {
        Vec::new()
    }

    /// Writes the complete logical state, excluding rebuildable derived values.
    fn save_snapshot(&self, _out: &mut Vec<u8>) -> Result<(), SimError> {
        Err(SimError::Unsupported("snapshots"))
    }

    /// Restores the complete logical state and rebuilds its derived values.
    fn load_snapshot(_bytes: &[u8]) -> Result<Self, SimError> {
        Err(SimError::Unsupported("snapshots"))
    }

    /// Fingerprint of effective settings and versioned game content.
    fn content_fingerprint(&self) -> u64 {
        0
    }

    /// Writes a line-oriented logical dump with a layout-independent order.
    ///
    /// Each line has the form `key<TAB>value` with a unique, stable key such as
    /// `entity:17:3` or `tile:12,40,5`. Runners compare dumps by key and report
    /// entries only in one dump and differing values. Lines without a tab are
    /// compared as whole lines.
    fn debug_dump(&self, _out: &mut String) -> Result<(), SimError> {
        Err(SimError::Unsupported("debug_dump"))
    }

    /// Current sizes of the opaque dimensions used for performance normalization.
    fn scale_units(&self) -> Vec<(&'static str, u64)> {
        vec![("entities", self.metrics().active_entities as u64)]
    }

    /// Dimension per phase. The runner defaults unlisted phases to `entities`.
    fn phase_dimensions(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// Diagnostic hashes for localizing differences between runs.
    fn subsystem_hashes(&self) -> Vec<(&'static str, u64)> {
        vec![("all", self.state_hash())]
    }

    /// Equivalent to step, with optional phase boundary notifications.
    fn step_profiled(&mut self, commands: &[Self::Command], _probe: &mut dyn PhaseProbe) {
        self.step(commands);
    }

    /// Describes an opaque identifier for tracing, if supported.
    fn describe_entity(&self, _id: u64) -> Option<Vec<(String, String)>> {
        None
    }
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
