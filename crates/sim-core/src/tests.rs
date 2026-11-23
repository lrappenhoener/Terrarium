use crate::{
    behavior::select_best,
    entity::{
        DenseStorage, EntityAllocator, EntityCommand, EntityCommandBuffer, EntityId, SparseStorage,
    },
    intent::IntentBuffer,
    rng::{SystemRng, below, stateless_u64},
    schedule::{EventQueue, staggered},
    space::{Grid2, Point, SpatialHit, SpatialIndex},
};

fn id(index: u32) -> EntityId {
    EntityId {
        index,
        generation: 0,
    }
}

#[test]
fn recycled_slots_invalidate_previous_generations() {
    let mut allocator = EntityAllocator::with_capacity(2);
    let first = allocator.create().unwrap();
    assert!(allocator.destroy(first));
    let recycled = allocator.create().unwrap();
    assert_eq!(first.index, recycled.index);
    assert_ne!(first.generation, recycled.generation);
    assert!(!allocator.is_alive(first));
    assert!(!allocator.destroy(first));
    assert!(allocator.is_alive(recycled));
    assert_eq!(EntityId::from_u64(recycled.to_u64()), recycled);
}

#[test]
fn component_storage_updates_and_removes_matching_generations() {
    let mut dense = DenseStorage::with_capacity(3);
    let mut sparse = SparseStorage::with_capacity(3);
    for (id, value) in [(id(2), 20), (id(0), 10), (id(1), 30)] {
        assert!(dense.insert(id, value));
        assert!(sparse.insert(id, value));
    }
    assert_eq!(
        dense.iter().map(|(id, _)| id.index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(
        sparse.iter().map(|(id, _)| id.index).collect::<Vec<_>>(),
        [2, 0, 1]
    );
    assert_eq!(dense.remove(id(2)), Some(20));
    assert_eq!(sparse.remove(id(2)), Some(20));
    assert_eq!(sparse.get(id(1)), Some(&30));
    assert_eq!(dense.get(id(2)), None);
    let newer = EntityId {
        generation: 1,
        ..id(2)
    };
    assert!(dense.insert(newer, 5) && sparse.insert(newer, 5));
    assert_eq!(dense.get(id(2)), None);
    assert_eq!(sparse.remove(id(2)), None);
}

#[test]
fn grid_is_row_major() {
    let mut grid = Grid2::new(3, 2, 0).unwrap();
    assert!(grid.set(Point { x: 1, y: 1 }, 5));
    assert!(!grid.set(Point { x: -1, y: 0 }, 3));
    assert_eq!(grid.as_slice(), [0, 0, 0, 0, 5, 0]);
}

#[test]
fn spatial_queries_match_brute_force() {
    let mut index = SpatialIndex::new(71, 53, 8, 3, 500).unwrap();
    for seed in [1, 2, 42] {
        let mut rng = SystemRng::new(seed, 7);
        let mut entries = Vec::new();
        for i in (0..500).rev() {
            let point = Point {
                x: below(rng.next_u64(), 71) as i32,
                y: below(rng.next_u64(), 53) as i32,
            };
            entries.push((SpatialHit { id: id(i), slot: i }, point, (i % 3) as u8));
        }
        index.rebuild(entries.iter().copied());
        let mut hits = Vec::new();
        for radius in [0u32, 1, 8, 12, 75] {
            for (_, point, _) in entries.iter().step_by(9) {
                for mask in [0, 1, 2, 4, 5, 7] {
                    index.query_radius(*point, radius, mask, &mut hits);
                    let mut expected: Vec<_> = entries
                        .iter()
                        .filter(|(_, p, category)| {
                            mask & (1 << category) != 0
                                && p.distance_squared(*point) <= u128::from(radius).pow(2)
                        })
                        .map(|(id, _, _)| *id)
                        .collect();
                    expected.sort_unstable();
                    assert_eq!(hits, expected);
                }
            }
        }
    }
}

#[test]
fn event_queue_breaks_ties_using_insert_order() {
    let mut queue = EventQueue::with_capacity(3);
    queue.push(9, "later");
    queue.push(4, "first");
    queue.push(4, "second");
    assert_eq!(queue.pop_due(3), None);
    assert_eq!(queue.pop_due(4), Some("first"));
    assert_eq!(queue.pop_due(9), Some("second"));
    assert_eq!(queue.pop_due(9), Some("later"));
    assert!(staggered(u64::MAX, 2, 1));
    assert!(!staggered(0, 0, 0));
}

#[test]
fn stateless_rng_vectors_and_range() {
    assert_eq!(stateless_u64(0, 0, 0, 0), 0x1957a7604e215178);
    assert_eq!(stateless_u64(42, 3, 0x100000020, 9), 0x5fcd8a11e2dfa31f);
    for n in [1, 17, u64::MAX] {
        assert_eq!(below(0, n), 0);
        assert!(below(u64::MAX, n) < n);
    }
}

#[test]
fn resolution_ignores_collection_order() {
    let resolve = |entries: &[(u32, u32, i32)]| {
        let mut buffer = IntentBuffer::with_capacity(entries.len());
        for &(key, index, priority) in entries {
            buffer.push(key, id(index), priority);
        }
        let mut result = Vec::new();
        buffer.resolve(
            |&priority| priority,
            |winner, losers| {
                result.push((
                    winner.0,
                    winner.1.index,
                    losers.iter().map(|loser| loser.1.index).collect::<Vec<_>>(),
                ));
            },
        );
        result
    };
    let a = [(1, 3, -2), (0, 9, 0), (1, 2, -2), (1, 1, 0)];
    let b = [a[3], a[1], a[0], a[2]];
    assert_eq!(resolve(&a), [(0, 9, vec![]), (1, 2, vec![3, 1])]);
    assert_eq!(resolve(&a), resolve(&b));
    assert_eq!(select_best(&[(4, 8), (2, 8), (1, 7)]), Some(2));
}

#[test]
fn command_buffer_ignores_collection_order() {
    let apply = |entries: &[(u32, u32, &'static str)]| {
        let mut buffer = EntityCommandBuffer::with_capacity(entries.len());
        for &(source, ordinal, value) in entries {
            buffer.push(id(source), ordinal, EntityCommand::Spawn(value));
        }
        let mut result = Vec::new();
        buffer.apply(|command| match command {
            EntityCommand::Spawn(value) => result.push(value),
            EntityCommand::Despawn(_) => unreachable!(),
        });
        result
    };
    assert_eq!(
        apply(&[(1, 1, "c"), (0, 0, "a"), (1, 0, "b")]),
        ["a", "b", "c"]
    );
    assert_eq!(
        apply(&[(1, 0, "b"), (0, 0, "a"), (1, 1, "c")]),
        ["a", "b", "c"]
    );
}

#[test]
fn dense_slots_and_columns_survive_lifecycle_and_permutations() {
    use crate::entity::{EntityTable, SlotPermutation};
    crate::entity_columns! {
        struct Columns, row ColumnsRow { id_bits: u64, inverse: u64 }
    }

    for seed in [1, 2, 42] {
        let mut rng = SystemRng::new(seed, 14);
        let mut original = EntityAllocator::with_capacity(48);
        let mut table = EntityTable::with_capacity(48);
        let mut columns = Columns::with_capacity(48);
        let mut permutation = SlotPermutation::with_capacity(48);
        for _ in 0..1000 {
            match below(rng.next_u64(), 3) {
                0 => {
                    let id = table.create();
                    assert_eq!(id, original.create());
                    if let Some(id) = id {
                        columns.push(id.to_u64(), !id.to_u64());
                    }
                }
                1 if !table.is_empty() => {
                    let slot = below(rng.next_u64(), table.len() as u64) as u32;
                    let id = table.id(slot).unwrap();
                    assert_eq!(table.destroy(id), Some(slot));
                    assert!(original.destroy(id));
                    assert_eq!(columns.swap_remove(slot), Some((id.to_u64(), !id.to_u64())));
                    assert!(!table.is_alive(id));
                }
                _ => {
                    let mut sources: Vec<_> = (0..table.len()).collect();
                    for end in (1..sources.len()).rev() {
                        sources.swap(end, below(rng.next_u64(), end as u64 + 1) as usize);
                    }
                    assert!(permutation.prepare(&sources));
                    assert!(table.apply_permutation(&permutation));
                    assert!(columns.apply_permutation(&permutation));
                }
            }
            assert_eq!(table.len(), columns.len());
            assert_eq!(
                table.iter_by_id().collect::<Vec<_>>(),
                original.iter().collect::<Vec<_>>()
            );
            for (slot, id) in table.iter().enumerate() {
                assert_eq!(table.slot(id), Some(slot as u32));
                assert_eq!(table.id(slot as u32), Some(id));
                assert_eq!(columns.id_bits[slot], id.to_u64());
                assert_eq!(columns.inverse[slot], !id.to_u64());
            }
        }
        assert!(!permutation.prepare(&[0, 0]));
    }
}

#[test]
fn chunked_grid_retains_logical_indices_and_excludes_padding() {
    use crate::space::ChunkedGrid2;
    let (width, height) = (79, 44);
    let mut flat = Grid2::new(width, height, 0).unwrap();
    let mut chunked = ChunkedGrid2::new(width, height, 0).unwrap();
    let mut rng = SystemRng::new(42, 9);
    for value in 1..1500 {
        let point = Point {
            x: below(rng.next_u64(), (width + 2) as u64) as i32 - 1,
            y: below(rng.next_u64(), (height + 2) as u64) as i32 - 1,
        };
        assert_eq!(flat.set(point, value), chunked.set(point, value));
        assert_eq!(flat.get(point), chunked.get(point));
    }
    chunked.for_each_block_row_mut(|first, row| {
        for (offset, value) in row.iter_mut().enumerate() {
            if (first + offset).is_multiple_of(7) {
                *value += 1;
            }
        }
    });
    for tile in flat.as_mut_slice().iter_mut().step_by(7) {
        *tile += 1;
    }
    assert!(chunked.iter().eq(flat.as_slice()));
    for (index, value) in flat.as_slice().iter().enumerate() {
        assert_eq!(chunked.get_index(index), Some(value));
    }
}

#[test]
fn entity_table_rejects_an_old_generation_after_reuse() {
    let mut table = crate::entity::EntityTable::with_capacity(1);
    let old = table.create().unwrap();
    assert_eq!(table.destroy(old), Some(0));
    let new = table.create().unwrap();
    assert_eq!(new.index, old.index);
    assert_ne!(old.generation, new.generation);
    assert_eq!(table.slot(old), None);
    assert_eq!(table.slot(new), Some(0));
}

#[test]
fn generated_rows_hash_dump_and_roundtrip_in_declaration_order() {
    use crate::state::{StateHash, StateHasher};
    crate::entity_columns! {
        struct Pair, row PairRow { left: u16, right: Option<EntityId> }
    }
    let mut columns = Pair::with_capacity(2);
    columns.push(7, None);
    columns.push_row(PairRow {
        left: 3,
        right: Some(id(4)),
    });
    let mut generated = StateHasher::new();
    assert!(columns.hash_row(1, &mut generated));
    assert!(!columns.hash_row(2, &mut generated));
    let mut manual = StateHasher::new();
    manual.write_u32(3);
    manual.write_bool(true);
    manual.write_u64(id(4).to_u64());
    assert_eq!(generated.finish(), manual.finish());
    let mut dump = String::new();
    assert!(columns.dump_row(0, &mut dump));
    assert_eq!(dump, "left=7 right=None");
    let row = columns.row(1).unwrap();
    assert_eq!((row.left, row.right), (3, Some(id(4))));
    let mut tuple = StateHasher::new();
    (3u16, Some(id(4))).state_hash(&mut tuple);
    assert_eq!(tuple.finish(), manual.finish());
}

#[test]
fn sparse_storage_hashes_snapshots_and_dumps_in_id_order() {
    use crate::state::StateHasher;
    let mut a = SparseStorage::with_capacity(8);
    let mut b = SparseStorage::with_capacity(8);
    for index in [5, 1, 3] {
        a.insert(id(index), u64::from(index) * 10);
    }
    for index in [3, 5, 1, 7] {
        b.insert(id(index), u64::from(index) * 10);
    }
    b.remove(id(7));
    let hash = |storage: &SparseStorage<u64>| {
        let mut hash = StateHasher::new();
        storage.hash_state(&mut hash);
        hash.finish()
    };
    assert_eq!(hash(&a), hash(&b));
    let entries = a.snapshot_entries();
    assert_eq!(
        entries.iter().map(|(id, _)| id.index).collect::<Vec<_>>(),
        [1, 3, 5]
    );
    let restored = SparseStorage::restore(8, entries).unwrap();
    assert_eq!(hash(&restored), hash(&a));
    assert!(SparseStorage::restore(8, vec![(id(2), 0u64), (id(1), 0)]).is_none());
    let mut dump = String::new();
    restored.dump("timer", &mut dump);
    assert_eq!(dump, "timer:1:0\t10\ntimer:3:0\t30\ntimer:5:0\t50\n");
}
