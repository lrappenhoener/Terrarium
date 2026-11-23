# Changelog

## Unreleased

- Versioned the logical state hash as `jungle-logical-v1`. The hash follows stable IDs and logical rows, serializes sparse components in ID order, and excludes physical slot order and redundant allocator occupancy. The hash format changes once to establish layout-independent reference hashes. Simulation rules and state transitions are preserved.

- Added snapshot payload version 1 using Postcard and the logical ID and tile order. Effective configuration uses its complete serialized JSON representation inside the payload. Occupancy, water distances, slot layout and scratch storage are rebuilt. The ChaCha stream is restored from seed, stream and word position.
