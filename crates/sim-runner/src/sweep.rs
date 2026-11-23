use std::{
    cmp::Ordering,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sim_runner::{ReportFormat, RunOptions, RunReport, RunnerError, parse_sweep};

use crate::games::GameEntry;

#[derive(Serialize)]
struct Row {
    value: String,
    #[serde(flatten)]
    report: RunReport,
}

#[derive(Serialize)]
struct SweepReport<'a> {
    key: &'a str,
    rows: Vec<Row>,
    scaling_warning: Option<String>,
}

pub fn execute(game: &GameEntry, opts: &RunOptions, text: &str) -> Result<(), RunnerError> {
    let (key, values) = parse_sweep(text)?;
    let mut rows = Vec::with_capacity(values.len());
    for (i, value) in values.iter().enumerate() {
        let mut run = opts.clone();
        run.overrides.push(format!("{key}={value}"));
        run.dump_hashes = output_path(run.dump_hashes.as_deref(), i + 1);
        run.telemetry.trace_out = output_path(run.telemetry.trace_out.as_deref(), i + 1);
        rows.push(Row {
            value: value.to_string(),
            report: (game.run)(&run)?,
        });
    }

    let compare = |a: &usize, b: &usize| compare_values(&values[*a], &values[*b]);
    let min = (0..values.len()).min_by(compare).unwrap_or(0);
    let max = (0..values.len()).max_by(compare).unwrap_or(0);
    let scaling_warning = rows[min]
        .report
        .ns_per_active_entity_tick
        .zip(rows[max].report.ns_per_active_entity_tick)
        .filter(|(small, large)| small * 2.0 < *large)
        .map(|(small, large)| {
            format!(
                "ns pro aktiver Entität und Tick steigen von {:.2} auf {:.2} zwischen {} und {}",
                small, large, rows[min].value, rows[max].value
            )
        });
    let report = SweepReport {
        key,
        rows,
        scaling_warning,
    };
    write_output(&mut io::stdout().lock(), opts.report_format, &report).map_err(|source| {
        RunnerError::Io {
            context: "stdout".into(),
            source,
        }
    })
}

fn compare_values(a: &toml::Value, b: &toml::Value) -> Ordering {
    match (a, b) {
        (toml::Value::Integer(a), toml::Value::Integer(b)) => a.cmp(b),
        (toml::Value::Float(a), toml::Value::Float(b)) => a.total_cmp(b),
        (toml::Value::Float(a), toml::Value::Integer(b)) => a.total_cmp(&(*b as f64)),
        (toml::Value::Integer(a), toml::Value::Float(b)) => (*a as f64).total_cmp(b),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

fn output_path(path: Option<&Path>, index: usize) -> Option<PathBuf> {
    path.map(|path| {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{index}"));
        name.into()
    })
}

fn write_output(
    out: &mut dyn Write,
    format: ReportFormat,
    sweep: &SweepReport<'_>,
) -> io::Result<()> {
    match format {
        ReportFormat::Json => {
            serde_json::to_writer_pretty(&mut *out, sweep)?;
            writeln!(out)?;
        }
        ReportFormat::Text => {
            writeln!(
                out,
                "{} | Ø aktive Entitäten | TPS | p50 (µs) | p99 (µs) | ns/Entität/Tick",
                sweep.key
            )?;
            let metric = |number: Option<f64>| {
                number.map_or_else(|| "n/a".into(), |value| format!("{value:.2}"))
            };
            for row in &sweep.rows {
                let report = &row.report;
                let timings = report.latency_us.as_ref();
                writeln!(
                    out,
                    "{} | {:.2} | {} | {} | {} | {}",
                    row.value,
                    report.average_active_entities,
                    metric(report.average_tps),
                    metric(timings.map(|t| t.p50)),
                    metric(timings.map(|t| t.p99)),
                    metric(report.ns_per_active_entity_tick)
                )?;
            }
            if let Some(warning) = &sweep.scaling_warning {
                writeln!(out, "Hinweis: {warning}")?;
            }
            for row in &sweep.rows {
                if !row.report.phases.is_empty() {
                    writeln!(out, "Phasen für {}={}", sweep.key, row.value)?;
                    for phase in &row.report.phases {
                        writeln!(
                            out,
                            "  {} | p50 {:.2} µs | p99 {:.2} µs | Max {:.2} µs | {:.2} %",
                            phase.name,
                            phase.p50_us,
                            phase.p99_us,
                            phase.max_us,
                            phase.share_percent
                        )?;
                    }
                }
            }
        }
    }
    out.flush()
}
