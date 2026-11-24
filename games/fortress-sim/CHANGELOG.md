# Changelog

## Unreleased

- Snapshot payload version 2 starts with the tag `FORTSNP2` and the world generator fingerprint (manual `GENERATOR_VERSION` plus a hash of all generator inputs and constants). Loading checks it before regenerating the world and fails with "Weltgenerator geändert: Version X → Y". Version 1 payloads load with a warning.
- `initial_food_per_resident` adds starting rations per initial resident (default 0).
- Spawn spots must be reachable from the embark tile, and drops on full stockpiles search further. Both only affect configurations where the old behavior broke invariants or stranded residents; golden hashes are unchanged.
- Loose items are kept in a per-chunk category index for food and stone queries, with results identical to the former scans.
