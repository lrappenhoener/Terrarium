use std::fmt::{self, Display};

use serde::Serialize;

use crate::{PhaseReport, RepeatReport, WorstTick};

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub tick: u64,
    pub ticks_executed: u64,
    pub ended_early: bool,
    pub step_total_seconds: f64,
    pub wall_total_seconds: f64,
    pub average_tps: Option<f64>,
    pub latency_us: Option<LatencyStats>,
    pub active_entities: usize,
    pub custom_gauges: Vec<(String, f64)>,
    pub state_hash: String,
    pub verified_checkpoints: Option<usize>,
    pub verified_save_load: Option<usize>,
    pub replay_verified: bool,
    pub replay_status: Option<String>,
    pub average_active_entities: f64,
    pub ns_per_active_entity_tick: Option<f64>,
    pub average_scale_units: std::collections::BTreeMap<&'static str, f64>,
    pub phases: Vec<PhaseReport>,
    pub worst_ticks: Vec<WorstTick>,
    pub snapshots: Vec<SnapshotReport>,
    pub step_allocations_total: Option<u128>,
    pub step_allocations_max: Option<u64>,
    pub repeat: Option<RepeatReport>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotReport {
    pub path: std::path::PathBuf,
    pub tick: u64,
    pub operation: &'static str,
    pub bytes: u64,
    pub duration_ms: f64,
}

impl SnapshotReport {
    pub(crate) fn new(
        path: &std::path::Path,
        tick: u64,
        operation: &'static str,
        bytes: u64,
        elapsed: std::time::Duration,
    ) -> Self {
        Self {
            path: path.into(),
            tick,
            operation,
            bytes,
            duration_ms: elapsed.as_secs_f64() * 1000.0,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LatencyStats {
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl LatencyStats {
    pub(crate) fn from_samples(samples: &mut [u64]) -> Option<Self> {
        samples.sort_unstable();
        let microseconds = |ns: u64| (ns as f64 / 10.0).round() / 100.0;
        Some(Self {
            min: microseconds(*samples.first()?),
            p50: microseconds(nearest_rank(samples, 50)?),
            p95: microseconds(nearest_rank(samples, 95)?),
            p99: microseconds(nearest_rank(samples, 99)?),
            max: microseconds(*samples.last()?),
        })
    }
}

pub(crate) fn nearest_rank(sorted: &[u64], percentile: u32) -> Option<u64> {
    let rank = (sorted.len() as u128 * u128::from(percentile)).div_ceil(100) as usize;
    sorted.get(rank.checked_sub(1)?).copied()
}

impl Display for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Tick {} ({} ausgeführt)", self.tick, self.ticks_executed)?;
        if self.ended_early {
            writeln!(f, "Simulation bei Tick {} vorzeitig beendet", self.tick)?;
        }
        writeln!(f, "Step-Gesamtzeit {:.6} s", self.step_total_seconds)?;
        writeln!(f, "Wall-Clock-Gesamtzeit {:.6} s", self.wall_total_seconds)?;
        match self.average_tps {
            Some(tps) => writeln!(f, "Durchschnittliche TPS {tps:.2}")?,
            None => writeln!(f, "Durchschnittliche TPS n/a")?,
        }
        if let Some(latency) = &self.latency_us {
            writeln!(
                f,
                "Tick-Latenz (µs) | Min {:.2} | p50 {:.2} | p95 {:.2} | p99 {:.2} | Max {:.2}",
                latency.min, latency.p50, latency.p95, latency.p99, latency.max,
            )?;
        } else {
            writeln!(f, "Tick-Latenz (µs) n/a")?;
        }
        writeln!(f, "active_entities {}", self.active_entities)?;
        writeln!(
            f,
            "Durchschnittlich aktive Entitäten {:.2}",
            self.average_active_entities
        )?;
        match self.ns_per_active_entity_tick {
            Some(value) => writeln!(f, "ns pro aktiver Entität und Tick {value:.2}")?,
            None => writeln!(f, "ns pro aktiver Entität und Tick n/a")?,
        }
        if let Some((total, max)) = self.step_allocations_total.zip(self.step_allocations_max) {
            writeln!(
                f,
                "Step-Allokationen {total} gesamt, {max} maximal pro Tick"
            )?;
        } else {
            writeln!(f, "Step-Allokationen n/a")?;
        }
        for phase in &self.phases {
            writeln!(
                f,
                "Phase {} (µs) | p50 {:.2} | p99 {:.2} | Max {:.2} | Anteil {:.2} % | {} ns/{}/Tick",
                phase.name,
                phase.p50_us,
                phase.p99_us,
                phase.max_us,
                phase.share_percent,
                phase
                    .ns_per_unit_tick
                    .map_or_else(|| "n/a".into(), |value| format!("{value:.2}")),
                phase.dimension,
            )?;
        }
        for snapshot in &self.snapshots {
            writeln!(
                f,
                "Snapshot {} bei Tick {} | {} Bytes | {:.3} ms | {}",
                snapshot.operation,
                snapshot.tick,
                snapshot.bytes,
                snapshot.duration_ms,
                snapshot.path.display()
            )?;
        }
        for tick in &self.worst_ticks {
            write!(
                f,
                "Langsamer Tick {} | {:.2} µs",
                tick.tick, tick.duration_us
            )?;
            for (name, us) in &tick.phases_us {
                write!(f, " | {name} {:.2} µs", us)?;
            }
            writeln!(f)?;
        }
        if let Some(repeat) = &self.repeat {
            writeln!(
                f,
                "Wiederholung: {} Läufe | Median-Tick {:?} µs | p99 {:?} µs | Ausreißer ab Faktor {}",
                repeat.runs, repeat.median_tick_us, repeat.p99_us, repeat.outlier_factor
            )?;
            writeln!(
                f,
                "Reproduzierbare Ausreißer: {} | einmalige Ausreißer (vermutlich Rauschen): {}",
                repeat.reproducible_total, repeat.one_off_total
            )?;
            for (label, list) in [
                ("Reproduzierbar", &repeat.reproducible),
                ("Einmalig, vermutlich Rauschen", &repeat.one_off),
            ] {
                for outlier in list {
                    write!(
                        f,
                        "{label}: Tick {} | Median {:.2} µs | Max {:.2} µs | in {}/{} Läufen",
                        outlier.tick,
                        outlier.median_us,
                        outlier.max_us,
                        outlier.runs_over,
                        repeat.runs
                    )?;
                    for (name, us) in &outlier.phases_us {
                        write!(f, " | {name} {us:.2} µs")?;
                    }
                    writeln!(f)?;
                }
            }
        }
        for (name, value) in &self.custom_gauges {
            writeln!(f, "{name} {value:.2}")?;
        }
        writeln!(f, "state_hash {}", self.state_hash)?;
        if let Some(count) = self.verified_checkpoints {
            writeln!(f, "Determinismus OK ({count} Checkpoints)")?;
        }
        if self.replay_verified {
            writeln!(f, "Replay OK")?;
        }
        if let Some(status) = &self.replay_status {
            writeln!(f, "Replay-Status: {status}")?;
        }
        if let Some(count) = self.verified_save_load {
            writeln!(f, "Save/Load OK ({count} Roundtrips)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank() {
        assert_eq!(nearest_rank(&[], 50), None);
        assert_eq!(nearest_rank(&[52], 50), Some(52));
        assert_eq!(nearest_rank(&[52], 99), Some(52));
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(nearest_rank(&values, 50), Some(50));
        assert_eq!(nearest_rank(&values, 95), Some(95));
        assert_eq!(nearest_rank(&values, 99), Some(99));
        assert_eq!(nearest_rank(&[1, 2, 3], 50), Some(2));
    }

    #[test]
    fn latency_samples_are_sorted_and_rounded_to_hundredths_of_a_microsecond() {
        assert!(LatencyStats::from_samples(&mut []).is_none());
        let stats = LatencyStats::from_samples(&mut [3334, 1205]).unwrap();
        assert_eq!(stats.min, 1.21);
        assert_eq!(stats.p50, 1.21);
        assert_eq!(stats.p95, 3.33);
        assert_eq!(stats.p99, 3.33);
        assert_eq!(stats.max, 3.33);
    }
}
