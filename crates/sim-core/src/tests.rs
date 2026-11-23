use crate::{
    behavior::select_best,
    entity::{
        DenseStorage, EntityAllocator, EntityCommand, EntityCommandBuffer, EntityId, SparseStorage,
    },
    intent::IntentBuffer,
    rng::{SystemRng, below, stateless_u64},
    schedule::{EventQueue, staggered},
    space::{Grid2, Point, SpatialIndex},
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
    let mut index = SpatialIndex::new(71, 53, 8, 500).unwrap();
    for seed in [1, 2, 42] {
        let mut rng = SystemRng::new(seed, 7);
        let mut entries = Vec::new();
        for i in (0..500).rev() {
            let point = Point {
                x: below(rng.next_u64(), 71) as i32,
                y: below(rng.next_u64(), 53) as i32,
            };
            entries.push((id(i), point));
        }
        index.rebuild(entries.iter().copied());
        let mut hits = Vec::new();
        for radius in [0u32, 1, 8, 12, 75] {
            for (_, point) in entries.iter().step_by(9) {
                index.query_radius(*point, radius, &mut hits);
                let mut expected: Vec<_> = entries
                    .iter()
                    .filter(|(_, p)| p.distance_squared(*point) <= u128::from(radius).pow(2))
                    .map(|(id, _)| *id)
                    .collect();
                expected.sort_unstable();
                assert_eq!(hits, expected);
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
