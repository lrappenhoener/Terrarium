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
    sim.world.as_mut_slice().fill(Tile {
        terrain: Terrain::Grass,
        biomass: 500,
        water_distance: 3,
    });
    sim.walkable = sim.world.as_slice().len();
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
        sim.position.iter().filter(|(_, p)| **p == target).count(),
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
            sim.vitals.get_mut(ids[i]).unwrap().hunger = hunger;
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
        assert_eq!(sim.vitals.get(ids[winner]).unwrap().hunger, 0);
        assert_ne!(sim.vitals.get(ids[1 - winner]).unwrap().hunger, 0);
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
    sim.vitals.get_mut(ids[0]).unwrap().hunger = 40;
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
    assert_eq!(sim.world.as_slice()[0].biomass, 488);
    assert_eq!(sim.drought_until, Some(2));
    sim.step(&[]);
    sim.step(&[]);
    assert_eq!(sim.drought_until, None);
    assert_eq!(sim.world.as_slice()[2].biomass, 517);
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
