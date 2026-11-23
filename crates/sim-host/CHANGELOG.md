# Changelog

## 0.1.0

- Added `Simulation`, `SimError`, and `SimMetrics` to keep the runner independent of game types and rules.
- Added `StateHasher` with FNV-1a, fixed little-endian encoding, and canonical floating-point representations so hash checkpoints can be compared across runs and machines.
