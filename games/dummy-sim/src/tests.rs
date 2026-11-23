use super::*;

fn small_config() -> DummyConfig {
    DummyConfig {
        initial_units: 2,
        max_units: 5,
        pickup_radius: 0.001,
        ..DummyConfig::default()
    }
}

fn gauge(sim: &DummySim, name: &str) -> f64 {
    sim.metrics()
        .custom_gauges
        .into_iter()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
}

#[test]
fn same_seed_produces_same_hash_each_tick() {
    let config = DummyConfig {
        initial_units: 256,
        ..DummyConfig::default()
    };
    let mut first = DummySim::new(42, config.clone()).unwrap();
    let mut second = DummySim::new(42, config).unwrap();
    assert_eq!(first.state_hash(), second.state_hash());
    for tick in 0..300 {
        let spawn = [DummyCommand::SpawnWave { count: 20 }];
        let commands = if tick == 150 { &spawn[..] } else { &[] };
        first.step(commands);
        second.step(commands);
        assert_eq!(first.state_hash(), second.state_hash(), "tick {tick}");
    }
}

#[test]
fn different_seed_produces_different_hash() {
    let first = DummySim::new(42, small_config()).unwrap();
    let second = DummySim::new(43, small_config()).unwrap();
    assert_ne!(first.state_hash(), second.state_hash());
}

#[test]
fn invariants_hold_at_every_tick() {
    let config = DummyConfig {
        initial_units: 512,
        ..DummyConfig::default()
    };
    let mut sim = DummySim::new(42, config).unwrap();
    for tick in 1..=500 {
        sim.step(&[]);
        assert_eq!(sim.tick(), tick);
        sim.check_invariants().unwrap();
    }
}

#[test]
fn spawn_wave_limits_capacity_and_reuses_slots_in_lifo_order() {
    let mut sim = DummySim::new(42, small_config()).unwrap();
    sim.step(&[DummyCommand::SpawnWave { count: 1 }]);
    assert_eq!(sim.metrics().active_entities, 3);
    sim.step(&[DummyCommand::SpawnWave { count: usize::MAX }]);
    assert_eq!(sim.metrics().active_entities, 5);
    assert_eq!(sim.rejected_spawns, (usize::MAX - 2) as u128);

    sim.energy[1] = 0.0;
    sim.energy[3] = 0.0;
    sim.step(&[]);
    assert_eq!(sim.free_slots, [1, 3]);
    assert_eq!(sim.metrics().active_entities, 3);
    sim.step(&[DummyCommand::SpawnWave { count: 1 }]);
    assert_eq!(sim.free_slots, [1]);
    assert!(sim.alive[3]);
    assert_eq!(
        sim.energy[3],
        sim.config.initial_energy - sim.config.energy_drain
    );
    assert_eq!(sim.alive.len(), 5);
    assert_eq!(sim.metrics().active_entities, 4);
    sim.check_invariants().unwrap();
}

#[test]
fn commands_apply_in_order_and_invalid_drains_are_rejected() {
    let mut sim = DummySim::new(42, small_config()).unwrap();
    sim.step(&[
        DummyCommand::SetEnergyDrain { drain: 2.0 },
        DummyCommand::SetEnergyDrain { drain: 3.0 },
        DummyCommand::SetEnergyDrain { drain: -1.0 },
        DummyCommand::SetEnergyDrain { drain: f64::NAN },
        DummyCommand::SetEnergyDrain {
            drain: f64::INFINITY,
        },
    ]);
    assert_eq!(gauge(&sim, "energy_drain"), 3.0);
    assert_eq!(gauge(&sim, "avg_energy"), 77.0);
    assert_eq!(gauge(&sim, "rejected_commands"), 3.0);

    sim.step(&[DummyCommand::SetEnergyDrain { drain: 0.0 }]);
    assert_eq!(gauge(&sim, "avg_energy"), 77.0);
    sim.step(&[DummyCommand::SetEnergyDrain { drain: 100.0 }]);
    assert!(sim.is_finished());
}

#[test]
fn invalid_configs_return_invalid_config() {
    let configs = [
        DummyConfig {
            world_width: f64::NAN,
            ..small_config()
        },
        DummyConfig {
            initial_energy: 101.0,
            ..small_config()
        },
        DummyConfig {
            initial_units: 6,
            ..small_config()
        },
        DummyConfig {
            resource_nodes: 0,
            ..small_config()
        },
        DummyConfig {
            pickup_rate: 0.0,
            ..small_config()
        },
        DummyConfig {
            steering_jitter: f64::MAX,
            ..small_config()
        },
    ];
    for config in configs {
        assert!(matches!(
            DummySim::new(42, config),
            Err(SimError::InvalidConfig(_))
        ));
    }
}

#[test]
fn commands_roundtrip_in_json_and_toml() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Commands {
        commands: Vec<DummyCommand>,
    }

    let commands = Commands {
        commands: vec![
            DummyCommand::SpawnWave { count: 2000 },
            DummyCommand::SetEnergyDrain { drain: 0.8 },
        ],
    };
    let json = serde_json::to_string(&commands).unwrap();
    assert_eq!(serde_json::from_str::<Commands>(&json).unwrap(), commands);
    let toml = toml::to_string(&commands).unwrap();
    assert_eq!(toml::from_str::<Commands>(&toml).unwrap(), commands);
}

#[test]
fn partial_config_and_example_use_defaults() {
    let config: DummyConfig = toml::from_str("initial_units = 20").unwrap();
    assert_eq!(
        config,
        DummyConfig {
            initial_units: 20,
            ..DummyConfig::default()
        }
    );
    assert_eq!(
        toml::from_str::<DummyConfig>(include_str!("../data/config.toml")).unwrap(),
        DummyConfig::default()
    );
}

#[test]
fn default_population_loses_units_but_survives_5000_ticks() {
    let mut sim = DummySim::new(42, DummyConfig::default()).unwrap();
    for _ in 0..5000 {
        sim.step(&[]);
    }
    sim.check_invariants().unwrap();
    assert!(sim.metrics().active_entities > 0);
    assert!(gauge(&sim, "died_total") > 0.0);
}

#[test]
fn golden_hash_at_tick_1000() {
    let mut sim = DummySim::new(42, DummyConfig::default()).unwrap();
    for _ in 0..1000 {
        sim.step(&[]);
    }
    assert_eq!(sim.state_hash(), 0x20f21cbeee53c309);
}
