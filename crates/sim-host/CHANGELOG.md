# Changelog

## Unreleased
- Documented the `key<TAB>value` line format of `debug_dump` with unique stable keys, so runners can compare dumps by key and report one inserted entity instead of every following line. Dumps without tabs remain valid and compare as whole lines.
- Added opt-in snapshot version, save/load, content fingerprint and logical dump hooks with defaults for existing games. `SimError` is now non-exhaustive so future diagnostic variants can be added. External exhaustive matches need a fallback. The crate exposes its version for archive headers.

- Added default scaling dimension hooks so phase costs can be normalized by measured workload without runner knowledge of the simulation.

- Added default subsystem hashes to localize divergence while retaining existing simulation implementations.
- Added optional phase telemetry and an inert probe so runners can measure the same step implementation with and without phase sampling.
- Added default opaque-ID descriptions for traces without runner knowledge of game types.

## 0.1.0

- Added `Simulation`, `SimError`, and `SimMetrics` to keep the runner independent of game types and rules.
- Added `StateHasher` with FNV-1a, fixed little-endian encoding, and canonical floating-point representations so hash checkpoints can be compared across runs and machines.
