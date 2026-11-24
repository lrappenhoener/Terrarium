# Changelog

## Unreleased

- Snapshot payload version 2 starts with the tag `FORTSNP2` and the world generator fingerprint (manual `GENERATOR_VERSION` plus a hash of all generator inputs and constants). Loading checks it before regenerating the world and fails with "Weltgenerator geändert: Version X → Y". Version 1 payloads load with a warning.
- `initial_food_per_resident` adds starting rations per initial resident (default 0).
- Spawn spots must be reachable from the embark tile, and drops on full stockpiles search further. Both only affect configurations where the old behavior broke invariants or stranded residents; golden hashes are unchanged.
- Loose items are kept in a per-chunk category index for food and stone queries, with results identical to the former scans.
- Economy (Etappe 6): content files in `content/base` (categories, materials, items, workshops, recipes, plants, skills), mods via `mods`; items with stacks, quality, wear and containers; trees, farms and a calendar; workshops, recipes and work orders; stockpile zones with filters and priorities, demand-driven capped transport and bulk hauling. Generator version 3 (materials indexed by sorted ID), snapshot payload v4 (`FORTSNP4`) with the content fingerprint; v1–v3 snapshots are rejected ("Weltgenerator geändert: Version 2 → 3"). New golden hashes and golden snapshot `fortress-v4-small.bin`.
