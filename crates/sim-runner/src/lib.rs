//! Game-independent execution, timing, and determinism verification.

mod container;
mod divergence;
mod input;
mod options;
mod profile;
mod repeat;
mod replay;
mod report;
pub use replay::replay_game;
mod trace;

use std::{
    error::Error,
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sim_host::{NoopProbe, PhaseProbe, SimError, SimMetrics, Simulation};
use thiserror::Error;

use input::{ScheduleEntry, load_schedule};
pub use options::parse_sweep;
pub use profile::{PhaseReport, WorstTick};
use profile::{PhaseSeries, Recorder, TickPhases, WorstTicks};
pub use repeat::{Outlier, RepeatReport};
pub use report::{LatencyStats, RunReport, SnapshotReport};
use trace::Trace;

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("I/O error ({context}): {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("cannot parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("unsupported input format for {} (expected .toml or .json)", .0.display())]
    UnsupportedFormat(PathBuf),
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error("invariant violation at tick {tick}: {message}")]
    InvariantViolation { tick: u64, message: String },
    #[error("determinism mismatch at tick {tick}: {first:#018x} vs {second:#018x}; checkpoint interval {interval}; differing subsystems: {}{details}", .subsystems.join(", "))]
    DeterminismMismatch {
        tick: u64,
        first: u64,
        second: u64,
        subsystems: Vec<&'static str>,
        interval: String,
        details: String,
    },
    #[error("replay mismatch in checkpoint interval ({from}, {to}]; differing subsystems: {}", .subsystems.join(", "))]
    ReplayMismatch {
        from: u64,
        to: u64,
        subsystems: Vec<String>,
    },
    #[error("save/load verification at tick {tick}: {message}")]
    SaveLoadMismatch { tick: u64, message: String },
    #[error("tick {tick} made {count} allocations (maximum {limit})")]
    AllocationLimit { tick: u64, count: u64, limit: u64 },
    #[error("step p99 {actual:.3} µs exceeds budget {limit:.3} µs")]
    LatencyBudget { actual: f64, limit: f64 },
    #[error("invalid options: {0}")]
    InvalidOptions(String),
    #[error("simulation contract violation: {0}")]
    ContractViolation(String),
    #[error("unknown game: {0}")]
    UnknownGame(String),
    #[error("simulation panicked at tick {tick}: {message}")]
    SimulationPanic { tick: u64, message: String },
}

impl From<sim_io::Error> for RunnerError {
    fn from(error: sim_io::Error) -> Self {
        match error {
            sim_io::Error::Io { context, source } => Self::Io { context, source },
            sim_io::Error::Parse { path, source } => Self::Parse { path, source },
            sim_io::Error::UnsupportedFormat(path) => Self::UnsupportedFormat(path),
            sim_io::Error::Sim(error) => Self::Sim(error),
            sim_io::Error::InvalidOptions(message) => Self::InvalidOptions(message),
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ReportFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub game: String,
    pub list_games: bool,
    pub ticks: u64,
    pub seed: u64,
    pub config: Option<PathBuf>,
    pub commands: Option<PathBuf>,
    pub verify_determinism: bool,
    /// Zero records only the final hash.
    pub hash_interval: u64,
    /// Zero checks only the final state.
    pub check_invariants_every: u64,
    /// None or zero disables intermediate reports.
    pub report_interval: Option<u64>,
    pub dump_hashes: Option<PathBuf>,
    pub report_format: ReportFormat,
    pub overrides: Vec<String>,
    pub budget_p99_us: Option<f64>,
    pub telemetry: TelemetryOptions,
    pub artifacts: ArtifactOptions,
}

#[derive(Debug, Clone)]
pub struct ArtifactOptions {
    pub save_snapshot_at: Vec<u64>,
    pub snapshot_dir: Option<PathBuf>,
    pub load_snapshot: Option<PathBuf>,
    pub verify_save_load: bool,
    pub roundtrip_at: Vec<u64>,
    pub record_replay: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub force: bool,
    pub dump_state_at: Option<u64>,
    pub bisect: bool,
    pub diff_dir: PathBuf,
    /// Ticks between replay flushes when no checkpoint is due.
    pub replay_flush_every: u64,
    pub replay_max_bytes: Option<u64>,
}

impl Default for ArtifactOptions {
    fn default() -> Self {
        Self {
            save_snapshot_at: Vec::new(),
            snapshot_dir: None,
            load_snapshot: None,
            verify_save_load: false,
            roundtrip_at: Vec::new(),
            record_replay: None,
            replay: None,
            force: false,
            dump_state_at: None,
            bisect: false,
            diff_dir: "divergence".into(),
            replay_flush_every: 100,
            replay_max_bytes: None,
        }
    }
}

enum Start<C> {
    New(C),
    Snapshot(container::Container, Duration),
}

impl<C: Clone> Start<C> {
    fn instantiate<S: Simulation<Config = C>>(&self, opts: &RunOptions) -> Result<S, RunnerError> {
        match self {
            Self::New(config) => Ok(S::new(opts.seed, config.clone())?),
            Self::Snapshot(data, _) => Ok(container::restore::<S>(&opts.game, data)?),
        }
    }

    fn tick(&self) -> u64 {
        match self {
            Self::New(_) => 0,
            Self::Snapshot(data, _) => data.header.tick,
        }
    }

    fn seed(&self, default: u64) -> u64 {
        match self {
            Self::New(_) => default,
            Self::Snapshot(data, _) => data.header.seed,
        }
    }
}

fn io_at(path: &Path, source: io::Error) -> RunnerError {
    RunnerError::Io {
        context: path.display().to_string(),
        source,
    }
}

pub type StepAllocationCounter = fn(&mut dyn FnMut()) -> u64;

#[derive(Debug, Clone)]
pub struct TelemetryOptions {
    pub profile_phases: bool,
    pub worst_ticks: u64,
    pub allocation_counter: Option<StepAllocationCounter>,
    pub max_step_allocs: Option<u64>,
    pub alloc_warmup: u64,
    pub trace_ids: Vec<u64>,
    pub trace_every: u64,
    pub trace_out: Option<PathBuf>,
    /// Runs of the same simulation compared tick by tick; 1 disables the comparison.
    pub repeat: u32,
    /// Multiple of a run's median tick above which a tick counts as an outlier.
    pub outlier_factor: f64,
}

impl Default for TelemetryOptions {
    fn default() -> Self {
        Self {
            profile_phases: false,
            worst_ticks: 0,
            allocation_counter: None,
            max_step_allocs: None,
            alloc_warmup: 10,
            trace_ids: Vec::new(),
            trace_every: 1,
            trace_out: None,
            repeat: 1,
            outlier_factor: 3.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Checkpoint {
    tick: u64,
    hash: u64,
    active: usize,
    units: Vec<(&'static str, u64)>,
    snapshot: Option<Vec<u8>>,
    subsystems: Vec<(&'static str, u64)>,
}

struct CompletedRun {
    tick: u64,
    hash: u64,
    checkpoints: Vec<Checkpoint>,
    latencies: Vec<u64>,
    wall_time: Duration,
    metrics: SimMetrics,
    phases: Vec<PhaseSeries>,
    worst_ticks: Vec<WorstTick>,
    snapshots: Vec<SnapshotReport>,
    roundtrips: usize,
    dimensions: Vec<(&'static str, &'static str)>,
    step_allocations_total: u128,
    step_allocations_max: u64,
    tick_phases: Option<TickPhases>,
}

pub fn run_simulation<S: Simulation>(opts: &RunOptions) -> Result<RunReport, RunnerError> {
    if opts
        .budget_p99_us
        .is_some_and(|limit| !limit.is_finite() || limit < 0.0)
    {
        return Err(RunnerError::InvalidOptions(
            "p99 budget must be finite and nonnegative".into(),
        ));
    }
    let playback = opts
        .artifacts
        .replay
        .as_deref()
        .map(replay::Playback::read)
        .transpose()?;
    let mut replay_opts;
    let opts = if let Some(playback) = &playback {
        if playback.header.game != opts.game {
            return Err(container::invalid(format!(
                "replay is for {}, selected game is {}",
                playback.header.game, opts.game
            )));
        }
        replay_opts = opts.clone();
        playback.options(&mut replay_opts);
        &replay_opts
    } else {
        opts
    };
    let telemetry = &opts.telemetry;
    if telemetry.repeat == 0
        || !telemetry.outlier_factor.is_finite()
        || telemetry.outlier_factor <= 0.0
    {
        return Err(RunnerError::InvalidOptions(
            "--repeat needs at least one run and --outlier-factor a positive factor".into(),
        ));
    }
    if telemetry.max_step_allocs.is_some() && telemetry.allocation_counter.is_none() {
        return Err(RunnerError::InvalidOptions(
            "an allocation limit requires an allocation counter".into(),
        ));
    }
    if telemetry.trace_ids.is_empty() == telemetry.trace_out.is_some() || telemetry.trace_every == 0
    {
        return Err(RunnerError::InvalidOptions(
            "traces require both --trace-entity and --trace-out, and a positive interval".into(),
        ));
    }
    if telemetry.trace_out.is_some() && telemetry.trace_out == opts.dump_hashes {
        return Err(RunnerError::InvalidOptions(
            "trace and CSV output paths must differ".into(),
        ));
    }
    if opts.artifacts.verify_save_load
        || opts.artifacts.load_snapshot.is_some()
        || !opts.artifacts.save_snapshot_at.is_empty()
    {
        container::require_snapshots::<S>()?;
    }
    if !opts.artifacts.save_snapshot_at.is_empty() && opts.artifacts.snapshot_dir.is_none() {
        return Err(RunnerError::InvalidOptions(
            "--save-snapshot-at requires --snapshot-dir".into(),
        ));
    }
    let mut diagnostics = io::stderr().lock();
    let mut prepared = None;
    let start = if let Some(playback) = &playback {
        playback.header.warn_build(&mut diagnostics)?;
        playback.start::<S>()?
    } else if let Some(path) = &opts.artifacts.load_snapshot {
        if opts.config.is_some() || !opts.overrides.is_empty() {
            return Err(RunnerError::InvalidOptions(
                "a snapshot supplies the configuration".into(),
            ));
        }
        let begin = Instant::now();
        let data = container::read(container::SNAPSHOT, path)?;
        data.header.warn_build(&mut diagnostics)?;
        Start::Snapshot(data, begin.elapsed())
    } else {
        let config = options::prepare_config(opts.config.as_deref(), &opts.overrides)?;
        let decoded = config.decode()?;
        prepared = Some(config);
        Start::New(decoded)
    };
    if start.tick() > opts.ticks {
        return Err(RunnerError::InvalidOptions(format!(
            "snapshot tick {} exceeds target tick {}",
            start.tick(),
            opts.ticks
        )));
    }
    if opts
        .artifacts
        .save_snapshot_at
        .iter()
        .any(|&tick| tick < start.tick() || tick > opts.ticks)
    {
        return Err(RunnerError::InvalidOptions(
            "snapshot save tick is outside the run".into(),
        ));
    }
    if opts
        .artifacts
        .dump_state_at
        .is_some_and(|tick| !(start.tick()..=opts.ticks).contains(&tick))
    {
        return Err(RunnerError::InvalidOptions(
            "state dump tick lies outside the run".into(),
        ));
    }
    let roundtrips = roundtrip_ticks(opts, start.tick())?;
    let schedule = match (&playback, &opts.commands) {
        (Some(playback), _) => playback.schedule()?,
        (None, Some(path)) => load_schedule::<S::Command>(path, opts.ticks, &mut diagnostics)?,
        (None, None) => Vec::new(),
    };
    let first = run_once::<S>(
        opts,
        &start,
        &schedule,
        Some(&mut diagnostics),
        Pass {
            playback: playback.as_ref(),
            config: prepared.as_ref(),
            retain_snapshots: opts.artifacts.bisect
                && (opts.verify_determinism || opts.artifacts.verify_save_load)
                && S::snapshot_version() != 0,
            timing: telemetry.repeat > 1,
            ..Pass::default()
        },
    );
    let mut first = match (first, &playback) {
        (Err(RunnerError::SimulationPanic { tick, message }), Some(playback)) => {
            return Err(RunnerError::SimulationPanic {
                tick,
                message: format!("{message} ({})", playback.explain_panic(tick)),
            });
        }
        (result, _) => result?,
    };
    drop(diagnostics);
    if let Some(playback) = &playback {
        playback.check(&first.checkpoints)?;
    }

    if let Some(path) = &opts.dump_hashes {
        dump_hashes(path, &first.checkpoints)?;
    }
    let total_ns = first
        .latencies
        .iter()
        .map(|&ns| u128::from(ns))
        .sum::<u128>() as f64;
    let step_total_seconds = total_ns / 1_000_000_000.0;
    let ticks_executed = first.latencies.len() as u64;
    let average_active_entities = first
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.active as f64)
        .sum::<f64>()
        / first.checkpoints.len() as f64;
    let entity_ticks = average_active_entities * ticks_executed as f64;
    let mut scale_units = std::collections::BTreeMap::new();
    for checkpoint in &first.checkpoints {
        for &(name, units) in &checkpoint.units {
            *scale_units.entry(name).or_insert(0.0) += units as f64;
        }
    }
    for units in scale_units.values_mut() {
        *units /= first.checkpoints.len() as f64;
    }
    let phases = first
        .phases
        .into_iter()
        .map(|series| series.report(total_ns, ticks_executed, &first.dimensions, &scale_units))
        .collect();
    let mut timings = Vec::new();
    if telemetry.repeat > 1 {
        timings.push(repeat::Timing {
            latencies: first.latencies.clone(),
            phases: first.tick_phases.take(),
        });
    }
    let latency_us = LatencyStats::from_samples(&mut first.latencies);
    if let Some(limit) = opts.budget_p99_us
        && let Some(actual) =
            report::nearest_rank(&first.latencies, 99).map(|ns| ns as f64 / 1000.0)
        && actual > limit
    {
        return Err(RunnerError::LatencyBudget { actual, limit });
    }
    drop(first.latencies);

    let verified_checkpoints = if opts.verify_determinism {
        let second = run_once::<S>(opts, &start, &schedule, None, Pass::default())?;
        if let Some(difference) = divergence::compare(&first.checkpoints, &second.checkpoints) {
            let error = difference.error();
            if let Some(exact) = divergence::locate::<S>(
                opts,
                &start,
                &schedule,
                &[],
                &first.checkpoints,
                &difference,
            )? {
                return Err(RunnerError::DeterminismMismatch {
                    tick: exact.tick,
                    first: exact.first,
                    second: exact.second,
                    subsystems: exact.subsystems,
                    interval: difference.interval(),
                    details: exact.details,
                });
            }
            return Err(error);
        }
        Some(first.checkpoints.len())
    } else {
        None
    };
    let verified_save_load = if opts.artifacts.verify_save_load {
        let second = run_once::<S>(
            opts,
            &start,
            &schedule,
            None,
            Pass {
                roundtrips: &roundtrips,
                ..Pass::default()
            },
        )?;
        if let Some(difference) = divergence::compare(&first.checkpoints, &second.checkpoints) {
            let exact = divergence::locate::<S>(
                opts,
                &start,
                &schedule,
                &roundtrips,
                &first.checkpoints,
                &difference,
            )?;
            let tick = exact.as_ref().map_or(difference.tick, |exact| exact.tick);
            let window = difference.error().to_string();
            let interval = difference.interval();
            let last = roundtrips
                .iter()
                .rev()
                .copied()
                .find(|&point| point <= tick);
            let context = last.map_or_else(
                || "before first reload".into(),
                |point| format!("{} ticks after reload at {point}", tick - point),
            );
            let message = exact.map_or_else(
                || format!("checkpoint mismatch {context}: {window}"),
                |exact| format!("exact mismatch {context} in checkpoint interval {interval}; differing subsystems: {}{}", exact.subsystems.join(", "), exact.details),
            );
            return Err(RunnerError::SaveLoadMismatch { tick, message });
        }
        Some(second.roundtrips)
    } else {
        None
    };
    let repeat_report = if timings.is_empty() {
        None
    } else {
        for _ in 1..telemetry.repeat {
            let mut run = run_once::<S>(
                opts,
                &start,
                &schedule,
                None,
                Pass {
                    timing: true,
                    ..Pass::default()
                },
            )?;
            timings.push(repeat::Timing {
                latencies: std::mem::take(&mut run.latencies),
                phases: run.tick_phases.take(),
            });
        }
        let (report, worst) = repeat::analyze(
            &timings,
            start.tick(),
            telemetry.outlier_factor,
            usize::try_from(telemetry.worst_ticks).unwrap_or(usize::MAX),
        );
        first.worst_ticks = worst;
        Some(report)
    };
    let tick = first.tick;
    Ok(RunReport {
        tick,
        ticks_executed,
        ended_early: tick < opts.ticks,
        step_total_seconds,
        wall_total_seconds: first.wall_time.as_secs_f64(),
        average_tps: (step_total_seconds > 0.0).then(|| ticks_executed as f64 / step_total_seconds),
        latency_us,
        active_entities: first.metrics.active_entities,
        custom_gauges: first.metrics.custom_gauges,
        state_hash: format!("{:#018x}", first.hash),
        verified_checkpoints,
        verified_save_load,
        replay_verified: playback.is_some(),
        replay_status: playback.as_ref().map(replay::Playback::status),
        average_active_entities,
        ns_per_active_entity_tick: (entity_ticks > 0.0).then(|| total_ns / entity_ticks),
        phases,
        worst_ticks: first.worst_ticks,
        snapshots: first.snapshots,
        average_scale_units: scale_units,
        step_allocations_total: telemetry
            .allocation_counter
            .map(|_| first.step_allocations_total),
        step_allocations_max: telemetry
            .allocation_counter
            .map(|_| first.step_allocations_max),
        repeat: repeat_report,
    })
}

#[derive(Default)]
struct Pass<'a> {
    /// Keeps per-tick phase times and profiles without diagnostics (`--repeat`).
    timing: bool,
    roundtrips: &'a [u64],
    playback: Option<&'a replay::Playback>,
    config: Option<&'a options::ConfigText>,
    retain_snapshots: bool,
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".into())
}

/// Runs one pass. The diagnostic pass streams a replay if requested and closes it
/// with the outcome, including panics and runner errors.
fn run_once<S: Simulation>(
    opts: &RunOptions,
    start: &Start<S::Config>,
    schedule: &[ScheduleEntry<S::Command>],
    diagnostics: Option<&mut dyn Write>,
    pass: Pass<'_>,
) -> Result<CompletedRun, RunnerError> {
    let mut writer = None;
    let result = run_pass::<S>(opts, start, schedule, diagnostics, pass, &mut writer);
    if let Some(writer) = writer {
        let (tick, ending) = match &result {
            Ok(run) => (
                run.tick,
                replay::Ending::Finished {
                    tick: run.tick,
                    hash: run.hash,
                },
            ),
            Err(RunnerError::SimulationPanic { tick, message }) => (
                *tick,
                replay::Ending::Panic {
                    tick: *tick,
                    message: message.clone(),
                },
            ),
            Err(error) => (
                0,
                replay::Ending::Failed {
                    message: error.to_string(),
                },
            ),
        };
        writer.finish(tick, ending)?;
    }
    result
}

fn run_pass<S: Simulation>(
    opts: &RunOptions,
    start: &Start<S::Config>,
    schedule: &[ScheduleEntry<S::Command>],
    mut diagnostics: Option<&mut dyn Write>,
    pass: Pass<'_>,
    replay_writer: &mut Option<sim_io::replay::ReplayWriter>,
) -> Result<CompletedRun, RunnerError> {
    let wall_start = Instant::now();
    let loading = Instant::now();
    let mut sim = start.instantiate::<S>(opts)?;
    if let Some(playback) = pass.playback {
        playback.check_start(&sim, opts.artifacts.force)?;
    }
    if diagnostics.is_some()
        && let Some(path) = &opts.artifacts.record_replay
    {
        let header = container::Header::capture(&opts.game, start.seed(opts.seed), &sim);
        *replay_writer = Some(replay::create_writer::<S>(
            path,
            opts,
            start,
            pass.config,
            header,
        )?);
    }
    let mut snapshots = Vec::new();
    if let Start::Snapshot(data, file_time) = start
        && diagnostics.is_some()
        && let Some(path) = &opts.artifacts.load_snapshot
    {
        snapshots.push(SnapshotReport::new(
            path,
            sim.tick(),
            "load",
            data.archive_len,
            *file_time + loading.elapsed(),
        ));
    }
    let mut latencies = reserve(opts.ticks.saturating_sub(sim.tick()), "step latencies")?;
    let checkpoint_capacity = opts
        .ticks
        .checked_div(opts.hash_interval)
        .unwrap_or(0)
        .saturating_add(1);
    let mut checkpoints = reserve(checkpoint_capacity, "hash checkpoints")?;
    let mut tick = sim.tick();
    let skip = schedule.partition_point(|entry| entry.tick < tick);
    if let Some(writer) = diagnostics.as_deref_mut() {
        for entry in &schedule[..skip] {
            writeln!(
                writer,
                "Warnung: Commands bei Tick {} liegen vor dem Snapshot-Tick {tick}",
                entry.tick
            )
            .map_err(|source| RunnerError::Io {
                context: "stderr".into(),
                source,
            })?;
        }
        save_if_requested(opts, start.seed(opts.seed), &sim, &mut snapshots)?;
    }
    let mut remaining = &schedule[skip..];
    let report_interval = opts.report_interval.unwrap_or(0);

    let telemetry = &opts.telemetry;
    let mut trace = telemetry
        .trace_out
        .as_deref()
        .filter(|_| diagnostics.is_some())
        .map(|path| Trace::new(path, &telemetry.trace_ids))
        .transpose()?;
    if let Some(trace) = &mut trace {
        trace.observe(&sim)?;
    }
    let mut reloaded = 0;
    if pass.roundtrips.binary_search(&tick).is_ok() {
        sim = reload(opts, sim)?;
        reloaded += 1;
    }
    if diagnostics.is_some() {
        dump_state_if_requested(opts, &sim)?;
    }
    let initial_tick = tick;
    let mut recorder = Recorder::new();
    let profile = telemetry.profile_phases && (diagnostics.is_some() || pass.timing);
    let mut tick_phases = (pass.timing && profile).then(TickPhases::default);
    let mut noop = NoopProbe;
    let mut phases = Vec::new();
    let mut worst = WorstTicks::new(if diagnostics.is_some() {
        telemetry
            .worst_ticks
            .min(opts.ticks - initial_tick.min(opts.ticks))
    } else {
        0
    })?;
    let mut step_allocations_total = 0;
    let mut step_allocations_max = 0;

    while tick < opts.ticks && !sim.is_finished() {
        let commands = match remaining.split_first() {
            Some((entry, rest)) if entry.tick == tick => {
                remaining = rest;
                entry.commands.as_slice()
            }
            _ => &[],
        };
        if let Some(writer) = replay_writer.as_mut() {
            writer.commands(tick, commands)?;
        }
        let mut elapsed = 0;
        let mut panicked = None;
        let probe: &mut dyn PhaseProbe = if profile { &mut recorder } else { &mut noop };
        let mut step = || {
            let start = Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sim.step_profiled(commands, probe);
            }));
            elapsed = nanoseconds(start.elapsed());
            panicked = result.err();
        };
        let allocations = if let Some(count) = telemetry.allocation_counter {
            count(&mut step)
        } else {
            step();
            0
        };
        if let Some(payload) = panicked {
            return Err(RunnerError::SimulationPanic {
                tick,
                message: panic_message(payload.as_ref()),
            });
        }
        latencies.push(elapsed);
        step_allocations_total += u128::from(allocations);
        step_allocations_max = step_allocations_max.max(allocations);
        worst.record(sim.tick(), elapsed, &recorder);
        if let Some(tick_phases) = &mut tick_phases {
            tick_phases.record(&recorder);
        }
        if profile {
            recorder.flush(&mut phases, opts.ticks)?;
        }

        let next = sim.tick();
        if next != tick + 1 {
            return Err(RunnerError::ContractViolation(format!(
                "step must advance tick from {tick} to {}, got {next}",
                tick + 1
            )));
        }
        tick = next;
        if pass.roundtrips.binary_search(&tick).is_ok() {
            sim = reload(opts, sim)?;
            reloaded += 1;
        }

        if diagnostics.is_some() {
            save_if_requested(opts, start.seed(opts.seed), &sim, &mut snapshots)?;
            dump_state_if_requested(opts, &sim)?;
        }
        if let Some(limit) = telemetry.max_step_allocs
            && tick - initial_tick > telemetry.alloc_warmup
            && allocations > limit
        {
            return Err(RunnerError::AllocationLimit {
                tick,
                count: allocations,
                limit,
            });
        }
        if let Some(trace) = &mut trace
            && due(tick, telemetry.trace_every)
        {
            trace.observe(&sim)?;
        }
        if due(tick, opts.check_invariants_every) {
            check_invariants(&sim, tick)?;
        }

        let checkpoint_due = due(tick, opts.hash_interval);
        let report_due = diagnostics.is_some() && due(tick, report_interval);
        if checkpoint_due || report_due {
            let hash = sim.state_hash();
            let metrics = sim.metrics();
            if checkpoint_due {
                let point = checkpoint(
                    &sim,
                    tick,
                    hash,
                    metrics.active_entities,
                    pass.retain_snapshots,
                )?;
                if let Some(writer) = replay_writer.as_mut() {
                    writer.checkpoint(point.tick, point.hash, &point.subsystems)?;
                }
                checkpoints.push(point);
            }
            if let Some(writer) = diagnostics.as_deref_mut().filter(|_| report_due) {
                write_progress(writer, tick, hash, &metrics)?;
            }
        }
        if let Some(writer) = replay_writer.as_mut() {
            writer.tick_done(tick)?;
        }
    }

    check_invariants(&sim, tick)?;
    let hash = sim.state_hash();
    let metrics = sim.metrics();
    if checkpoints.last().is_none_or(|last| last.tick != tick) {
        let point = checkpoint(
            &sim,
            tick,
            hash,
            metrics.active_entities,
            pass.retain_snapshots,
        )?;
        if let Some(writer) = replay_writer.as_mut() {
            writer.checkpoint(point.tick, point.hash, &point.subsystems)?;
        }
        checkpoints.push(point);
    }
    if let Some(trace) = &mut trace {
        if tick != initial_tick && !due(tick, telemetry.trace_every) {
            trace.observe(&sim)?;
        }
        trace.finish()?;
    }
    Ok(CompletedRun {
        tick,
        hash,
        checkpoints,
        latencies,
        wall_time: wall_start.elapsed(),
        metrics,
        phases,
        worst_ticks: worst.report(),
        snapshots,
        roundtrips: reloaded,
        dimensions: sim.phase_dimensions(),
        step_allocations_total,
        step_allocations_max,
        tick_phases,
    })
}

fn roundtrip_ticks(opts: &RunOptions, start: u64) -> Result<Vec<u64>, RunnerError> {
    let artifacts = &opts.artifacts;
    if !artifacts.verify_save_load {
        if !artifacts.roundtrip_at.is_empty() {
            return Err(RunnerError::InvalidOptions(
                "--roundtrip-at requires --verify-save-load".into(),
            ));
        }
        return Ok(Vec::new());
    }
    let mut ticks = artifacts.roundtrip_at.clone();
    if ticks.is_empty() {
        let elapsed = opts.ticks - start;
        ticks = vec![
            start + elapsed / 3,
            start + (u128::from(elapsed) * 2 / 3) as u64,
        ];
    }
    if ticks
        .iter()
        .any(|&tick| !(start..=opts.ticks).contains(&tick))
    {
        return Err(RunnerError::InvalidOptions(
            "roundtrip tick lies outside the run".into(),
        ));
    }
    ticks.sort_unstable();
    ticks.dedup();
    Ok(ticks)
}

fn reload<S: Simulation>(opts: &RunOptions, sim: S) -> Result<S, RunnerError> {
    let tick = sim.tick();
    let hash = sim.state_hash();
    let before = sim.subsystem_hashes();
    let mut bytes = Vec::new();
    sim.save_snapshot(&mut bytes)
        .map_err(|err| RunnerError::SaveLoadMismatch {
            tick,
            message: format!("could not save: {err}"),
        })?;
    let dump_before = divergence::dump(&sim);
    drop(sim);
    let loaded = S::load_snapshot(&bytes).map_err(|err| RunnerError::SaveLoadMismatch {
        tick,
        message: format!("could not load: {err}"),
    })?;
    let actual = loaded.state_hash();
    let subsystems = container::differing(before, loaded.subsystem_hashes());
    if hash != actual || loaded.tick() != tick || !subsystems.is_empty() {
        let details = divergence::details(
            &opts.artifacts.diff_dir,
            tick,
            dump_before,
            divergence::dump(&loaded),
        );
        return Err(RunnerError::SaveLoadMismatch {
            tick,
            message: format!(
                "hash differs immediately after loading ({hash:#018x} vs {actual:#018x}, loaded tick {}); differing subsystems: {}{details}",
                loaded.tick(),
                subsystems.join(", ")
            ),
        });
    }
    Ok(loaded)
}

fn dump_state_if_requested<S: Simulation>(opts: &RunOptions, sim: &S) -> Result<(), RunnerError> {
    if opts.artifacts.dump_state_at == Some(sim.tick()) {
        let dir = opts.artifacts.diff_dir.as_path();
        std::fs::create_dir_all(dir).map_err(|err| io_at(dir, err))?;
        let path = dir.join(format!("replay-t{}.txt", sim.tick()));
        let mut dump = String::new();
        sim.debug_dump(&mut dump)?;
        container::write(&path, dump.as_bytes())?;
    }
    Ok(())
}

fn save_if_requested<S: Simulation>(
    opts: &RunOptions,
    seed: u64,
    sim: &S,
    reports: &mut Vec<SnapshotReport>,
) -> Result<(), RunnerError> {
    let tick = sim.tick();
    if opts.artifacts.save_snapshot_at.contains(&tick)
        && let Some(dir) = &opts.artifacts.snapshot_dir
    {
        let begin = Instant::now();
        let bytes = container::snapshot_bytes(&opts.game, seed, sim)?;
        std::fs::create_dir_all(dir).map_err(|err| io_at(dir, err))?;
        let path = dir.join(format!("{}-seed{seed}-t{tick}.snap", opts.game));
        container::write(&path, &bytes)?;
        reports.push(SnapshotReport::new(
            &path,
            tick,
            "save",
            bytes.len() as u64,
            begin.elapsed(),
        ));
    }
    Ok(())
}

fn checkpoint<S: Simulation>(
    sim: &S,
    tick: u64,
    hash: u64,
    active: usize,
    retain_snapshot: bool,
) -> Result<Checkpoint, RunnerError> {
    let snapshot = if retain_snapshot {
        let mut bytes = Vec::new();
        sim.save_snapshot(&mut bytes)?;
        Some(bytes)
    } else {
        None
    };
    let mut subsystems = sim.subsystem_hashes();
    subsystems.sort_unstable_by_key(|&(name, value)| (name, value));
    Ok(Checkpoint {
        tick,
        hash,
        active,
        units: sim.scale_units(),
        snapshot,
        subsystems,
    })
}

pub(crate) fn nanoseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn due(tick: u64, interval: u64) -> bool {
    interval != 0 && tick.is_multiple_of(interval)
}

fn reserve<T>(capacity: u64, label: &str) -> Result<Vec<T>, RunnerError> {
    let mut values = Vec::new();
    let failure = |error: &dyn Error| {
        RunnerError::ContractViolation(format!("cannot reserve {capacity} {label}: {error}"))
    };
    let entries = usize::try_from(capacity).map_err(|error| failure(&error))?;
    values
        .try_reserve_exact(entries)
        .map_err(|error| failure(&error))?;
    Ok(values)
}

fn check_invariants<S: Simulation>(sim: &S, tick: u64) -> Result<(), RunnerError> {
    sim.check_invariants()
        .map_err(|error| RunnerError::InvariantViolation {
            tick,
            message: match error {
                SimError::InvariantViolation(message) => message,
                other => other.to_string(),
            },
        })
}

#[cfg(test)]
fn verify_checkpoints(first: &[Checkpoint], second: &[Checkpoint]) -> Result<(), RunnerError> {
    match divergence::compare(first, second) {
        Some(difference) => Err(difference.error()),
        None => Ok(()),
    }
}

fn write_progress(
    writer: &mut dyn Write,
    tick: u64,
    hash: u64,
    metrics: &SimMetrics,
) -> Result<(), RunnerError> {
    let result = (|| {
        write!(
            writer,
            "Tick {tick} | active_entities={} | state_hash={hash:#018x}",
            metrics.active_entities
        )?;
        for (name, value) in &metrics.custom_gauges {
            write!(writer, " | {name}={value:.2}")?;
        }
        writeln!(writer)
    })();
    result.map_err(|source| RunnerError::Io {
        context: "stderr".into(),
        source,
    })
}

fn dump_hashes(path: &Path, checkpoints: &[Checkpoint]) -> Result<(), RunnerError> {
    let result = (|| {
        let mut writer = BufWriter::new(File::create(path)?);
        writeln!(writer, "tick,hash_hex")?;
        for checkpoint in checkpoints {
            writeln!(writer, "{},{:#018x}", checkpoint.tick, checkpoint.hash)?;
        }
        writer.flush()
    })();
    result.map_err(|source| RunnerError::Io {
        context: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests;
