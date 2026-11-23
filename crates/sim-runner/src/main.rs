mod games;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::Parser;
use sim_host::SimError;
use sim_runner::{ReportFormat, RunOptions, RunnerError};

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
        }
    }
}

fn main() -> ExitCode {
    let opts = match Cli::try_parse() {
        Ok(cli) => cli.into(),
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    match execute(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "Fehler: {error}");
            match error {
                RunnerError::DeterminismMismatch { .. } => ExitCode::from(2),
                RunnerError::InvariantViolation { .. }
                | RunnerError::Sim(SimError::InvariantViolation(_)) => ExitCode::from(3),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn execute(opts: &RunOptions) -> Result<(), RunnerError> {
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
