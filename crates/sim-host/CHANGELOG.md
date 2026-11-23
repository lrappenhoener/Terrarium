# Changelog

## Unreleased

- Added default subsystem hashes to localize divergence while retaining existing simulation implementations.
- Added optional phase telemetry and an inert probe so runners can measure the same step implementation with and without phase sampling.
- Added default opaque-ID descriptions for traces without runner knowledge of game types.

## 0.1.0

- Added `Simulation`, `SimError`, and `SimMetrics` to keep the runner independent of game types and rules.
- Added `StateHasher` with FNV-1a, fixed little-endian encoding, and canonical floating-point representations so hash checkpoints can be compared across runs and machines.
