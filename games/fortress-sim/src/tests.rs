use super::*;

fn scenario() -> Vec<(u64, Vec<FortressCommand>)> {
    #[derive(serde::Deserialize)]
    struct Entry {
        tick: u64,
        commands: Vec<FortressCommand>,
    }
    #[derive(serde::Deserialize)]
    struct File {
        schedule: Vec<Entry>,
    }
    let text = include_str!("../data/scenario_dig_in.toml");
    let file: File = toml::from_str(text).unwrap();
    file.schedule
        .into_iter()
        .map(|entry| (entry.tick, entry.commands))
        .collect()
}

#[test]
fn phases_do_not_allocate_after_warmup() {
    let schedule = scenario();
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut worst: Vec<(&str, u64, u64)> = Vec::new();
    for tick in 0..6000u64 {
        let commands: &[FortressCommand] = schedule
            .iter()
            .find(|(t, _)| *t == tick)
            .map_or(&[], |(_, c)| c.as_slice());
        let cmds = commands.to_vec();
        let n = allocation_counter::measure(|| sim.commands(&cmds)).count_total;
        if tick > 10 && n > 0 {
            worst.push(("commands", tick, n));
        }
        let mut count = |name: &'static str, sim: &mut FortressSim, f: fn(&mut FortressSim)| {
            let n = allocation_counter::measure(|| f(sim)).count_total;
            if tick > 10 && n > 0 {
                worst.push((name, tick, n));
            }
        };
        count("connectivity", &mut sim, FortressSim::update_connectivity);
        count("needs", &mut sim, FortressSim::needs);
        count("job_assign", &mut sim, FortressSim::job_assign);
        count("path", &mut sim, FortressSim::path);
        count("move", &mut sim, FortressSim::movement);
        count("work", &mut sim, FortressSim::work);
        count("lifecycle", &mut sim, FortressSim::lifecycle);
        count("environment", &mut sim, FortressSim::environment);
        sim.tick += 1;
    }
    worst.truncate(20);
    assert!(worst.is_empty(), "{worst:?}");
}

fn run_with_scenario(
    sim: &mut FortressSim,
    schedule: &[(u64, Vec<FortressCommand>)],
    ticks: u64,
    mut each: impl FnMut(&mut FortressSim),
) {
    for _ in 0..ticks {
        let tick = sim.tick();
        let commands = schedule
            .iter()
            .find(|(t, _)| *t == tick)
            .map_or(&[][..], |(_, c)| c.as_slice());
        sim.step(commands);
        each(sim);
    }
}

fn roundtrip(sim: &FortressSim) -> FortressSim {
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes).unwrap();
    let loaded = FortressSim::load_snapshot(&bytes).unwrap();
    assert_eq!(loaded.subsystem_hashes(), sim.subsystem_hashes());
    assert_eq!(loaded.state_hash(), sim.state_hash());
    loaded
}

#[test]
fn scenario_is_deterministic_and_keeps_invariants_every_tick() {
    let schedule = scenario();
    let mut a = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut b = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut hashes = Vec::new();
    run_with_scenario(&mut a, &schedule, 2000, |sim| {
        sim.check_invariants().unwrap();
        hashes.push(sim.state_hash());
    });
    let mut index = 0;
    run_with_scenario(&mut b, &schedule, 2000, |sim| {
        assert_eq!(sim.state_hash(), hashes[index], "tick {}", sim.tick());
        index += 1;
    });
    let other = FortressSim::new(7, FortressConfig::default()).unwrap();
    assert_ne!(other.hashes()[0], a.hashes()[0]);
}

#[test]
fn shuffled_slots_reproduce_every_logical_hash() {
    let schedule = scenario();
    let mut ordered = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut shuffled = FortressSim::new(
        42,
        FortressConfig {
            debug_shuffle_slots: true,
            ..FortressConfig::default()
        },
    )
    .unwrap();
    let mut hashes = Vec::new();
    run_with_scenario(&mut ordered, &schedule, 2000, |sim| {
        hashes.push(sim.state_hash())
    });
    let mut index = 0;
    run_with_scenario(&mut shuffled, &schedule, 2000, |sim| {
        assert_eq!(sim.state_hash(), hashes[index], "tick {}", sim.tick());
        index += 1;
    });
    assert!(ordered.counters.dug > 100 && ordered.counters.hauled > 50);
}

#[test]
fn save_load_mid_job_preserves_every_following_tick() {
    let schedule = scenario();
    // A small path budget leaves requests waiting in the queue at tick boundaries.
    let config = FortressConfig {
        path_budget_per_tick: 40,
        path_expansion_limit: 400,
        ..FortressConfig::default()
    };
    let mut continuous = FortressSim::new(42, config.clone()).unwrap();
    let mut interrupted = FortressSim::new(42, config).unwrap();
    let mut seen = [false; 3];
    for _ in 0..1500 {
        let tick = continuous.tick();
        let commands = schedule
            .iter()
            .find(|(t, _)| *t == tick)
            .map_or(&[][..], |(_, c)| c.as_slice());
        continuous.step(commands);
        interrupted.step(commands);
        let carrying = interrupted.rcols.carried.iter().any(Option::is_some);
        let digging = interrupted
            .rcols
            .task
            .iter()
            .zip(&interrupted.rcols.work)
            .any(|(task, &work)| matches!(task, Task::Dig { .. }) && work > 0);
        let waiting = !interrupted.queue.is_empty();
        let mut reload = false;
        for (flag, now) in seen.iter_mut().zip([carrying, digging, waiting]) {
            if now && !*flag {
                *flag = true;
                reload = true;
            }
        }
        if reload || continuous.tick().is_multiple_of(250) {
            interrupted = roundtrip(&interrupted);
        }
        assert_eq!(
            continuous.state_hash(),
            interrupted.state_hash(),
            "tick {}",
            continuous.tick()
        );
    }
    assert_eq!(
        seen, [true; 3],
        "carrying, digging and waiting states were reached"
    );
}

#[test]
fn golden_hash_after_two_thousand_scenario_ticks() {
    let schedule = scenario();
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    run_with_scenario(&mut sim, &schedule, 2000, |_| {});
    assert_eq!(sim.state_hash(), GOLDEN_ASTAR, "{:#018x}", sim.state_hash());
}

const GOLDEN_ASTAR: u64 = 0x0647c8823c28e430;

#[test]
fn scenario_digs_in_and_reaches_the_bottom_of_the_shaft() {
    let schedule = scenario();
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    run_with_scenario(&mut sim, &schedule, 2000, |_| {});
    assert!(sim.counters.dug >= 450, "dug {}", sim.counters.dug);
    assert_eq!(sim.diag.lowest_level, 2);
    let bottom = sim.world.get(Pos3::new(40, 48, 2)).unwrap();
    assert_eq!(bottom.kind, TileKind::StairUp);
    assert_eq!(sim.counters.invalid_designations, 3);
    assert_eq!(sim.counters.deaths, [0]);
}

/// A flat one-level world: rock at z=0, floor at z=1, air above.
fn flat(config: FortressConfig) -> FortressSim {
    let mut sim = FortressSim::new(5, config).unwrap();
    let (w, d) = (sim.config.world_size_xy, sim.config.world_depth);
    for z in 0..d {
        for y in 0..w {
            for x in 0..w {
                let tile = match z {
                    0 => Tile::new(TileKind::Solid, false, 3),
                    1 => Tile::new(TileKind::Open, true, 0),
                    _ => Tile::new(TileKind::Open, false, 0),
                };
                sim.world.set(Pos3::new(x, y, z), tile);
            }
        }
    }
    sim.world.clear_changes();
    sim.connectivity = Connectivity::new(&sim.world);
    for p in &mut sim.rcols.position {
        *p = Pos3::new(1, 1, 1);
    }
    for location in &mut sim.icols.location {
        *location = ItemLocation::Ground(Pos3::new(0, 0, 1));
    }
    sim.check_invariants().unwrap();
    sim
}

fn calm() -> FortressConfig {
    FortressConfig {
        world_size_xy: 24,
        world_depth: 8,
        ground_level: 3,
        mountain_center_x: 12,
        mountain_center_y: 12,
        mountain_radius: 6,
        mountain_height: 3,
        embark_x: 2,
        embark_y: 2,
        initial_residents: 2,
        initial_food: 0,
        hungry_at: 60_000,
        starve_at: 65_000,
        tired_at: 60_000,
        caravan_interval: 1_000_000,
        ..FortressConfig::default()
    }
}

fn set_wall(sim: &mut FortressSim, p: Pos3) {
    sim.world.set(p, Tile::new(TileKind::Solid, false, 3));
}

#[test]
fn sealed_ore_is_never_assigned_and_never_searched() {
    let mut sim = flat(calm());
    // A sealed pocket at x 15..17, y 15..17 with an ore wall inside reach only from within.
    for y in 14..=18 {
        for x in 14..=18 {
            if x == 14 || x == 18 || y == 14 || y == 18 {
                set_wall(&mut sim, Pos3::new(x, y, 1));
            }
        }
    }
    set_wall(&mut sim, Pos3::new(16, 16, 1));
    sim.step(&[FortressCommand::DesignateDig {
        from: Pos3::new(16, 16, 1),
        to: Pos3::new(16, 16, 1),
        mode: DigMode::Dig,
    }]);
    for _ in 0..400 {
        sim.step(&[]);
        sim.check_invariants().unwrap();
    }
    let job = sim.jobs.iter().next().unwrap();
    assert_eq!(job.state, JobState::Open);
    assert_eq!(sim.diag.path.searches, 0);
    assert_eq!(sim.diag.path.skipped, 0);
    assert_eq!(sim.diag.unreachable_prechecks, 0);
}

#[test]
fn two_workers_one_stone_exactly_one_carries_it() {
    let mut sim = flat(calm());
    sim.spawn_item(Pos3::new(6, 6, 1), ItemKind::Stone, 3);
    let stone = sim.items.iter_by_id().last().unwrap();
    sim.step(&[
        FortressCommand::Build {
            pos: Pos3::new(10, 4, 1),
            kind: BuildKind::Wall,
        },
        FortressCommand::Build {
            pos: Pos3::new(10, 8, 1),
            kind: BuildKind::Wall,
        },
    ]);
    let mut carried = 0;
    for _ in 0..300 {
        sim.step(&[]);
        sim.check_invariants().unwrap();
        let carriers = sim
            .rcols
            .carried
            .iter()
            .filter(|c| **c == Some(stone))
            .count();
        assert!(carriers <= 1);
        carried += carriers;
        let claimants = sim
            .rcols
            .task
            .iter()
            .filter(|t| matches!(t, Task::Build { item, .. } if *item == stone))
            .count();
        assert!(claimants <= 1);
    }
    assert!(carried > 0, "the stone was carried");
    assert_eq!(sim.counters.built, 1);
}

#[test]
fn a_wall_seals_jobs_and_digging_through_reopens_them() {
    let mut sim = flat(calm());
    for y in 0..24 {
        if y != 12 {
            set_wall(&mut sim, Pos3::new(12, y, 1));
        }
    }
    set_wall(&mut sim, Pos3::new(20, 20, 1));
    sim.world.clear_changes();
    sim.connectivity = Connectivity::new(&sim.world);
    sim.spawn_item(Pos3::new(2, 2, 1), ItemKind::Stone, 3);
    let (home, behind) = (Pos3::new(1, 1, 1), Pos3::new(19, 20, 1));
    assert!(sim.connectivity.reachable(home, behind));
    sim.step(&[FortressCommand::Build {
        pos: Pos3::new(12, 12, 1),
        kind: BuildKind::Wall,
    }]);
    for _ in 0..300 {
        sim.step(&[]);
    }
    assert_eq!(sim.counters.built, 1);
    assert!(!sim.connectivity.reachable(home, behind));
    let floods = sim.connectivity.stats.flood_fills;
    assert!(floods > 0);

    let dig = |p: Pos3| FortressCommand::DesignateDig {
        from: p,
        to: p,
        mode: DigMode::Dig,
    };
    sim.step(&[dig(Pos3::new(20, 20, 1))]);
    let searches = sim.diag.path.searches;
    for _ in 0..300 {
        sim.step(&[]);
        sim.check_invariants().unwrap();
        let job = sim.jobs.iter().next().unwrap();
        assert_eq!(job.state, JobState::Open, "the sealed job is never claimed");
    }
    assert_eq!(
        sim.diag.path.searches, searches,
        "no searches toward the sealed job"
    );

    sim.step(&[dig(Pos3::new(12, 12, 1))]);
    for _ in 0..600 {
        sim.step(&[]);
        sim.check_invariants().unwrap();
    }
    assert!(sim.connectivity.reachable(home, behind));
    assert_eq!(
        sim.counters.dug, 2,
        "the wall and the reopened job were dug"
    );
    assert!(
        sim.jobs
            .iter()
            .all(|job| matches!(job.data, JobKind::Haul(_)))
    );
}

fn small() -> FortressConfig {
    FortressConfig {
        world_size_xy: 32,
        world_depth: 12,
        ground_level: 4,
        mountain_center_x: 20,
        mountain_center_y: 16,
        mountain_radius: 10,
        mountain_height: 5,
        ore_veins: 4,
        ore_vein_length: 10,
        embark_x: 3,
        embark_y: 16,
        initial_residents: 4,
        initial_food: 6,
        max_residents: 64,
        max_items: 2_000,
        max_jobs: 2_000,
        caravan_interval: 200,
        ..FortressConfig::default()
    }
}

fn small_run(ticks: u64) -> FortressSim {
    let mut sim = FortressSim::new(42, small()).unwrap();
    let commands = [
        FortressCommand::DesignateStockpile {
            from: Pos3::new(0, 10, 4),
            to: Pos3::new(5, 13, 4),
        },
        FortressCommand::DesignateDig {
            from: Pos3::new(9, 15, 4),
            to: Pos3::new(22, 17, 4),
            mode: DigMode::Dig,
        },
        FortressCommand::DesignateDig {
            from: Pos3::new(23, 16, 4),
            to: Pos3::new(23, 16, 4),
            mode: DigMode::StairDown,
        },
        FortressCommand::DesignateDig {
            from: Pos3::new(23, 16, 1),
            to: Pos3::new(23, 16, 3),
            mode: DigMode::StairUpDown,
        },
    ];
    sim.step(&commands);
    for _ in 1..ticks {
        sim.step(&[]);
    }
    sim
}

const GOLDEN_SNAPSHOT: &str = "tests/fixtures/fortress-v1-small.bin";

#[test]
#[ignore = "writes the golden snapshot fixture"]
fn write_golden_snapshot() {
    let sim = small_run(300);
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes).unwrap();
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_SNAPSHOT),
        bytes,
    )
    .unwrap();
    let mut next = small_run(600);
    next.step(&[]);
    eprintln!("t300 {:#018x}", sim.state_hash());
    eprintln!("t601 {:#018x}", next.state_hash());
}

#[test]
fn golden_small_snapshot_loads_and_continues() {
    let bytes = include_bytes!("../tests/fixtures/fortress-v1-small.bin");
    let mut sim = FortressSim::load_snapshot(bytes).unwrap();
    assert_eq!(sim.tick(), 300);
    assert_eq!(sim.state_hash(), 0x4b5fe3168d2245db);
    for _ in 300..601 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x87e72f66415229d4);
}
