use super::*;
use decision::Action;

fn mini() -> JungleSim {
    let mut sim = JungleSim::new(
        19,
        JungleConfig {
            world_size: 16,
            initial_herbivores: 0,
            initial_predators: 0,
            max_animals: 40,
            think_interval: 1,
            plant_update_stripes: 3,
            ..JungleConfig::default()
        },
    )
    .unwrap();
    sim.world.for_each_block_row_mut(|_, row| {
        row.fill(Tile {
            terrain: Terrain::Grass,
            biomass: 500,
            water_distance: 3,
        })
    });
    sim.walkable = sim.world.len();
    sim
}

fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

#[test]
fn same_seed_same_hash_sequence_and_other_seed_different_animals() {
    let config = JungleConfig {
        world_size: 48,
        initial_herbivores: 200,
        initial_predators: 20,
        max_animals: 500,
        ..JungleConfig::default()
    };
    let mut a = JungleSim::new(42, config.clone()).unwrap();
    let mut b = JungleSim::new(42, config.clone()).unwrap();
    let c = JungleSim::new(17, config).unwrap();
    assert_ne!(a.hashes()[1], c.hashes()[1]);
    for tick in 0..500 {
        a.step(&[]);
        if tick < 300 {
            b.step(&[]);
            assert_eq!(a.state_hash(), b.state_hash());
        }
        a.check_invariants().unwrap();
    }
}

#[test]
fn a_destination_can_only_have_one_winner() {
    let mut sim = mini();
    let target = point(6, 5);
    assert!(sim.spawn_at(Species::Herbivore, point(5, 5), false));
    assert!(sim.spawn_at(Species::Herbivore, point(7, 5), false));
    sim.decisions.extend(sim.allocator.iter().map(|id| {
        (
            id,
            Decision {
                action: Action::Roam,
                target: None,
                herd: None,
                intent: Intent::Move(target),
            },
        )
    }));
    sim.resolve();
    sim.apply_effects();
    assert_eq!(
        sim.columns
            .position
            .iter()
            .filter(|p| **p == target)
            .count(),
        1
    );
    assert_eq!(sim.occupied.get(target).unwrap().unwrap().index, 0);
}

#[test]
fn the_hungriest_hunter_wins_and_ties_use_the_smaller_id() {
    for (first, second, winner) in [(90, 100, 1), (100, 100, 0)] {
        let mut sim = mini();
        assert!(sim.spawn_at(Species::Predator, point(5, 5), false));
        assert!(sim.spawn_at(Species::Predator, point(7, 5), false));
        assert!(sim.spawn_at(Species::Herbivore, point(6, 5), false));
        let ids: Vec<_> = sim.allocator.iter().collect();
        for (i, hunger) in [first, second].into_iter().enumerate() {
            sim.columns.vitals[sim.allocator.slot(ids[i]).unwrap() as usize].hunger = hunger;
            sim.decisions.push((
                ids[i],
                Decision {
                    action: Action::Hunt,
                    target: Some(ids[2]),
                    herd: None,
                    intent: Intent::Attack(ids[2], hunger),
                },
            ));
        }
        sim.resolve();
        sim.apply_effects();
        sim.lifecycle();
        assert_eq!(sim.deaths[0], 1);
        assert_eq!(sim.vitals(ids[winner]).unwrap().hunger, 0);
        assert_ne!(sim.vitals(ids[1 - winner]).unwrap().hunger, 0);
    }
}

#[test]
fn a_stale_target_is_replaced_after_its_slot_is_reused() {
    let mut sim = mini();
    assert!(sim.spawn_at(Species::Predator, point(5, 5), false));
    assert!(sim.spawn_at(Species::Herbivore, point(6, 5), false));
    let ids: Vec<_> = sim.allocator.iter().collect();
    sim.hunting.insert(ids[0], ids[1]);
    sim.despawn(ids[1]);
    assert!(sim.spawn_at(Species::Herbivore, point(5, 6), false));
    sim.columns.vitals[sim.allocator.slot(ids[0]).unwrap() as usize].hunger = 40;
    let replacement = sim.allocator.iter().nth(1).unwrap();
    assert_eq!(ids[1].index, replacement.index);
    assert!(!sim.allocator.is_alive(ids[1]));
    sim.step(&[]);
    assert_eq!(sim.hunting.get(ids[0]), Some(&replacement));
    assert_eq!(sim.deaths[0], 1);
    assert!(sim.describe_entity(ids[1].to_u64()).is_none());
}

#[test]
fn commands_spawn_adjust_growth_and_schedule_end_of_drought() {
    let mut sim = mini();
    sim.step(&[
        JungleCommand::SpawnAnimals {
            species: Species::Herbivore,
            count: 3,
        },
        JungleCommand::SetPlantGrowth { per_mille: 17 },
        JungleCommand::Drought { duration_ticks: 2 },
    ]);
    assert_eq!(sim.population, [3, 0]);
    assert_eq!(sim.growth, 17);
    assert_eq!(sim.world.get_index(0).unwrap().biomass, 488);
    assert_eq!(sim.drought_until, Some(2));
    sim.step(&[]);
    sim.step(&[]);
    assert_eq!(sim.drought_until, None);
    assert_eq!(sim.world.get_index(2).unwrap().biomass, 517);
    sim.check_invariants().unwrap();
}

#[test]
fn default_populations_survive_and_fluctuate() {
    for seed in [1, 2, 42] {
        let mut sim = JungleSim::new(seed, JungleConfig::default()).unwrap();
        let initial = sim.population;
        let mut min = initial;
        let mut max = initial;
        for tick in 1..=10_000 {
            sim.step(&[]);
            if tick % 1000 == 0 {
                eprintln!("seed {seed} tick {tick}: {:?}", sim.population);
                assert!(
                    sim.population.iter().all(|&n| n > 0),
                    "seed {seed}, tick {tick}"
                );
                for i in 0..2 {
                    min[i] = min[i].min(sim.population[i]);
                    max[i] = max[i].max(sim.population[i]);
                }
            }
        }
        sim.check_invariants().unwrap();
        for i in 0..2 {
            assert!(
                max[i] - min[i] >= initial[i] / 20,
                "seed {seed} species {i}"
            );
        }
        assert!(sim.births > 0 && sim.deaths[0] > 0);
    }
}

#[test]
fn golden_hash_at_tick_1000() {
    let mut sim = JungleSim::new(42, JungleConfig::default()).unwrap();
    for _ in 0..1000 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x5317cfd6e2e12024);
}

#[test]
fn shuffling_slots_preserves_each_of_a_thousand_logical_states() {
    let config = JungleConfig {
        world_size: 64,
        initial_herbivores: 120,
        initial_predators: 8,
        max_animals: 600,
        reorder_interval: 0,
        ..JungleConfig::default()
    };
    let mut ordered = JungleSim::new(42, config.clone()).unwrap();
    let mut shuffled = JungleSim::new(
        42,
        JungleConfig {
            debug_shuffle_slots: true,
            ..config
        },
    )
    .unwrap();
    for tick in 0..1000 {
        let commands = [JungleCommand::SpawnAnimals {
            species: Species::Herbivore,
            count: 20,
        }];
        let commands = if tick % 125 == 0 { &commands[..] } else { &[] };
        ordered.step(commands);
        shuffled.step(commands);
        assert_eq!(
            ordered.state_hash(),
            shuffled.state_hash(),
            "tick {}",
            tick + 1
        );
    }
    assert!(ordered.births > 0);
}

#[test]
fn disabling_spatial_reorder_preserves_reference_hash() {
    let mut sim = JungleSim::new(
        42,
        JungleConfig {
            reorder_interval: 0,
            ..JungleConfig::default()
        },
    )
    .unwrap();
    for _ in 0..1000 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x5317cfd6e2e12024);
}

fn snapshot_config() -> JungleConfig {
    JungleConfig {
        world_size: 128,
        initial_herbivores: 500,
        initial_predators: 25,
        max_animals: 2_000,
        ..JungleConfig::default()
    }
}

fn roundtrip(sim: &JungleSim) -> JungleSim {
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes).unwrap();
    let loaded = JungleSim::load_snapshot(&bytes).unwrap();
    assert_eq!(loaded.state_hash(), sim.state_hash());
    assert_eq!(loaded.subsystem_hashes(), sim.subsystem_hashes());
    assert_eq!(loaded.metrics().custom_gauges, sim.metrics().custom_gauges);
    loaded
}

#[test]
fn snapshot_preserves_each_tick_across_reorder_boundaries() {
    let mut continuous = JungleSim::new(42, snapshot_config()).unwrap();
    let mut interrupted = roundtrip(&continuous);
    for tick in 1..=1000 {
        continuous.step(&[]);
        interrupted.step(&[]);
        if [1, 63, 64, 65, 500, 999].contains(&tick) {
            interrupted = roundtrip(&interrupted);
        }
        assert_eq!(
            continuous.state_hash(),
            interrupted.state_hash(),
            "tick {tick}"
        );
    }
}

#[test]
fn snapshot_preserves_pregnancy_and_active_drought_events() {
    let mut sim = JungleSim::new(42, snapshot_config()).unwrap();
    sim.step(&[JungleCommand::Drought {
        duration_ticks: 500,
    }]);
    while sim.pregnant.iter().next().is_none() && sim.tick() < 300 {
        sim.step(&[]);
    }
    assert!(sim.pregnant.iter().next().is_some());
    assert_eq!(sim.drought_until, Some(500));
    let mut resumed = roundtrip(&sim);
    while sim.tick() < 600 {
        sim.step(&[]);
        resumed.step(&[]);
        assert_eq!(resumed.state_hash(), sim.state_hash());
    }
    assert_eq!(resumed.drought_until, None);
}

#[test]
fn golden_v1_snapshot_loads_and_reaches_reference() {
    let bytes = include_bytes!("../tests/fixtures/jungle-v1-t500.bin");
    let mut sim = JungleSim::load_snapshot(bytes).unwrap();
    assert_eq!(sim.tick(), 500);
    assert_eq!(sim.state_hash(), 0xcd31ad5b0c3262ff);
    for _ in 500..1000 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x22ed95e00104e9ba);
}
