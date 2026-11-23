//! Game-independent execution, timing, and determinism verification.

mod input;
mod report;

use std::{
    error::Error,
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sim_host::{SimError, SimMetrics, Simulation};
use thiserror::Error;

use input::{ScheduleEntry, load_schedule, read_input};
pub use report::{LatencyStats, RunReport};

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
    #[error("determinism mismatch at tick {tick}: {first:#018x} vs {second:#018x}")]
    DeterminismMismatch { tick: u64, first: u64, second: u64 },
    #[error("simulation contract violation: {0}")]
    ContractViolation(String),
    #[error("unknown game: {0}")]
    UnknownGame(String),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Checkpoint {
    tick: u64,
    hash: u64,
}

struct CompletedRun {
    final_checkpoint: Checkpoint,
    checkpoints: Vec<Checkpoint>,
    latencies: Vec<u64>,
    wall_time: Duration,
    metrics: SimMetrics,
}

pub fn run_simulation<S: Simulation>(opts: &RunOptions) -> Result<RunReport, RunnerError> {
    let config = match &opts.config {
        Some(path) => read_input::<S::Config>(path)?,
        None => S::Config::default(),
    };
    let mut diagnostics = io::stderr().lock();
    let schedule = match &opts.commands {
        Some(path) => load_schedule::<S::Command>(path, opts.ticks, &mut diagnostics)?,
        None => Vec::new(),
    };
    let mut first = run_once::<S>(opts, config.clone(), &schedule, Some(&mut diagnostics))?;
    drop(diagnostics);

    if let Some(path) = &opts.dump_hashes {
        dump_hashes(path, &first.checkpoints)?;
    }
    let step_total_seconds = first
        .latencies
        .iter()
        .map(|&ns| u128::from(ns))
        .sum::<u128>() as f64
        / 1_000_000_000.0;
    let ticks_executed = first.latencies.len() as u64;
    let latency_us = LatencyStats::from_samples(&mut first.latencies);
    drop(first.latencies);

    let verified_checkpoints = if opts.verify_determinism {
        let second = run_once::<S>(opts, config, &schedule, None)?;
        verify_checkpoints(&first.checkpoints, &second.checkpoints)?;
        Some(first.checkpoints.len())
    } else {
        None
    };
    let tick = first.final_checkpoint.tick;
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
        state_hash: format!("{:#018x}", first.final_checkpoint.hash),
        verified_checkpoints,
    })
}

fn run_once<S: Simulation>(
    opts: &RunOptions,
    config: S::Config,
    schedule: &[ScheduleEntry<S::Command>],
    mut diagnostics: Option<&mut dyn Write>,
) -> Result<CompletedRun, RunnerError> {
    let wall_start = Instant::now();
    let mut sim = S::new(opts.seed, config)?;
    let mut latencies = reserve(opts.ticks, "step latencies")?;
    let checkpoint_capacity = opts
        .ticks
        .checked_div(opts.hash_interval)
        .unwrap_or(0)
        .saturating_add(1);
    let mut checkpoints = reserve(checkpoint_capacity, "hash checkpoints")?;
    let mut tick = sim.tick();
    let mut remaining = &schedule[schedule.partition_point(|entry| entry.tick < tick)..];
    let report_interval = opts.report_interval.unwrap_or(0);

    while tick < opts.ticks && !sim.is_finished() {
        let commands = match remaining.split_first() {
            Some((entry, rest)) if entry.tick == tick => {
                remaining = rest;
                entry.commands.as_slice()
            }
            _ => &[],
        };
        let start = Instant::now();
        sim.step(commands);
        let elapsed = start.elapsed();
        latencies.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));

        let next = sim.tick();
        if next != tick + 1 {
            return Err(RunnerError::ContractViolation(format!(
                "step must advance tick from {tick} to {}, got {next}",
                tick + 1
            )));
        }
        tick = next;
        if due(tick, opts.check_invariants_every) {
            check_invariants(&sim, tick)?;
        }

        let checkpoint_due = due(tick, opts.hash_interval);
        let report_due = diagnostics.is_some() && due(tick, report_interval);
        if checkpoint_due || report_due {
            let hash = sim.state_hash();
            if checkpoint_due {
                checkpoints.push(Checkpoint { tick, hash });
            }
            if let Some(writer) = diagnostics.as_deref_mut().filter(|_| report_due) {
                write_progress(writer, tick, hash, &sim.metrics())?;
            }
        }
    }

    check_invariants(&sim, tick)?;
    let final_checkpoint = Checkpoint {
        tick,
        hash: sim.state_hash(),
    };
    match checkpoints.last_mut() {
        Some(last) if last.tick == tick => *last = final_checkpoint,
        _ => checkpoints.push(final_checkpoint),
    }
    let metrics = sim.metrics();
    Ok(CompletedRun {
        final_checkpoint,
        checkpoints,
        latencies,
        wall_time: wall_start.elapsed(),
        metrics,
    })
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

fn verify_checkpoints(first: &[Checkpoint], second: &[Checkpoint]) -> Result<(), RunnerError> {
    let different_pair = first.iter().zip(second).find(|(a, b)| a != b);
    let different_lengths = || {
        if first.len() != second.len() {
            first.last().zip(second.last())
        } else {
            None
        }
    };
    if let Some((a, b)) = different_pair.or_else(different_lengths) {
        return Err(RunnerError::DeterminismMismatch {
            tick: a.tick.min(b.tick),
            first: a.hash,
            second: b.hash,
        });
    }
    Ok(())
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
