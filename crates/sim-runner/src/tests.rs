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

    fn step_profiled(&mut self, commands: &[usize], probe: &mut dyn PhaseProbe) {
        probe.enter("advance");
        self.step(commands);
        if ALLOCATE.with(std::cell::Cell::get) {
            std::hint::black_box(vec![self.tick; 11]);
        }
        probe.exit("advance");
        probe.enter("other");
        std::hint::black_box(self.tick);
        probe.exit("other");
    }

    fn scale_units(&self) -> Vec<(&'static str, u64)> {
        vec![("work", self.tick * 5), ("entities", 2)]
    }
    fn phase_dimensions(&self) -> Vec<(&'static str, &'static str)> {
        vec![("advance", "work")]
    }

    fn subsystem_hashes(&self) -> Vec<(&'static str, u64)> {
        vec![("a", self.tick), ("b", self.nonce)]
    }

    fn describe_entity(&self, id: u64) -> Option<Vec<(String, String)>> {
        (id == 7 && self.tick < 2).then(|| vec![("age".into(), self.tick.to_string())])
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

thread_local! {
    static ALLOCATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

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
        overrides: vec![],
        budget_p99_us: None,
        telemetry: TelemetryOptions::default(),
        artifacts: ArtifactOptions::default(),
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
        Err(RunnerError::DeterminismMismatch { tick: 2, first, second, .. }) if first != second
    ));
}

#[test]
fn mismatched_checkpoint_lists_include_both_real_hashes() {
    let first = [Checkpoint {
        tick: 2,
        hash: 10,
        ..Checkpoint::default()
    }];
    let second = [
        first[0].clone(),
        Checkpoint {
            tick: 4,
            hash: 20,
            ..Checkpoint::default()
        },
    ];
    assert!(matches!(
        verify_checkpoints(&first, &second),
        Err(RunnerError::DeterminismMismatch {
            tick: 2,
            first: 10,
            second: 20,
            ..
        })
    ));
    assert!(matches!(
        verify_checkpoints(&first, &second[1..]),
        Err(RunnerError::DeterminismMismatch {
            tick: 2,
            first: 10,
            second: 20,
            ..
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
        run_once::<StableSim>(&options(), &Start::New(config.clone()), &[], None),
        Err(RunnerError::InvariantViolation { tick: 2, message }) if message == "deliberate test violation"
    ));

    let opts = RunOptions {
        check_invariants_every: 0,
        ..options()
    };
    assert!(matches!(
        run_once::<StableSim>(&opts, &Start::New(config), &[], None),
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
    let run = run_once::<StableSim>(
        &options(),
        &Start::New(TestConfig::default()),
        &schedule,
        None,
    )
    .unwrap();
    assert_eq!(run.metrics.active_entities, 123);
}

#[test]
fn rejects_steps_that_advance_more_than_one_tick() {
    let config = TestConfig {
        tick_increment: 2,
        ..TestConfig::default()
    };
    assert!(matches!(
        run_once::<StableSim>(&options(), &Start::New(config), &[], None),
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

#[test]
fn phase_report_contains_both_phases_and_counters_exclude_probe_work() {
    let mut opts = options();
    opts.telemetry.profile_phases = true;
    opts.telemetry.worst_ticks = 2;
    opts.telemetry.allocation_counter = Some(|step| allocation_counter::measure(step).count_total);
    ALLOCATE.set(true);
    let report = run_simulation::<StableSim>(&opts).unwrap();
    assert_eq!(
        report
            .phases
            .iter()
            .map(|phase| phase.name)
            .collect::<Vec<_>>(),
        ["advance", "other"]
    );
    assert_eq!(report.worst_ticks.len(), 2);
    assert!(report.worst_ticks[0].duration_us >= report.worst_ticks[1].duration_us);
    assert_eq!(report.worst_ticks[0].phases_us.len(), 2);
    assert_eq!(report.average_scale_units["work"], 12.5);
    assert_eq!(report.phases[0].dimension, "work");
    assert!(report.phases[0].ns_per_unit_tick.is_some());
    assert_eq!(report.phases[1].dimension, "entities");
    assert_eq!(report.step_allocations_total, Some(3));
    assert_eq!(report.step_allocations_max, Some(1));

    opts.telemetry.alloc_warmup = 1;
    opts.telemetry.max_step_allocs = Some(0);
    let failure = run_simulation::<StableSim>(&opts);
    ALLOCATE.set(false);
    assert!(matches!(
        failure,
        Err(RunnerError::AllocationLimit {
            tick: 2,
            count: 1,
            limit: 0
        })
    ));
}

#[test]
fn mismatch_identifies_only_the_subsystem_that_differs() {
    let opts = RunOptions {
        verify_determinism: true,
        ..options()
    };
    let error = run_simulation::<TestSim<true>>(&opts).unwrap_err();
    assert!(error.to_string().ends_with("differing subsystems: b"));
}

#[test]
fn trace_writes_null_once_when_an_entity_dies() {
    let path =
        std::env::temp_dir().join(format!("terrarium-trace-test-{}.jsonl", std::process::id()));
    let mut opts = options();
    opts.telemetry.trace_ids = vec![7, 7];
    opts.telemetry.trace_out = Some(path.clone());
    opts.verify_determinism = true;
    run_simulation::<StableSim>(&opts).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["fields"]["age"], "0");
    assert_eq!(lines[1]["fields"]["age"], "1");
    assert_eq!(
        lines[2],
        serde_json::json!({"tick": 2, "entity": 7, "fields": null})
    );
}
