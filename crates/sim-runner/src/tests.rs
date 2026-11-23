use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use sim_host::StateHasher;

use super::*;

#[derive(Clone, Deserialize)]
struct TestConfig {
    finish_at: u64,
    invalid_at: Option<u64>,
    tick_increment: u64,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            finish_at: 3,
            invalid_at: None,
            tick_increment: 1,
        }
    }
}

struct TestSim<const UNSTABLE: bool> {
    config: TestConfig,
    tick: u64,
    value: usize,
    nonce: u64,
}

impl<const UNSTABLE: bool> Simulation for TestSim<UNSTABLE> {
    type Config = TestConfig;
    type Command = usize;

    fn new(_seed: u64, config: TestConfig) -> Result<Self, SimError> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let nonce = if UNSTABLE {
            COUNT.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        };
        Ok(Self {
            config,
            tick: 0,
            value: 0,
            nonce,
        })
    }

    fn step(&mut self, commands: &[usize]) {
        for &command in commands {
            self.value = self.value * 10 + command;
        }
        self.tick += self.config.tick_increment;
    }

    fn tick(&self) -> u64 {
        self.tick
    }

    fn is_finished(&self) -> bool {
        self.tick >= self.config.finish_at
    }

    fn metrics(&self) -> SimMetrics {
        SimMetrics {
            active_entities: self.value,
            custom_gauges: vec![],
        }
    }

    fn state_hash(&self) -> u64 {
        let mut hash = StateHasher::new();
        hash.write_u64(self.nonce);
        hash.write_u64(self.tick);
        hash.write_usize(self.value);
        hash.finish()
    }

    fn check_invariants(&self) -> Result<(), SimError> {
        if self.config.invalid_at.is_some_and(|tick| self.tick >= tick) {
            return Err(SimError::InvariantViolation(
                "deliberate test violation".into(),
            ));
        }
        Ok(())
    }
}

type StableSim = TestSim<false>;

fn options() -> RunOptions {
    RunOptions {
        game: "test".into(),
        list_games: false,
        ticks: 9,
        seed: 42,
        config: None,
        commands: None,
        verify_determinism: false,
        hash_interval: 2,
        check_invariants_every: 2,
        report_interval: None,
        dump_hashes: None,
        report_format: ReportFormat::Text,
    }
}

#[test]
fn detects_nondeterminism_at_the_first_checkpoint() {
    let opts = RunOptions {
        verify_determinism: true,
        ..options()
    };
    let result = run_simulation::<TestSim<true>>(&opts);
    assert!(matches!(
        result,
        Err(RunnerError::DeterminismMismatch { tick: 2, first, second }) if first != second
    ));
}

#[test]
fn mismatched_checkpoint_lists_include_both_real_hashes() {
    let first = [Checkpoint { tick: 2, hash: 10 }];
    let second = [first[0], Checkpoint { tick: 4, hash: 20 }];
    assert!(matches!(
        verify_checkpoints(&first, &second),
        Err(RunnerError::DeterminismMismatch {
            tick: 2,
            first: 10,
            second: 20
        })
    ));
    assert!(matches!(
        verify_checkpoints(&first, &second[1..]),
        Err(RunnerError::DeterminismMismatch {
            tick: 2,
            first: 10,
            second: 20
        })
    ));
}

#[test]
fn reports_invariant_failures_with_the_checked_tick() {
    let config = TestConfig {
        invalid_at: Some(2),
        ..TestConfig::default()
    };
    assert!(matches!(
        run_once::<StableSim>(&options(), config.clone(), &[], None),
        Err(RunnerError::InvariantViolation { tick: 2, message }) if message == "deliberate test violation"
    ));

    let opts = RunOptions {
        check_invariants_every: 0,
        ..options()
    };
    assert!(matches!(
        run_once::<StableSim>(&opts, config, &[], None),
        Err(RunnerError::InvariantViolation { tick: 3, .. })
    ));
}

#[test]
fn stops_early_when_the_simulation_finishes() {
    let opts = RunOptions {
        verify_determinism: true,
        ..options()
    };
    let report = run_simulation::<StableSim>(&opts).unwrap();
    assert_eq!(report.tick, 3);
    assert_eq!(report.ticks_executed, 3);
    assert!(report.ended_early);
    assert_eq!(report.verified_checkpoints, Some(2));
}

#[test]
fn steps_receive_commands_at_the_current_tick_including_tick_zero() {
    let schedule = [
        ScheduleEntry {
            tick: 0,
            commands: vec![1, 2],
        },
        ScheduleEntry {
            tick: 1,
            commands: vec![3],
        },
        ScheduleEntry {
            tick: 3,
            commands: vec![4],
        },
    ];
    let run = run_once::<StableSim>(&options(), TestConfig::default(), &schedule, None).unwrap();
    assert_eq!(run.metrics.active_entities, 123);
}

#[test]
fn rejects_steps_that_advance_more_than_one_tick() {
    let config = TestConfig {
        tick_increment: 2,
        ..TestConfig::default()
    };
    assert!(matches!(
        run_once::<StableSim>(&options(), config, &[], None),
        Err(RunnerError::ContractViolation(message)) if message.contains("from 0 to 1, got 2")
    ));
}

#[test]
fn end_on_interval_records_each_checkpoint_once() {
    let opts = RunOptions {
        ticks: 2,
        verify_determinism: true,
        ..options()
    };
    let report = run_simulation::<StableSim>(&opts).unwrap();
    assert_eq!(report.verified_checkpoints, Some(1));
}

#[test]
fn zero_ticks_has_only_a_final_hash_and_no_latency_statistics() {
    let opts = RunOptions {
        ticks: 0,
        hash_interval: 0,
        verify_determinism: true,
        ..options()
    };
    let report = run_simulation::<StableSim>(&opts).unwrap();
    assert_eq!(report.tick, 0);
    assert_eq!(report.step_total_seconds, 0.0);
    assert!(report.average_tps.is_none());
    assert!(report.latency_us.is_none());
    assert!(report.to_string().contains("Tick-Latenz (µs) n/a"));
    assert_eq!(report.verified_checkpoints, Some(1));
}
