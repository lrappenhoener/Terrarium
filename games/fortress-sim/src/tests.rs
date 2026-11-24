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
    let schedule = scenario_file("scenario_magma.toml");
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
        count("hazards", &mut sim, FortressSim::hazards);
        count("needs", &mut sim, FortressSim::needs);
        count("job_assign", &mut sim, FortressSim::job_assign);
        count("path", &mut sim, FortressSim::path);
        count("move", &mut sim, FortressSim::movement);
        count("work", &mut sim, FortressSim::work);
        count("plants", &mut sim, FortressSim::plants_step);
        count("orders", &mut sim, FortressSim::manage_orders);
        count("fluids", &mut sim, FortressSim::fluids);
        count("temperature", &mut sim, FortressSim::temperature_step);
        count("support", &mut sim, FortressSim::support_step);
        count("collapse", &mut sim, FortressSim::collapse_step);
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

const GOLDEN_ASTAR: u64 = 0xd80b08289b0dff3b;

#[test]
fn scenario_digs_in_and_reaches_the_bottom_of_the_shaft() {
    let schedule = scenario();
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    run_with_scenario(&mut sim, &schedule, 2000, |_| {});
    // Demand-driven hauling (Etappe 6) shifts the job mix: 426 tiles at the time.
    assert!(sim.counters.dug >= 420, "dug {}", sim.counters.dug);
    assert_eq!(sim.diag.lowest_level, 2);
    let bottom = sim.world.get(Pos3::new(40, 48, 2)).unwrap();
    assert_eq!(bottom.kind, TileKind::StairUp);
    assert_eq!(sim.counters.invalid_designations, 3);
    assert_eq!(sim.counters.deaths[0], 0, "no resident starves");
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
    for (p, _) in sim.world.iter_logical().collect::<Vec<_>>() {
        sim.fluid.set(p, 0);
    }
    sim.fluid.clear_changes();
    sim.balance = fluids::FluidBalance::default();
    sim.init_physics();
    for p in &mut sim.rcols.position {
        *p = Pos3::new(1, 1, 1);
    }
    for location in &mut sim.icols.location {
        *location = ItemLocation::Ground(Pos3::new(0, 0, 1));
    }
    sim.rebuild_item_index();
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
        physics: PhysicsConfig {
            river: false,
            magma_sea: false,
            ..PhysicsConfig::default()
        },
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
    sim.spawn_item(Pos3::new(6, 6, 1), kind(&sim, "stone"), 3, 1, 0);
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
    sim.spawn_item(Pos3::new(2, 2, 1), kind(&sim, "stone"), 3, 1, 0);
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

const GOLDEN_SNAPSHOT: &str = "tests/fixtures/fortress-v4-small.bin";

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
    let bytes = include_bytes!("../tests/fixtures/fortress-v4-small.bin");
    let mut sim = FortressSim::load_snapshot(bytes).unwrap();
    assert_eq!(sim.tick(), 300);
    assert_eq!(sim.state_hash(), 0x67a03d5dfb04d251);
    for _ in 300..601 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x082f9d656bf9ef2d);
}

#[test]
fn golden_hash_with_hierarchical_pathfinding() {
    let schedule = scenario();
    let config = FortressConfig {
        pathfinding: Pathfinding::Hierarchical,
        ..FortressConfig::default()
    };
    let mut sim = FortressSim::new(42, config).unwrap();
    run_with_scenario(&mut sim, &schedule, 2000, |sim| {
        if sim.tick() % 100 == 0 {
            sim.check_invariants().unwrap();
        }
    });
    assert_eq!(
        sim.state_hash(),
        GOLDEN_HIERARCHICAL,
        "{:#018x}",
        sim.state_hash()
    );
    // Demand-driven hauling (Etappe 6) shifts the job mix: 426 tiles at the time.
    assert!(sim.counters.dug >= 420, "dug {}", sim.counters.dug);
}

const GOLDEN_HIERARCHICAL: u64 = 0xebcf2633d7875684;

#[test]
fn aborting_on_a_full_stockpile_drops_outside_it() {
    let mut sim = flat(calm());
    sim.step(&[FortressCommand::DesignateStockpile {
        from: Pos3::new(4, 4, 1),
        to: Pos3::new(14, 14, 1),
    }]);
    for y in 4..=14 {
        for x in 4..=14 {
            sim.spawn_item(Pos3::new(x, y, 1), kind(&sim, "stone"), 3, 1, 0);
        }
    }
    let carried = sim
        .spawn_item(Pos3::new(1, 1, 1), kind(&sim, "ration"), 0, 1, 0)
        .unwrap();
    let id = sim.residents.iter_by_id().next().unwrap();
    let slot = sim.rslot(id).unwrap();
    sim.rcols.position[slot] = Pos3::new(9, 9, 1);
    sim.pick_up(slot, id, carried);
    sim.check_invariants().unwrap();
    sim.abort_task(id);
    let dropped = sim.item_ground(carried).unwrap();
    assert!(!sim.is_stockpile(dropped), "dropped at {dropped:?}");
    assert_eq!(dropped.chebyshev(Pos3::new(9, 9, 1)), 6);
    sim.check_invariants().unwrap();
}

#[test]
fn large_embarks_spawn_only_on_tiles_reachable_from_the_embark() {
    let config = FortressConfig {
        initial_residents: 400,
        world_size_xy: 192,
        ..FortressConfig::default()
    };
    let sim = FortressSim::new(42, config).unwrap();
    let embark = sim.embark();
    assert!(
        sim.rcols
            .position
            .iter()
            .all(|p| sim.connectivity.reachable(embark, *p))
    );
}

#[test]
fn changed_generator_version_is_reported_before_reconstruction() {
    let sim = small_run(20);
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes).unwrap();
    assert!(
        FortressSim::load_snapshot(&bytes)
            .unwrap()
            .load_warnings()
            .is_empty()
    );
    assert_eq!(&bytes[..8], b"FORTSNP4");
    assert_eq!(bytes[8], 3, "generator version varint");
    bytes[8] = 7;
    let error = FortressSim::load_snapshot(&bytes)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.ends_with("Weltgenerator geändert: Version 7 → 3"),
        "{error}"
    );
    // Payloads of earlier generator versions are a documented break.
    for (legacy, version) in [
        (
            &include_bytes!("../tests/fixtures/fortress-v1-small.bin")[..],
            1,
        ),
        (
            &include_bytes!("../tests/fixtures/fortress-v3-small.bin")[..],
            2,
        ),
    ] {
        let error = FortressSim::load_snapshot(legacy)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.ends_with(&format!("Weltgenerator geändert: Version {version} → 3")),
            "{error}"
        );
    }
}

#[test]
fn querying_the_full_view_every_tick_leaves_hashes_unchanged() {
    use crate::view::View;
    let schedule = scenario();
    let mut plain = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut observed = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut hashes = Vec::new();
    run_with_scenario(&mut plain, &schedule, 600, |sim| {
        hashes.push(sim.state_hash())
    });
    let (mut residents, mut items, mut jobs, mut path) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut tiles = 0usize;
    let mut index = 0;
    run_with_scenario(&mut observed, &schedule, 600, |sim| {
        let view = sim.view();
        for chunk in 0..view.chunk_count() as u32 {
            let _ = view.chunk_version(chunk);
            view.chunk_tiles(chunk, |p, _| {
                tiles += 1;
                let _ = view.debug_region(p);
            });
        }
        view.residents(&mut residents);
        view.items(&mut items);
        view.jobs(&mut jobs);
        for r in &residents {
            view.path(r.id, &mut path);
            let _ = view.describe(r.id);
        }
        let _ = (view.stockpiles().len(), view.counts(), View::chunk_dims());
        assert_eq!(sim.state_hash(), hashes[index], "tick {}", sim.tick());
        index += 1;
    });
    assert_eq!(tiles, 600 * observed.world.len());
    assert!(!items.is_empty() && !jobs.is_empty());
}

fn scenario_file(name: &str) -> Vec<(u64, Vec<FortressCommand>)> {
    #[derive(serde::Deserialize)]
    struct Entry {
        tick: u64,
        commands: Vec<FortressCommand>,
    }
    #[derive(serde::Deserialize)]
    struct File {
        schedule: Vec<Entry>,
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name);
    let file: File = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    file.schedule
        .into_iter()
        .map(|entry| (entry.tick, entry.commands))
        .collect()
}

#[test]
fn sleeping_chunks_do_not_change_behavior() {
    let schedule = scenario_file("scenario_magma.toml");
    let mut asleep = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut config = FortressConfig::default();
    config.physics.sleeping = false;
    let mut awake = FortressSim::new(42, config).unwrap();
    let mut hashes = Vec::new();
    let mut slept = 0;
    run_with_scenario(&mut asleep, &schedule, 3500, |sim| {
        hashes.push(sim.state_hash());
        slept = slept.max(sim.fluid_active.chunk_count() - sim.fluid_active.awake_count());
    });
    let mut index = 0;
    run_with_scenario(&mut awake, &schedule, 3500, |sim| {
        assert_eq!(sim.state_hash(), hashes[index], "tick {}", sim.tick());
        index += 1;
    });
    assert!(slept > 100, "chunks fell asleep in the sleeping run");
    assert!(awake.fluid_active.awake_count() > asleep.fluid_active.awake_count());
    assert!(
        asleep.balance.converted[1] > 0,
        "the run exercised obsidian"
    );
}

#[test]
fn fluids_are_conserved_for_ten_thousand_flood_ticks() {
    let schedule = scenario_file("scenario_flood.toml");
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    run_with_scenario(&mut sim, &schedule, 10_000, |sim| {
        sim.check_invariants().unwrap()
    });
    let b = sim.balance;
    assert!(b.sources[0] > 10_000 && b.evaporated > 0);
    assert_eq!(sim.count_fluids(), b.total);
    assert_eq!(i128::from(b.total[0]), b.expected(0));
}

#[test]
fn heat_balances_exactly_and_returns_to_ambient() {
    let schedule = scenario_file("scenario_magma.toml");
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut peak = 0;
    run_with_scenario(&mut sim, &schedule, 3000, |sim| {
        sim.check_invariants().unwrap();
        peak = peak.max(sim.heat.bound());
    });
    let h = sim.heat_balance;
    assert!(h.injected > 0 && h.returned > 0, "{h:?}");
    assert_eq!(sim.count_energy(), h.total);
    assert_eq!(h.total, h.injected - h.returned);
    assert!(
        peak > 0 && peak < sim.world.chunk_count(),
        "heat stays local"
    );
    assert!(sim.counters.deaths[2] < 10, "residents avoid hot tiles");
}

#[test]
fn an_undermined_slab_caves_in_and_becomes_stone() {
    let schedule = scenario_file("scenario_cavein.toml");
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut before = None;
    run_with_scenario(&mut sim, &schedule, 1500, |sim| {
        sim.check_invariants().unwrap();
        if sim.counters.collapses == 0 {
            before = Some(sim.icols.len());
        }
    });
    assert_eq!(sim.counters.collapses, 1);
    assert_eq!(sim.counters.collapsed_tiles, 16);
    for y in 53..=56 {
        for x in 25..=28 {
            let tile = *sim.world.get(Pos3::new(x, y, 15)).unwrap();
            assert_ne!(tile.kind, TileKind::Solid, "slab tile {x},{y} fell");
        }
    }
    assert!(
        sim.icols.len() >= before.unwrap(),
        "fallen tiles became stones"
    );
}

fn shuffled_matches_ordered(name: &str, ticks: u64) -> Vec<u64> {
    let schedule = scenario_file(name);
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
    run_with_scenario(&mut ordered, &schedule, ticks, |sim| {
        hashes.push(sim.state_hash())
    });
    let mut index = 0;
    run_with_scenario(&mut shuffled, &schedule, ticks, |sim| {
        assert_eq!(
            sim.state_hash(),
            hashes[index],
            "{name} tick {}",
            sim.tick()
        );
        index += 1;
    });
    hashes
}

/// Roundtrips whenever chunks fall asleep or wake, or a support check is pending.
fn save_load_through_transitions(name: &str, ticks: u64) -> usize {
    let schedule = scenario_file(name);
    let mut continuous = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut interrupted = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut awake = 0;
    let mut roundtrips = 0;
    for _ in 0..ticks {
        let tick = continuous.tick();
        let commands = schedule
            .iter()
            .find(|(t, _)| *t == tick)
            .map_or(&[][..], |(_, c)| c.as_slice());
        continuous.step(commands);
        interrupted.step(commands);
        let now = interrupted.fluid_active.awake_count();
        let transition = now != awake
            || !interrupted.support.is_empty()
            || interrupted.counters.collapses > 0 && roundtrips < 40;
        awake = now;
        if transition && roundtrips < 60 {
            interrupted = roundtrip(&interrupted);
            roundtrips += 1;
        }
        assert_eq!(
            continuous.state_hash(),
            interrupted.state_hash(),
            "{name} tick {}",
            continuous.tick()
        );
    }
    roundtrips
}

#[test]
fn flood_is_deterministic_shuffle_neutral_and_golden() {
    let hashes = shuffled_matches_ordered("scenario_flood.toml", 2500);
    assert_eq!(hashes[2499], GOLDEN_FLOOD, "{:#018x}", hashes[2499]);
}

#[test]
fn magma_is_deterministic_shuffle_neutral_and_golden() {
    let hashes = shuffled_matches_ordered("scenario_magma.toml", 2500);
    assert_eq!(hashes[2499], GOLDEN_MAGMA, "{:#018x}", hashes[2499]);
}

#[test]
fn cavein_is_deterministic_shuffle_neutral_and_golden() {
    let hashes = shuffled_matches_ordered("scenario_cavein.toml", 1500);
    assert_eq!(hashes[1499], GOLDEN_CAVEIN, "{:#018x}", hashes[1499]);
}

const GOLDEN_FLOOD: u64 = 0xb41b346373005396;
const GOLDEN_MAGMA: u64 = 0x609ebddfbdcdd4fd;
const GOLDEN_CAVEIN: u64 = 0x81da5b63db25a4cc;

#[test]
fn save_load_survives_flowing_water_heat_and_sleep_transitions() {
    assert!(save_load_through_transitions("scenario_magma.toml", 2500) > 20);
}

#[test]
fn save_load_survives_a_cave_in() {
    assert!(save_load_through_transitions("scenario_cavein.toml", 1300) > 5);
}

#[test]
fn a_flooded_room_hides_jobs_until_it_is_pumped_out() {
    let schedule = scenario_file("scenario_flood.toml");
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    run_with_scenario(&mut sim, &schedule, 5000, |_| {});
    let site = Pos3::new(52, 48, 2);
    assert!(
        crate::fluids::level(sim.fluid_at(site)) >= 4,
        "the room is flooded"
    );
    let target = Pos3::new(53, 48, 2);
    sim.step(&[FortressCommand::DesignateDig {
        from: target,
        to: target,
        mode: DigMode::Dig,
    }]);
    let job = sim.jobs.iter().find(|j| j.pos == target).unwrap().id;
    for _ in 0..300 {
        sim.step(&[]);
        assert_eq!(
            sim.jobs.get(job).unwrap().state,
            JobState::Open,
            "unreachable behind water"
        );
    }
    // Pump out: drain all water below the gallery, counted as a sink.
    let mut dug = false;
    for _ in 0..3000 {
        let wet: Vec<Pos3> = sim
            .fluid
            .iter_logical()
            .filter(|(p, f)| p.z < 10 && **f != 0 && !crate::fluids::is_magma(**f))
            .map(|(p, _)| p)
            .collect();
        for p in wet {
            sim.pump(p);
        }
        sim.step(&[]);
        sim.check_invariants().unwrap();
        if sim.world.get(target).unwrap().kind != TileKind::Solid {
            dug = true;
            break;
        }
    }
    assert!(dug, "the job became reachable and was done after pumping");
}

#[test]
fn hysteresis_markers_survive_save_and_load() {
    let mut sim = flat(calm());
    // Two enclosed wells keep their water level.
    let (rising, falling) = (Pos3::new(5, 5, 1), Pos3::new(10, 5, 1));
    for well in [rising, falling] {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            sim.world
                .set(well.offset(dx, dy, 0), Tile::new(TileKind::Solid, false, 3));
        }
    }
    sim.world.clear_changes();
    sim.connectivity = Connectivity::new(&sim.world);
    sim.place_fluid(rising, 32);
    sim.place_fluid(falling, 64);
    sim.step(&[]);
    assert!(!sim.world.get(rising).unwrap().impassable());
    assert!(sim.world.get(falling).unwrap().impassable());
    sim.place_fluid(rising, 48);
    sim.place_fluid(falling, 48);
    sim.step(&[]);
    let (hot_falling, hot_rising) = (Pos3::new(15, 15, 3), Pos3::new(15, 5, 3));
    sim.place_temperature(hot_falling, 95);
    assert!(sim.world.get(hot_falling).unwrap().impassable());
    sim.place_temperature(hot_falling, 75);
    sim.place_temperature(hot_rising, 75);
    sim.check_invariants().unwrap();
    // Same values, different histories, different markers.
    assert_eq!(crate::fluids::level(sim.fluid_at(rising)), 3);
    assert_eq!(crate::fluids::level(sim.fluid_at(falling)), 3);
    assert!(!sim.world.get(rising).unwrap().impassable());
    assert!(sim.world.get(falling).unwrap().impassable());
    assert_eq!(sim.temperature(hot_rising), sim.temperature(hot_falling));
    assert!(!sim.world.get(hot_rising).unwrap().impassable());
    assert!(sim.world.get(hot_falling).unwrap().impassable());

    let mut loaded = roundtrip(&sim);
    for ((p, a), (_, b)) in sim.world.iter_logical().zip(loaded.world.iter_logical()) {
        assert_eq!(a.impassable(), b.impassable(), "{p:?}");
        assert_eq!(
            sim.connectivity.is_standable(p),
            loaded.connectivity.is_standable(p),
            "{p:?}"
        );
    }
    let mut dump = String::new();
    sim.write_debug_dump(&mut dump).unwrap();
    assert!(
        dump.contains("tile:10,5,1\tOpen floor=true impassable=true"),
        "{dump}"
    );
    for _ in 0..1000 {
        sim.step(&[]);
        loaded.step(&[]);
        assert_eq!(sim.state_hash(), loaded.state_hash(), "tick {}", sim.tick());
    }
}

#[test]
#[ignore = "writes a debug dump for manual comparisons"]
fn write_scenario_dump() {
    let schedule = scenario_file(
        &std::env::var("DUMP_SCENARIO").unwrap_or_else(|_| "scenario_dig_in.toml".into()),
    );
    let mut sim = FortressSim::new(42, FortressConfig::default()).unwrap();
    let ticks = std::env::var("DUMP_TICKS").map_or(2000, |t| t.parse().unwrap());
    run_with_scenario(&mut sim, &schedule, ticks, |_| {});
    let mut dump = String::new();
    sim.write_debug_dump(&mut dump).unwrap();
    let path = std::env::var("DUMP_PATH").unwrap();
    std::fs::write(path, dump).unwrap();
}

fn kind(sim: &FortressSim, id: &str) -> ItemKind {
    ItemKind(GameContent::id_of(&sim.content.ids.items, id).unwrap())
}

#[test]
fn embedded_base_content_matches_the_content_directory() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("content/base");
    let files = sim_core::content::read_layer("base", &dir).unwrap();
    let on_disk: Vec<(&str, &str)> = files
        .iter()
        .map(|f| (f.path.as_str(), f.text.as_str()))
        .collect();
    assert_eq!(on_disk, crate::content::BASE.to_vec());
}

fn fixture_mod(name: &str) -> String {
    format!("{}/tests/fixtures/mods/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn a_mod_adds_a_material_and_a_recipe() {
    let base = GameContent::load(&[]).unwrap();
    let modded = GameContent::load(&[fixture_mod("marble")]).unwrap();
    assert_eq!(modded.materials.len(), base.materials.len() + 1);
    assert_eq!(modded.recipes.len(), base.recipes.len() + 1);
    assert_ne!(modded.fingerprint, base.fingerprint);
    let marble = GameContent::id_of(&modded.ids.materials, "marble").unwrap();
    let recipe = GameContent::id_of(&modded.ids.recipes, "marble_table").unwrap();
    let output = modded.recipes[usize::from(recipe)].outputs[0];
    assert_eq!(
        output.material,
        crate::content::OutputMaterial::Fixed(marble)
    );
    // Indices follow the sorted IDs, so the mod shifts later materials.
    let granite = GameContent::id_of(&base.ids.materials, "granite").unwrap();
    let modded_granite = GameContent::id_of(&modded.ids.materials, "granite").unwrap();
    assert_eq!(granite, modded_granite);

    let replaced = GameContent::load(&[fixture_mod("faster_smelting")]).unwrap();
    let smelt = GameContent::id_of(&replaced.ids.recipes, "smelt_ore").unwrap();
    assert_eq!(replaced.recipes[usize::from(smelt)].work_ticks, 40);

    let config = FortressConfig {
        mods: vec![fixture_mod("marble")],
        ..FortressConfig::default()
    };
    let sim = FortressSim::new(42, config).unwrap();
    let plain = FortressSim::new(42, FortressConfig::default()).unwrap();
    assert_ne!(sim.content_fingerprint(), plain.content_fingerprint());
}

#[test]
fn content_errors_reject_the_configuration() {
    let dir = std::env::temp_dir().join(format!("fortress-bad-mod-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.toml"),
        "[[recipe]]\nid = \"bad\"\nworkshop = \"smithy\"\nskill = \"smithing\"\nwork_ticks = 5\ninputs = [{ category = \"bar\" }]\noutputs = [{ item = \"pick\" }]\n",
    )
    .unwrap();
    let config = FortressConfig {
        mods: vec![dir.display().to_string()],
        ..FortressConfig::default()
    };
    let Err(error) = FortressSim::new(42, config) else {
        panic!("invalid content must be rejected");
    };
    assert!(
        error
            .to_string()
            .contains("recipe „bad“: workshop: unbekannte Referenz auf workshop „smithy“"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshots_with_changed_content_are_rejected() {
    let dir = std::env::temp_dir().join(format!("fortress-content-change-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = std::path::Path::new(&fixture_mod("marble")).join("marble.toml");
    std::fs::copy(&source, dir.join("marble.toml")).unwrap();
    let config = FortressConfig {
        mods: vec![dir.display().to_string()],
        ..small()
    };
    let sim = FortressSim::new(42, config).unwrap();
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes).unwrap();
    assert!(FortressSim::load_snapshot(&bytes).is_ok());
    let text = std::fs::read_to_string(dir.join("marble.toml")).unwrap();
    std::fs::write(
        dir.join("marble.toml"),
        text.replace("value = 4", "value = 5"),
    )
    .unwrap();
    let error = FortressSim::load_snapshot(&bytes)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("Inhalt geändert: Fingerabdruck"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn ground_units(sim: &FortressSim, kind: ItemKind) -> u64 {
    (0..sim.icols.len())
        .filter(|&slot| sim.icols.kind[slot] == kind)
        .map(|slot| u64::from(sim.icols.quantity[slot]))
        .sum()
}

#[test]
fn stacks_split_merge_and_consume_conserve_units() {
    use sim_core::rng::{SystemRng, below};
    let mut sim = flat(calm());
    let ration = kind(&sim, "ration");
    let material = sim.content.ration_material;
    let holder = sim.residents.iter_by_id().next().unwrap();
    let first = sim
        .spawn_item(Pos3::new(3, 3, 1), ration, material, 25, 0)
        .unwrap();
    sim.check_invariants().unwrap();
    assert_eq!(ground_units(&sim, ration), 25);
    let at = |sim: &FortressSim| -> Vec<(ItemId, u32)> {
        let mut found: Vec<(ItemId, u32)> = sim
            .items
            .iter_by_id()
            .filter(|&id| sim.icols.kind[sim.islot(id).unwrap()] == ration)
            .map(|id| (id, sim.icols.quantity[sim.islot(id).unwrap()]))
            .collect();
        found.sort_unstable();
        found
    };
    assert_eq!(
        at(&sim).iter().map(|e| e.1).collect::<Vec<_>>(),
        [10, 10, 5]
    );
    sim.ops.push(
        (9, 0),
        0,
        EntityCommand::Spawn(Op::Split {
            item: first,
            amount: 4,
        }),
    );
    sim.apply_ops();
    assert_eq!(
        at(&sim).iter().map(|e| e.1).collect::<Vec<_>>(),
        [6, 10, 5, 4]
    );
    let stacks = at(&sim);
    // Too large: 10 + 5 exceeds the stack size, nothing happens.
    sim.ops.push(
        (9, 1),
        0,
        EntityCommand::Spawn(Op::Merge {
            from: stacks[2].0,
            into: stacks[1].0,
        }),
    );
    sim.ops.push(
        (9, 2),
        0,
        EntityCommand::Spawn(Op::Merge {
            from: stacks[3].0,
            into: stacks[0].0,
        }),
    );
    sim.apply_ops();
    assert_eq!(
        at(&sim).iter().map(|e| e.1).collect::<Vec<_>>(),
        [10, 10, 5]
    );
    sim.consume_units(holder, first, 3);
    sim.check_invariants().unwrap();
    assert_eq!(ground_units(&sim, ration), 22);
    let ledger = sim.ledger[usize::from(ration.0)];
    assert_eq!((ledger.produced, ledger.removed), (25, 3));

    let mut rng = SystemRng::new(77, 3);
    for round in 0..400u32 {
        let stacks = at(&sim);
        match below(rng.next_u64(), 5) {
            0 => {
                let amount = 1 + below(rng.next_u64(), 15) as u32;
                let p = Pos3::new(2 + below(rng.next_u64(), 3) as i32, 3, 1);
                sim.spawn_item(p, ration, material, amount, 0);
            }
            1 if !stacks.is_empty() => {
                let (item, quantity) = stacks[below(rng.next_u64(), stacks.len() as u64) as usize];
                let amount = below(rng.next_u64(), u64::from(quantity) + 1) as u32;
                sim.ops.push(
                    (9, u64::from(round)),
                    0,
                    EntityCommand::Spawn(Op::Split { item, amount }),
                );
            }
            2 if stacks.len() >= 2 => {
                let a = stacks[below(rng.next_u64(), stacks.len() as u64) as usize].0;
                let b = stacks[below(rng.next_u64(), stacks.len() as u64) as usize].0;
                sim.ops.push(
                    (9, u64::from(round)),
                    0,
                    EntityCommand::Spawn(Op::Merge { from: a, into: b }),
                );
            }
            3 if !stacks.is_empty() => {
                let (item, quantity) = stacks[below(rng.next_u64(), stacks.len() as u64) as usize];
                let units = 1 + below(rng.next_u64(), u64::from(quantity)) as u32;
                sim.consume_units(holder, item, units);
            }
            _ => {
                if let Some(&(item, _)) = stacks.first() {
                    sim.ops.push(
                        (9, u64::from(round)),
                        1,
                        EntityCommand::Spawn(Op::DespawnItem(item)),
                    );
                }
            }
        }
        sim.apply_ops();
        sim.check_invariants().unwrap();
        let ledger = sim.ledger[usize::from(ration.0)];
        assert_eq!(ground_units(&sim, ration), ledger.produced - ledger.removed);
    }
}

#[test]
fn containers_keep_their_invariants_and_carry_their_contents() {
    let mut sim = flat(calm());
    let chest = kind(&sim, "chest");
    let stone = kind(&sim, "stone");
    let wood = GameContent::id_of(&sim.content.ids.materials, "oak_wood").unwrap();
    let home = Pos3::new(5, 5, 1);
    let container = sim.spawn_item(home, chest, wood, 1, 2).unwrap();
    let second = sim.spawn_item(home, chest, wood, 1, 0).unwrap();
    let mut stones = Vec::new();
    for _ in 0..9 {
        stones.push(sim.spawn_item(Pos3::new(9, 9, 1), stone, 3, 1, 0).unwrap());
    }
    for &s in &stones[..8] {
        assert!(sim.put_into(s, container));
    }
    assert!(!sim.put_into(stones[8], container), "capacity 8");
    assert!(!sim.put_into(second, container), "containers do not nest");
    assert!(!sim.put_into(container, container));
    sim.check_invariants().unwrap();
    let stone_category = sim.content.stone;
    let (_, nearest) = sim
        .nearest_free_item(Pos3::new(4, 5, 1), stone_category)
        .unwrap();
    assert_eq!(
        sim.item_ground(nearest),
        Some(home),
        "contents count at the container"
    );

    // Carrying the container hides its contents; dropping it moves them along.
    let rid = sim.residents.iter_by_id().next().unwrap();
    let rslot = sim.rslot(rid).unwrap();
    sim.pick_up(rslot, rid, container);
    sim.check_invariants().unwrap();
    let (_, nearest) = sim
        .nearest_free_item(Pos3::new(4, 5, 1), stone_category)
        .unwrap();
    assert_eq!(nearest, stones[8]);
    let away = Pos3::new(12, 12, 1);
    sim.drop_item(rslot, container, away);
    sim.check_invariants().unwrap();
    for &s in &stones[..8] {
        assert_eq!(sim.item_ground(s), Some(away));
        assert_eq!(sim.item_index.get(s.index).map(|e| e.1), Some(away));
    }
    // Removing the container leaves its contents on its tile.
    sim.ops
        .push((9, 0), 0, EntityCommand::Spawn(Op::DespawnItem(container)));
    sim.apply_ops();
    sim.check_invariants().unwrap();
    for &s in &stones[..8] {
        let slot = sim.islot(s).unwrap();
        assert_eq!(sim.icols.location[slot], ItemLocation::Ground(away));
    }
}

#[test]
fn item_queries_match_brute_force_with_reachability() {
    use sim_core::{
        index::index_distance,
        rng::{SystemRng, below},
    };
    for seed in [1u64, 2, 3] {
        let config = FortressConfig {
            item_search_cells: u32::MAX,
            item_search_candidates: u32::MAX,
            max_items: 2_000,
            ..calm()
        };
        let mut sim = flat(config);
        let w = sim.config.world_size_xy;
        // A wall splits the floor into two regions.
        for y in 0..w {
            for z in 1..3 {
                sim.world
                    .set(Pos3::new(12, y, z), Tile::new(TileKind::Solid, false, 3));
            }
        }
        sim.world.clear_changes();
        sim.connectivity = Connectivity::new(&sim.world);
        let mut rng = SystemRng::new(seed, 5);
        let kinds = sim.content.items.len() as u64;
        let materials = sim.content.materials.len() as u64;
        let chest = kind(&sim, "chest");
        let mut containers = Vec::new();
        let holder = sim.residents.iter_by_id().next().unwrap();
        for i in 0..600u32 {
            let p = Pos3::new(
                below(rng.next_u64(), w as u64) as i32,
                below(rng.next_u64(), w as u64) as i32,
                1,
            );
            if p.x == 12 {
                continue;
            }
            let k = if i % 40 == 0 {
                chest
            } else {
                ItemKind(below(rng.next_u64(), kinds) as u16)
            };
            let stack = sim.content.items[usize::from(k.0)].stack;
            let quantity = 1 + below(rng.next_u64(), u64::from(stack)) as u32;
            let material = below(rng.next_u64(), materials) as u16;
            let Some(id) = sim.spawn_item(p, k, material, quantity, 0) else {
                continue;
            };
            if k == chest {
                containers.push(id);
            } else if !containers.is_empty() && below(rng.next_u64(), 6) == 0 {
                let c = containers[below(rng.next_u64(), containers.len() as u64) as usize];
                sim.put_into(id, c);
            }
            if below(rng.next_u64(), 9) == 0 {
                sim.reservations.claim(ResKey::Item(id), holder, 1, 0);
            }
        }
        // Reservations without tasks only exclude items here; invariants would reject them.
        sim.reservations.resolve(sim.tick, |_, _| {});
        let categories = sim.content.categories.len() as u64;
        for _ in 0..300 {
            let at = Pos3::new(
                below(rng.next_u64(), w as u64) as i32,
                below(rng.next_u64(), w as u64) as i32,
                1,
            );
            if at.x == 12 {
                continue;
            }
            let what = if below(rng.next_u64(), 2) == 0 {
                content::Ingredient::Category(below(rng.next_u64(), categories) as u8)
            } else {
                content::Ingredient::Item(below(rng.next_u64(), kinds) as u16)
            };
            let input = content::Input {
                what,
                amount: 1,
                raw: below(rng.next_u64(), 2) == 0,
            };
            let material =
                (below(rng.next_u64(), 3) == 0).then(|| below(rng.next_u64(), materials) as u16);
            let expected = sim
                .items
                .iter_by_id()
                .filter_map(|id| {
                    let slot = sim.islot(id)?;
                    let p = sim.item_ground(id)?;
                    let fits = sim.content.accepts(&input, sim.icols.kind[slot].0)
                        && material.is_none_or(|m| sim.icols.material[slot] == m)
                        && !sim.reservations.is_reserved(ResKey::Item(id))
                        && sim.connectivity.reachable(at, p);
                    fits.then(|| (index_distance(at, p), id))
                })
                .min();
            assert_eq!(
                sim.nearest_item(at, &input, material, &[]),
                expected,
                "seed {seed}"
            );
        }
    }
}

#[test]
fn the_calendar_cycles_through_four_seasons() {
    let mut sim = flat(calm());
    let season = sim.config.ticks_per_day * sim.config.days_per_season;
    for (tick, expected) in [
        (0, 0),
        (season - 1, 0),
        (season, 1),
        (3 * season, 3),
        (4 * season, 0),
    ] {
        sim.tick = tick;
        assert_eq!(sim.season(), expected, "tick {tick}");
    }
}

#[test]
fn felling_trees_yields_logs_and_experience() {
    let mut sim = FortressSim::new(42, small()).unwrap();
    let trees = sim.plants.len();
    assert!(trees > 0, "embark has trees");
    let w = sim.config.world_size_xy;
    let depth = sim.config.world_depth;
    sim.step(&[FortressCommand::DesignateFelling {
        from: Pos3::new(0, 0, 0),
        to: Pos3::new(w - 1, w - 1, depth - 1),
    }]);
    let jobs = sim
        .jobs
        .iter()
        .filter(|j| matches!(j.data, JobKind::Fell(_)))
        .count();
    assert_eq!(jobs, trees);
    for tick in 0..3000 {
        sim.step(&[]);
        if tick % 50 == 0 {
            sim.check_invariants().unwrap();
        }
    }
    assert!(sim.counters.felled >= 5, "felled {}", sim.counters.felled);
    let log = kind(&sim, "log");
    let logs = sim.ledger[usize::from(log.0)];
    assert_eq!(logs.produced, sim.counters.felled * 3);
    let woodcutting = GameContent::id_of(&sim.content.ids.skills, "woodcutting").unwrap();
    let experienced =
        (0..sim.rcols.len()).any(|s| sim.rcols.skills[s].0[usize::from(woodcutting)] > 0);
    assert!(experienced);
    assert_eq!(sim.plants.len() as u64, trees as u64 - sim.counters.felled);
}

#[test]
fn underground_farms_sow_grow_and_harvest_food_and_seeds() {
    let mut sim = flat(calm());
    sim.step(&[FortressCommand::DesignateFarm {
        from: Pos3::new(4, 4, 1),
        to: Pos3::new(6, 6, 1),
        crop: "cave_mushroom".into(),
    }]);
    assert_eq!(sim.farms.len(), 9);
    sim.step(&[FortressCommand::DesignateFarm {
        from: Pos3::new(8, 8, 1),
        to: Pos3::new(8, 8, 1),
        crop: "unknown".into(),
    }]);
    let invalid = sim.counters.invalid_designations;
    assert!(invalid >= 1);
    let mushroom = kind(&sim, "cave_mushroom");
    let spawn = kind(&sim, "cave_mushroom_spawn");
    let seeds_before = sim.ledger[usize::from(spawn.0)];
    for tick in 0..4000 {
        sim.step(&[]);
        if tick % 25 == 0 {
            sim.check_invariants().unwrap();
        }
        if tick == 1000 {
            // Mid-growth save and load keeps the plants and the jobs.
            let reloaded = roundtrip(&sim);
            sim = reloaded;
        }
    }
    assert!(sim.counters.sown >= 9, "sown {}", sim.counters.sown);
    assert!(
        sim.counters.aborted_tasks < 50,
        "no abort loops: {}",
        sim.counters.aborted_tasks
    );
    assert!(
        sim.counters.harvested >= 9,
        "harvested {}",
        sim.counters.harvested
    );
    let food = sim.ledger[usize::from(mushroom.0)];
    assert_eq!(food.produced, 3 * sim.counters.harvested);
    let seeds = sim.ledger[usize::from(spawn.0)];
    assert_eq!(seeds.removed - seeds_before.removed, sim.counters.sown);
    assert_eq!(
        seeds.produced - seeds_before.produced,
        sim.counters.harvested
    );
}

#[test]
fn surface_crops_do_not_grow_in_winter() {
    let mut sim = flat(calm());
    let barley = GameContent::id_of(&sim.content.ids.plants, "barley").unwrap();
    let season = sim.config.ticks_per_day * sim.config.days_per_season;
    sim.tick = 3 * season;
    let plant = sim.create_plant(Pos3::new(3, 3, 1), barley, 0).unwrap();
    for _ in 0..500 {
        sim.step(&[]);
    }
    assert_eq!(sim.season(), 3);
    assert_eq!(sim.pcols.growth[sim.pslot(plant).unwrap()], 0);
    sim.tick = 4 * season;
    for _ in 0..500 {
        sim.step(&[]);
    }
    assert!(sim.pcols.growth[sim.pslot(plant).unwrap()] >= 450);
}

fn ready_workshop(sim: &mut FortressSim, kind: &str, pos: Pos3) -> u32 {
    let k = GameContent::id_of(&sim.content.ids.workshops, kind).unwrap();
    assert!(sim.place_workshop(k, pos));
    let w = sim.workshops.last().copied().unwrap();
    sim.jobs.remove(w.job.unwrap());
    let i = sim.workshop_index(w.id).unwrap();
    sim.workshops[i].job = None;
    sim.workshops[i].built = true;
    w.id
}

fn holds_items(sim: &FortressSim, id: ResidentId) -> usize {
    sim.reservations
        .iter()
        .filter(|r| r.holder == id && matches!(r.key, ResKey::Item(_)))
        .count()
}

#[test]
fn atomic_reservation_lets_exactly_one_workshop_start_without_deadlock() {
    let mut sim = flat(calm());
    let first = ready_workshop(&mut sim, "smelter", Pos3::new(4, 4, 1));
    let second = ready_workshop(&mut sim, "smelter", Pos3::new(12, 4, 1));
    let smelt = GameContent::id_of(&sim.content.ids.recipes, "smelt_ore").unwrap();
    let iron_ore = GameContent::id_of(&sim.content.ids.materials, "iron_ore").unwrap();
    let oak = GameContent::id_of(&sim.content.ids.materials, "oak_wood").unwrap();
    let (ore, charcoal, bar) = (kind(&sim, "ore"), kind(&sim, "charcoal"), kind(&sim, "bar"));
    sim.spawn_item(Pos3::new(8, 10, 1), ore, iron_ore, 1, 0);
    sim.spawn_item(Pos3::new(8, 11, 1), charcoal, oak, 1, 0);
    let _ = smelt;
    sim.step(&[
        FortressCommand::AddWorkOrder {
            workshop: Some(first),
            recipe: "smelt_ore".into(),
            amount: 1,
            condition: OrderCondition::Times,
        },
        FortressCommand::AddWorkOrder {
            workshop: Some(second),
            recipe: "smelt_ore".into(),
            amount: 1,
            condition: OrderCondition::Times,
        },
    ]);
    let crafting = |sim: &FortressSim| -> Vec<(ResidentId, u32)> {
        sim.residents
            .iter_by_id()
            .filter_map(|id| match sim.rcols.task[sim.rslot(id).unwrap()] {
                Task::Craft { workshop, .. } => Some((id, workshop)),
                _ => None,
            })
            .collect()
    };
    let mut started = false;
    let mut done_at = None;
    for tick in 0..2000 {
        sim.step(&[]);
        sim.check_invariants().unwrap();
        let active = crafting(&sim);
        assert!(active.len() <= 1, "only one workshop has ingredients");
        for id in sim.residents.iter_by_id() {
            let held = holds_items(&sim, id);
            let working = active.iter().any(|&(r, _)| r == id);
            assert!(
                held == 0 || (working && held == 2),
                "no partial reservations: {held}"
            );
        }
        started |= !active.is_empty();
        if sim.ledger[usize::from(bar.0)].produced == 1 {
            done_at = Some(tick);
            break;
        }
    }
    assert!(started);
    assert!(
        done_at.is_some(),
        "the first workshop finished: no deadlock"
    );
    assert_eq!(
        sim.orders.len(),
        1,
        "the second order waits for ingredients"
    );
    assert_eq!(sim.orders[0].done, 0);
    // New ingredients let the second workshop start and finish.
    sim.spawn_item(Pos3::new(8, 10, 1), ore, iron_ore, 1, 0);
    sim.spawn_item(Pos3::new(8, 11, 1), charcoal, oak, 1, 0);
    for _ in 0..2000 {
        sim.step(&[]);
        if sim.ledger[usize::from(bar.0)].produced == 2 {
            break;
        }
    }
    sim.check_invariants().unwrap();
    assert_eq!(sim.ledger[usize::from(bar.0)].produced, 2);
    let iron = GameContent::id_of(&sim.content.ids.materials, "iron").unwrap();
    for id in sim.items.iter_by_id() {
        let slot = sim.islot(id).unwrap();
        if sim.icols.kind[slot] == bar {
            assert_eq!(
                sim.icols.material[slot], iron,
                "smelted ore yields iron bars"
            );
        }
    }
    assert_eq!(sim.ledger[usize::from(ore.0)].removed, 2);
    assert_eq!(sim.ledger[usize::from(charcoal.0)].removed, 2);
}

#[test]
fn placing_a_workshop_builds_it_from_stones() {
    let mut sim = flat(calm());
    let stone = kind(&sim, "stone");
    for i in 0..3 {
        sim.spawn_item(Pos3::new(2 + i, 9, 1), stone, 3, 1, 0);
    }
    sim.step(&[
        FortressCommand::PlaceWorkshop {
            kind: "mason".into(),
            pos: Pos3::new(6, 6, 1),
        },
        FortressCommand::PlaceWorkshop {
            kind: "mason".into(),
            pos: Pos3::new(7, 7, 1),
        },
        FortressCommand::PlaceWorkshop {
            kind: "palace".into(),
            pos: Pos3::new(14, 14, 1),
        },
    ]);
    assert_eq!(
        sim.workshops.len(),
        1,
        "overlap and unknown kinds are rejected"
    );
    assert_eq!(sim.counters.invalid_designations, 2);
    for _ in 0..1500 {
        sim.step(&[]);
        if sim.workshops[0].built {
            break;
        }
    }
    sim.check_invariants().unwrap();
    assert!(sim.workshops[0].built);
    assert_eq!(sim.counters.workshops_built, 1);
    assert_eq!(
        sim.ledger[usize::from(stone.0)].removed,
        2,
        "two stones were used"
    );
    // A built mason cuts blocks with the remaining stone.
    let cut = GameContent::id_of(&sim.content.ids.recipes, "cut_blocks").unwrap();
    let w = sim.workshops[0].id;
    sim.add_craft_job(w, Some(cut)).unwrap();
    for _ in 0..1500 {
        sim.step(&[]);
    }
    sim.check_invariants().unwrap();
    let block = kind(&sim, "block");
    assert_eq!(sim.ledger[usize::from(block.0)].produced, 2);
    let masonry = GameContent::id_of(&sim.content.ids.skills, "masonry").unwrap();
    assert!((0..sim.rcols.len()).any(|s| sim.rcols.skills[s].0[usize::from(masonry)] > 0));
}

fn open_hauls(sim: &FortressSim) -> usize {
    sim.jobs
        .iter()
        .filter(|j| matches!(j.data, JobKind::Haul(_)) && j.state != JobState::Done)
        .count()
}

#[test]
fn zones_filter_and_relocate_by_priority() {
    let mut sim = flat(calm());
    let (stone, ration) = (kind(&sim, "stone"), kind(&sim, "ration"));
    let material = sim.content.ration_material;
    sim.step(&[FortressCommand::DesignateStockpile {
        from: Pos3::new(2, 10, 1),
        to: Pos3::new(9, 12, 1),
    }]);
    for i in 0..6 {
        sim.spawn_item(Pos3::new(3 + i, 4, 1), stone, 3, 1, 0);
        sim.spawn_item(Pos3::new(3 + i, 5, 1), ration, material, 1, 0);
    }
    for _ in 0..800 {
        sim.step(&[]);
    }
    let in_zone = |sim: &FortressSim, kind: ItemKind, zone: u32| {
        sim.items
            .iter_by_id()
            .filter(|&id| {
                let s = sim.islot(id).unwrap();
                sim.icols.kind[s] == kind
                    && sim
                        .item_ground(id)
                        .and_then(|p| sim.slot_index(p))
                        .is_some_and(|i| sim.slot_zone[i] == zone)
            })
            .count()
    };
    assert_eq!(in_zone(&sim, stone, 0), 6);
    // A second, higher zone for stone only takes the stones out of the first.
    sim.step(&[
        FortressCommand::DesignateStockpile {
            from: Pos3::new(2, 14, 1),
            to: Pos3::new(9, 14, 1),
        },
        FortressCommand::ConfigureStockpile {
            zone: 1,
            categories: vec!["stone".into()],
            materials: vec![],
            priority: 5,
            containers: false,
        },
        FortressCommand::ConfigureStockpile {
            zone: 7,
            categories: vec![],
            materials: vec![],
            priority: 5,
            containers: false,
        },
    ]);
    assert_eq!(sim.counters.invalid_designations, 1, "unknown zone");
    for tick in 0..1500 {
        sim.step(&[]);
        if tick % 50 == 0 {
            sim.check_invariants().unwrap();
        }
    }
    assert_eq!(
        in_zone(&sim, stone, 1),
        6,
        "stones moved to the higher zone"
    );
    assert_eq!(in_zone(&sim, stone, 0), 0);
    assert_eq!(in_zone(&sim, ration, 1), 0, "the filter keeps food out");
}

#[test]
fn transport_jobs_are_created_on_demand_and_capped() {
    let config = FortressConfig {
        max_open_hauls: 20,
        hauls_per_tick: 4,
        initial_residents: 4,
        ..calm()
    };
    let mut sim = flat(config);
    let stone = kind(&sim, "stone");
    for y in 3..21 {
        for x in 12..22 {
            sim.spawn_item(Pos3::new(x, y, 1), stone, 3, 1, 0);
        }
    }
    sim.step(&[]);
    assert_eq!(open_hauls(&sim), 0, "no stockpile, no transport jobs");
    sim.step(&[FortressCommand::DesignateStockpile {
        from: Pos3::new(2, 3, 1),
        to: Pos3::new(9, 20, 1),
    }]);
    let mut peak = 0;
    let mut previous = open_hauls(&sim);
    for tick in 0..3000 {
        sim.step(&[]);
        let open = open_hauls(&sim);
        assert!(open <= 20, "cap of open hauls");
        assert!(open <= previous + 4, "at most four new hauls per tick");
        peak = peak.max(open);
        previous = open;
        if tick % 100 == 0 {
            sim.check_invariants().unwrap();
        }
    }
    assert!(peak >= 10, "demand creates hauls: {peak}");
    assert!(sim.counters.hauled >= 100, "hauled {}", sim.counters.hauled);
    assert!(sim.diag.hauls_deferred > 0, "the cap deferred hauls");
}

#[test]
fn bulk_hauls_carry_several_items_in_a_barrel() {
    let mut sim = flat(calm());
    let (stone, barrel) = (kind(&sim, "stone"), kind(&sim, "barrel"));
    let oak = GameContent::id_of(&sim.content.ids.materials, "oak_wood").unwrap();
    for i in 0..8 {
        sim.spawn_item(Pos3::new(14 + i % 4, 14 + i / 4, 1), stone, 3, 1, 0);
    }
    let container = sim
        .spawn_item(Pos3::new(13, 13, 1), barrel, oak, 1, 0)
        .unwrap();
    sim.step(&[
        FortressCommand::DesignateStockpile {
            from: Pos3::new(2, 4, 1),
            to: Pos3::new(4, 6, 1),
        },
        FortressCommand::ConfigureStockpile {
            zone: 0,
            categories: vec!["stone".into()],
            materials: vec![],
            priority: 1,
            containers: true,
        },
    ]);
    for tick in 0..1500 {
        sim.step(&[]);
        if tick % 25 == 0 {
            sim.check_invariants().unwrap();
        }
    }
    assert!(sim.diag.bulk_hauls >= 1, "a bulk haul ran");
    let at = sim.item_ground(container).unwrap();
    assert!(sim.is_stockpile(at), "the barrel stands on the stockpile");
    assert!(
        sim.contents.count(container.index) >= 2,
        "it carries several stones"
    );
    let stored = (0..sim.icols.len())
        .filter(|&s| sim.icols.kind[s] == stone)
        .filter(|&s| {
            sim.items
                .id(s as u32)
                .and_then(|id| sim.item_ground(id))
                .is_some_and(|p| sim.is_stockpile(p))
        })
        .count();
    assert_eq!(stored, 8, "all stones are stored");
}

fn live_units(sim: &FortressSim, kind: ItemKind) -> u64 {
    let ledger = sim.ledger[usize::from(kind.0)];
    ledger.produced - ledger.removed
}

#[test]
fn an_until_stock_order_stops_at_its_target_and_resumes_after_use() {
    let mut sim = flat(calm());
    let mason = ready_workshop(&mut sim, "mason", Pos3::new(4, 4, 1));
    let (stone, block) = (kind(&sim, "stone"), kind(&sim, "block"));
    for i in 0..10 {
        sim.spawn_item(Pos3::new(10 + i % 5, 10 + i / 5, 1), stone, 3, 1, 0);
    }
    sim.step(&[FortressCommand::AddWorkOrder {
        workshop: Some(mason),
        recipe: "cut_blocks".into(),
        amount: 6,
        condition: OrderCondition::UntilStock,
    }]);
    for tick in 0..3000 {
        sim.step(&[]);
        if tick % 50 == 0 {
            sim.check_invariants().unwrap();
        }
    }
    assert_eq!(live_units(&sim, block), 6, "three runs of two blocks");
    assert_eq!(sim.orders.len(), 1, "until-stock orders stay");
    assert_eq!(sim.orders[0].done, 3);
    assert!(
        sim.workshops[0].job.is_none(),
        "the workshop idles at the target"
    );
    // Using up blocks makes the order due again.
    let used: Vec<ItemId> = sim
        .items
        .iter_by_id()
        .filter(|&id| sim.icols.kind[sim.islot(id).unwrap()] == block)
        .take(2)
        .collect();
    for (k, id) in used.into_iter().enumerate() {
        sim.ops
            .push((9, k as u64), 0, EntityCommand::Spawn(Op::DespawnItem(id)));
    }
    sim.apply_ops();
    for _ in 0..1500 {
        sim.step(&[]);
    }
    sim.check_invariants().unwrap();
    assert_eq!(live_units(&sim, block), 6);
    assert_eq!(sim.orders[0].done, 4);
}

#[test]
fn times_orders_end_cancel_aborts_and_repeat_uses_every_workshop() {
    let mut sim = flat(FortressConfig {
        initial_residents: 4,
        ..calm()
    });
    let first = ready_workshop(&mut sim, "mason", Pos3::new(4, 4, 1));
    let _second = ready_workshop(&mut sim, "mason", Pos3::new(10, 4, 1));
    let stone = kind(&sim, "stone");
    for i in 0..30 {
        sim.spawn_item(Pos3::new(4 + i % 10, 12 + i / 10, 1), stone, 3, 1, 0);
    }
    sim.step(&[
        FortressCommand::AddWorkOrder {
            workshop: Some(first),
            recipe: "cut_blocks".into(),
            amount: 2,
            condition: OrderCondition::Times,
        },
        FortressCommand::AddWorkOrder {
            workshop: Some(first),
            recipe: "wooden_bed".into(),
            amount: 1,
            condition: OrderCondition::Times,
        },
        FortressCommand::AddWorkOrder {
            workshop: None,
            recipe: "no_such_recipe".into(),
            amount: 1,
            condition: OrderCondition::Times,
        },
    ]);
    assert_eq!(
        sim.orders.len(),
        1,
        "carpentry at a mason and unknown recipes are rejected"
    );
    assert_eq!(sim.counters.invalid_designations, 2);
    for _ in 0..1500 {
        sim.step(&[]);
    }
    assert!(sim.orders.is_empty(), "a finished Times order disappears");
    let block = kind(&sim, "block");
    assert_eq!(live_units(&sim, block), 4);

    sim.step(&[FortressCommand::AddWorkOrder {
        workshop: None,
        recipe: "cut_blocks".into(),
        amount: 0,
        condition: OrderCondition::Repeat,
    }]);
    let repeat = sim.orders[0].id;
    let mut both = false;
    for _ in 0..400 {
        sim.step(&[]);
        both |= sim.workshops.iter().all(|w| w.order == Some(repeat));
    }
    assert!(both, "an unbound order runs in every matching workshop");
    sim.step(&[FortressCommand::CancelWorkOrder { order: repeat }]);
    sim.check_invariants().unwrap();
    assert!(sim.orders.is_empty());
    assert!(
        sim.workshops
            .iter()
            .all(|w| w.job.is_none() && w.order.is_none())
    );
    let crafting = (0..sim.rcols.len()).any(|s| matches!(sim.rcols.task[s], Task::Craft { .. }));
    assert!(!crafting, "workers of a cancelled order abort");
}

const GOLDEN_INDUSTRY: u64 = 0xb72c4d5a769a11a5;

#[test]
fn industry_is_deterministic_shuffle_neutral_and_golden() {
    let hashes = shuffled_matches_ordered("scenario_industry.toml", 3000);
    assert_eq!(hashes[2999], GOLDEN_INDUSTRY, "{:#018x}", hashes[2999]);
}

fn units_in_category(sim: &FortressSim, category: &str) -> u64 {
    let c = GameContent::id_of(&sim.content.ids.categories, category).unwrap() as u8;
    sim.ledger
        .iter()
        .enumerate()
        .filter(|(kind, _)| sim.content.category_of(*kind as u16) == c)
        .map(|(_, l)| l.produced - l.removed)
        .sum()
}

#[test]
fn the_industry_makes_tools_and_feeds_itself_without_caravans() {
    let schedule = scenario_file("scenario_industry.toml");
    let config = FortressConfig {
        caravan: false,
        ..FortressConfig::default()
    };
    let mut sim = FortressSim::new(42, config).unwrap();
    let mut food_at_half = 0;
    run_with_scenario(&mut sim, &schedule, 20_000, |sim| {
        if sim.tick() % 500 == 0 {
            sim.check_invariants().unwrap();
        }
        if sim.tick() == 10_000 {
            food_at_half = units_in_category(sim, "food");
        }
    });
    let tools = units_in_category(&sim, "tool");
    assert!(tools >= 12, "tools {tools}");
    assert_eq!(sim.counters.deaths[Death::Hunger as usize], 0);
    assert_eq!(sim.counters.caravans, 0);
    let food = units_in_category(&sim, "food");
    assert!(
        food >= 100 && food > food_at_half,
        "food {food_at_half} -> {food}"
    );
    let food_category = sim.content.food;
    let (mut produced, mut removed) = (0, 0);
    for (kind, ledger) in sim.ledger.iter().enumerate() {
        if sim.content.category_of(kind as u16) == food_category {
            produced += ledger.produced;
            removed += ledger.removed;
        }
    }
    assert!(
        produced > removed,
        "farms and kitchen outpace eating: {produced} > {removed}"
    );
}

#[test]
fn save_load_mid_production_preserves_every_following_tick() {
    let schedule = scenario_file("scenario_industry.toml");
    let mut continuous = FortressSim::new(42, FortressConfig::default()).unwrap();
    let mut interrupted = FortressSim::new(42, FortressConfig::default()).unwrap();
    let (mut carrying, mut working, mut filled, mut bulk) = (0, 0, 0, 0);
    let mut roundtrips = 0;
    for _ in 0..5000 {
        let tick = continuous.tick();
        let commands = schedule
            .iter()
            .find(|(t, _)| *t == tick)
            .map_or(&[][..], |(_, c)| c.as_slice());
        continuous.step(commands);
        interrupted.step(commands);
        let sim = &interrupted;
        let carry =
            (0..sim.rcols.len()).any(|s| matches!(sim.rcols.task[s], Task::Craft { stage: 1, .. }));
        let work = (0..sim.rcols.len()).any(|s| {
            matches!(sim.rcols.task[s], Task::Craft { stage: 2, .. }) && sim.rcols.work[s] > 1
        });
        let full = sim
            .items
            .iter_by_id()
            .any(|id| sim.contents.count(id.index) > 0);
        let hauling = (0..sim.rcols.len())
            .any(|s| matches!(sim.rcols.task[s], Task::BulkHaul { stage: 1 | 3, .. }));
        let due = (carry && carrying < 4)
            || (work && working < 4)
            || (full && filled < 2)
            || (hauling && bulk < 2);
        if due {
            carrying += usize::from(carry);
            working += usize::from(work);
            filled += usize::from(full);
            bulk += usize::from(hauling);
            interrupted = roundtrip(&interrupted);
            roundtrips += 1;
        }
        assert_eq!(
            continuous.state_hash(),
            interrupted.state_hash(),
            "tick {}",
            continuous.tick()
        );
    }
    assert!(
        carrying > 0 && working > 0 && filled > 0,
        "{carrying} {working} {filled}"
    );
    assert!(roundtrips >= 8, "roundtrips {roundtrips}");
}
