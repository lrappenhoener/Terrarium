mod games;
mod sweep;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::Parser;
use sim_host::SimError;
use sim_runner::{ReportFormat, RunOptions, RunnerError, TelemetryOptions};

#[derive(Parser)]
#[command(version, about = "Run and verify deterministic headless simulations")]
struct Cli {
    #[arg(long, default_value = games::DEFAULT_GAME)]
    game: String,
    #[arg(long)]
    list_games: bool,
    #[arg(long, default_value_t = 1_000)]
    ticks: u64,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    commands: Option<PathBuf>,
    #[arg(long)]
    verify_determinism: bool,
    #[arg(
        long,
        default_value_t = 100,
        help = "Ticks between hash checkpoints (0: final only)"
    )]
    hash_interval: u64,
    #[arg(
        long,
        default_value_t = 100,
        help = "Ticks between invariant checks (0: final only)"
    )]
    check_invariants_every: u64,
    #[arg(long, help = "Ticks between reports on stderr (0: disabled)")]
    report_interval: Option<u64>,
    #[arg(long, value_name = "PATH")]
    dump_hashes: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "text")]
    report_format: ReportFormat,
    #[arg(long = "set", value_name = "KEY=VALUE")]
    overrides: Vec<String>,
    #[arg(long, value_name = "KEY=V1,V2,...")]
    sweep: Vec<String>,
    #[arg(long, default_value_t = 1.25)]
    sweep_warn_factor: f64,
    #[arg(long)]
    budget_p99_us: Option<f64>,
    #[arg(long)]
    profile_phases: bool,
    #[arg(long, default_value_t = 0)]
    worst_ticks: u64,
    #[arg(long)]
    max_step_allocs: Option<u64>,
    #[arg(long, default_value_t = 10)]
    alloc_warmup: u64,
    #[arg(long = "trace-entity", value_name = "ID")]
    trace_ids: Vec<u64>,
    #[arg(long, default_value_t = 1)]
    trace_every: u64,
    #[arg(long)]
    trace_out: Option<PathBuf>,
}

impl From<Cli> for RunOptions {
    fn from(cli: Cli) -> Self {
        Self {
            game: cli.game,
            list_games: cli.list_games,
            ticks: cli.ticks,
            seed: cli.seed,
            config: cli.config,
            commands: cli.commands,
            verify_determinism: cli.verify_determinism,
            hash_interval: cli.hash_interval,
            check_invariants_every: cli.check_invariants_every,
            report_interval: cli.report_interval,
            dump_hashes: cli.dump_hashes,
            report_format: cli.report_format,
            overrides: cli.overrides,
            budget_p99_us: cli.budget_p99_us,
            telemetry: TelemetryOptions {
                profile_phases: cli.profile_phases,
                worst_ticks: cli.worst_ticks,
                allocation_counter: Some(count_step_allocations),
                max_step_allocs: cli.max_step_allocs,
                alloc_warmup: cli.alloc_warmup,
                trace_ids: cli.trace_ids,
                trace_every: cli.trace_every,
                trace_out: cli.trace_out,
            },
        }
    }
}

// Linking allocation-counter here installs its global allocator for the CLI.
fn count_step_allocations(step: &mut dyn FnMut()) -> u64 {
    allocation_counter::measure(step).count_total
}

fn main() -> ExitCode {
    let mut cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    let sweep = std::mem::take(&mut cli.sweep);
    let warn_factor = cli.sweep_warn_factor;
    match execute(&cli.into(), &sweep, warn_factor) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "Fehler: {error}");
            exit_code(&error)
        }
    }
}

fn exit_code(error: &RunnerError) -> ExitCode {
    match error {
        RunnerError::DeterminismMismatch { .. } => ExitCode::from(2),
        RunnerError::InvariantViolation { .. }
        | RunnerError::Sim(SimError::InvariantViolation(_)) => ExitCode::from(3),
        RunnerError::LatencyBudget { .. } => ExitCode::from(5),
        RunnerError::AllocationLimit { .. } => ExitCode::from(4),
        _ => ExitCode::FAILURE,
    }
}

fn execute(opts: &RunOptions, sweep: &[String], warn_factor: f64) -> Result<(), RunnerError> {
    let output_error = |source| RunnerError::Io {
        context: "stdout".into(),
        source,
    };
    if opts.list_games {
        let mut output = io::stdout().lock();
        for game in games::GAMES {
            writeln!(output, "{}", game.name).map_err(output_error)?;
        }
        return output.flush().map_err(output_error);
    }

    let game = games::GAMES
        .iter()
        .find(|entry| entry.name == opts.game)
        .ok_or_else(|| RunnerError::UnknownGame(opts.game.clone()))?;
    if !sweep.is_empty() {
        return sweep::execute(game, opts, sweep, warn_factor);
    }
    let report = (game.run)(opts)?;
    let mut output = io::stdout().lock();
    match opts.report_format {
        ReportFormat::Text => write!(output, "{report}").map_err(output_error)?,
        ReportFormat::Json => {
            serde_json::to_writer_pretty(&mut output, &report)
                .map_err(|source| output_error(source.into()))?;
            writeln!(output).map_err(output_error)?;
        }
    }
    output.flush().map_err(output_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocation_limit_has_its_own_exit_code() {
        assert_eq!(
            exit_code(&RunnerError::AllocationLimit {
                tick: 11,
                count: 1,
                limit: 0
            }),
            ExitCode::from(4)
        );
    }
}
