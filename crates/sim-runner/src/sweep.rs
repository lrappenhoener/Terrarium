use std::{
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
    keys: Vec<&'a str>,
    scaling_warnings: Vec<String>,
}

pub fn execute(
    game: &GameEntry,
    opts: &RunOptions,
    texts: &[String],
    factor: f64,
) -> Result<(), RunnerError> {
    if !factor.is_finite() || factor < 1.0 {
        return Err(RunnerError::InvalidOptions(
            "sweep warning factor must be finite and at least 1".into(),
        ));
    }
    let columns = texts
        .iter()
        .map(|text| parse_sweep(text))
        .collect::<Result<Vec<_>, _>>()?;
    let count = columns.first().map_or(0, |(_, values)| values.len());
    if columns.iter().any(|(_, values)| values.len() != count) {
        return Err(RunnerError::InvalidOptions(
            "sweep lists must have equal lengths".into(),
        ));
    }
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let mut run = opts.clone();
        let values = columns
            .iter()
            .map(|(key, values)| {
                run.overrides.push(format!("{key}={}", values[i]));
                values[i].to_string()
            })
            .collect::<Vec<_>>();
        run.dump_hashes = output_path(run.dump_hashes.as_deref(), i + 1);
        run.telemetry.trace_out = output_path(run.telemetry.trace_out.as_deref(), i + 1);
        rows.push(Row {
            value: values.join(", "),
            report: (game.run)(&run)?,
        });
    }
    let mut scaling_warnings = Vec::new();
    for pair in rows.windows(2) {
        let [a, b] = pair else { continue };
        for (phase, first, next) in std::iter::once((
            "step",
            a.report.ns_per_active_entity_tick,
            b.report.ns_per_active_entity_tick,
        ))
        .chain(a.report.phases.iter().filter_map(|phase| {
            let other = b
                .report
                .phases
                .iter()
                .find(|p| p.name == phase.name && p.dimension == phase.dimension)?;
            Some((phase.name, phase.ns_per_unit_tick, other.ns_per_unit_tick))
        })) {
            if let Some(message) = warning(phase, &a.value, &b.value, first, next, factor) {
                scaling_warnings.push(message);
            }
        }
    }
    let keys: Vec<_> = columns.iter().map(|(name, _)| *name).collect();
    let title = keys.join(", ");
    let report = SweepReport {
        key: &title,
        keys,
        rows,
        scaling_warnings,
    };
    write_output(&mut io::stdout().lock(), opts.report_format, &report).map_err(|source| {
        RunnerError::Io {
            context: "stdout".into(),
            source,
        }
    })
}

fn warning(
    phase: &str,
    a: &str,
    b: &str,
    first: Option<f64>,
    next: Option<f64>,
    factor: f64,
) -> Option<String> {
    let (first, next) = first.zip(next)?;
    (next > first * factor)
        .then(|| format!("{phase} ({a} → {b}): {first:.2} → {next:.2} ns/Einheit/Tick"))
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
            for warning in &sweep.scaling_warnings {
                writeln!(out, "Hinweis: {warning}")?;
            }
            for row in &sweep.rows {
                if !row.report.phases.is_empty() {
                    writeln!(out, "Phasen für {}={}", sweep.key, row.value)?;
                    for phase in &row.report.phases {
                        writeln!(
                            out,
                            "  {} | p50 {:.2} µs | p99 {:.2} µs | Max {:.2} µs | {:.2} % | {} ns/{}/Tick",
                            phase.name,
                            phase.p50_us,
                            phase.p99_us,
                            phase.max_us,
                            phase.share_percent,
                            metric(phase.ns_per_unit_tick),
                            phase.dimension,
                        )?;
                    }
                }
            }
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_only_increases_above_the_threshold() {
        assert!(warning("decide", "a", "b", Some(4.0), Some(5.0), 1.25).is_none());
        assert!(
            warning("decide", "b", "c", Some(5.0), Some(7.0), 1.25)
                .unwrap()
                .contains("decide (b → c)")
        );
    }
}
