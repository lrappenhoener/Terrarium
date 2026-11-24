//! Repeated runs separate reproducible slow ticks from machine noise: a
//! deterministic simulation does the same work in every run of the same tick.

use serde::Serialize;

use crate::{WorstTick, profile::TickPhases, report::nearest_rank};

/// Outliers listed per class; the totals count all of them.
const LISTED: usize = 50;

pub(crate) struct Timing {
    pub latencies: Vec<u64>,
    pub phases: Option<TickPhases>,
}

#[derive(Debug, Serialize)]
pub struct Outlier {
    pub tick: u64,
    /// Runs in which the tick exceeded the factor times that run's median tick.
    pub runs_over: usize,
    pub median_us: f64,
    pub max_us: f64,
    /// Median per phase over the runs, with `--profile-phases`.
    pub phases_us: Vec<(&'static str, f64)>,
}

#[derive(Debug, Serialize)]
pub struct RepeatReport {
    pub runs: usize,
    pub outlier_factor: f64,
    pub median_tick_us: Vec<f64>,
    pub p99_us: Vec<f64>,
    pub reproducible_total: usize,
    pub reproducible: Vec<Outlier>,
    pub one_off_total: usize,
    pub one_off: Vec<Outlier>,
}

fn us(ns: u64) -> f64 {
    ns as f64 / 1000.0
}

fn median(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    nearest_rank(values, 50).unwrap_or(0)
}

fn phases(runs: &[Timing], index: usize) -> Vec<(&'static str, f64)> {
    let Some(first) = runs.first().and_then(|run| run.phases.as_ref()) else {
        return Vec::new();
    };
    let mut names: Vec<&'static str> = first.names().to_vec();
    for run in runs {
        for name in run.phases.as_ref().map_or(&[][..], TickPhases::names) {
            if !names.contains(name) {
                names.push(name);
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let mut values: Vec<u64> = runs
                .iter()
                .map(|run| run.phases.as_ref().map_or(0, |p| p.get(index, name)))
                .collect();
            (name, us(median(&mut values)))
        })
        .collect()
}

/// Classifies ticks over all runs. A tick over `factor` times its run's median
/// tick is reproducible if that holds in at least N−1 runs, otherwise one-off.
/// The worst ticks use the median per tick over the runs.
pub(crate) fn analyze(
    runs: &[Timing],
    first_tick: u64,
    factor: f64,
    worst: usize,
) -> (RepeatReport, Vec<WorstTick>) {
    let n = runs.len();
    let ticks = runs
        .iter()
        .map(|run| run.latencies.len())
        .min()
        .unwrap_or(0);
    let mut sorted = Vec::with_capacity(ticks);
    let mut thresholds = Vec::with_capacity(n);
    let mut median_tick_us = Vec::with_capacity(n);
    let mut p99_us = Vec::with_capacity(n);
    for run in runs {
        sorted.clear();
        sorted.extend_from_slice(&run.latencies[..ticks]);
        sorted.sort_unstable();
        let mid = nearest_rank(&sorted, 50).unwrap_or(0);
        median_tick_us.push(us(mid));
        p99_us.push(us(nearest_rank(&sorted, 99).unwrap_or(0)));
        thresholds.push(mid as f64 * factor);
    }
    let needed = n.saturating_sub(1).max(1);
    let mut reproducible = Vec::new();
    let mut one_off = Vec::new();
    let mut medians = Vec::with_capacity(ticks);
    let mut values = Vec::with_capacity(n);
    for index in 0..ticks {
        values.clear();
        values.extend(runs.iter().map(|run| run.latencies[index]));
        let over = values
            .iter()
            .zip(&thresholds)
            .filter(|&(&ns, &limit)| ns as f64 > limit)
            .count();
        let max = values.iter().copied().max().unwrap_or(0);
        let mid = median(&mut values);
        medians.push((mid, index));
        if over == 0 {
            continue;
        }
        let entry = (over, mid, max, index);
        if over >= needed {
            reproducible.push(entry);
        } else {
            one_off.push(entry);
        }
    }
    let tick_of = |index: usize| first_tick + index as u64 + 1;
    let describe = |&(over, mid, max, index): &(usize, u64, u64, usize)| Outlier {
        tick: tick_of(index),
        runs_over: over,
        median_us: us(mid),
        max_us: us(max),
        phases_us: phases(runs, index),
    };
    reproducible.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)));
    one_off.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)));
    medians.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let worst_ticks = medians
        .iter()
        .take(worst)
        .map(|&(mid, index)| WorstTick {
            tick: tick_of(index),
            duration_us: us(mid),
            phases_us: phases(runs, index),
        })
        .collect();
    let report = RepeatReport {
        runs: n,
        outlier_factor: factor,
        median_tick_us,
        p99_us,
        reproducible_total: reproducible.len(),
        reproducible: reproducible.iter().take(LISTED).map(describe).collect(),
        one_off_total: one_off.len(),
        one_off: one_off.iter().take(LISTED).map(describe).collect(),
    };
    (report, worst_ticks)
}
