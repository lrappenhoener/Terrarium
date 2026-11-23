use std::{collections::BTreeMap, time::Instant};

use serde::Serialize;
use sim_host::PhaseProbe;

use crate::{RunnerError, report::nearest_rank, reserve};

const LIMIT: usize = 64;

#[derive(Default, Clone, Copy)]
struct Span {
    name: &'static str,
    ns: u64,
}

pub(crate) struct Recorder {
    starts: [Option<(&'static str, Instant)>; LIMIT],
    depth: usize,
    spans: [Span; LIMIT],
    count: usize,
    failed: bool,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            starts: [None; LIMIT],
            depth: 0,
            spans: [Span::default(); LIMIT],
            count: 0,
            failed: false,
        }
    }

    pub fn flush(&mut self, series: &mut Vec<PhaseSeries>, ticks: u64) -> Result<(), RunnerError> {
        if self.depth != 0 || self.failed {
            return Err(RunnerError::ContractViolation("unbalanced profiling phase boundaries or exceeded 64 distinct phases / nesting levels".into()));
        }
        for &Span { name, ns } in &self.spans[..self.count] {
            if let Some(phase) = series.iter_mut().find(|phase| phase.name == name) {
                phase.ns.push(ns);
            } else {
                let mut samples = reserve(ticks, "phase latency samples")?;
                samples.push(ns);
                series.push(PhaseSeries { name, ns: samples });
            }
        }
        self.count = 0;
        Ok(())
    }
}

impl PhaseProbe for Recorder {
    fn enter(&mut self, name: &'static str) {
        if let Some(start) = self.starts.get_mut(self.depth) {
            *start = Some((name, Instant::now()));
            self.depth += 1;
        } else {
            self.failed = true;
        }
    }

    fn exit(&mut self, name: &'static str) {
        let Some(depth) = self.depth.checked_sub(1) else {
            self.failed = true;
            return;
        };
        self.depth = depth;
        let Some((entered, start)) = self.starts[depth]
            .take()
            .filter(|(entered, _)| *entered == name)
        else {
            self.failed = true;
            return;
        };
        let ns = crate::nanoseconds(start.elapsed());
        if let Some(span) = self.spans[..self.count]
            .iter_mut()
            .find(|span| span.name == entered)
        {
            span.ns = span.ns.saturating_add(ns);
        } else if let Some(span) = self.spans.get_mut(self.count) {
            *span = Span { name, ns };
            self.count += 1;
        } else {
            self.failed = true;
        }
    }
}

pub(crate) struct PhaseSeries {
    name: &'static str,
    ns: Vec<u64>,
}

#[derive(Debug, Serialize)]
pub struct PhaseReport {
    pub name: &'static str,
    pub p50_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub share_percent: f64,
    pub dimension: &'static str,
    pub ns_per_unit_tick: Option<f64>,
}

impl PhaseSeries {
    pub fn report(
        mut self,
        total_ns: f64,
        ticks: u64,
        dimensions: &[(&'static str, &'static str)],
        units: &BTreeMap<&'static str, f64>,
    ) -> PhaseReport {
        self.ns.sort_unstable();
        let us = |ns: u64| ns as f64 / 1000.0;
        let phase_ns: u128 = self.ns.iter().map(|&value| u128::from(value)).sum();
        let dimension = dimensions
            .iter()
            .find_map(|&(phase, dim)| (phase == self.name).then_some(dim))
            .unwrap_or("entities");
        let unit_ticks = units.get(dimension).copied().unwrap_or(0.0) * ticks as f64;
        PhaseReport {
            dimension,
            ns_per_unit_tick: (unit_ticks > 0.0).then(|| phase_ns as f64 / unit_ticks),
            name: self.name,
            p50_us: us(nearest_rank(&self.ns, 50).unwrap_or(0)),
            p99_us: us(nearest_rank(&self.ns, 99).unwrap_or(0)),
            max_us: us(self.ns.last().copied().unwrap_or(0)),
            share_percent: if total_ns > 0.0 {
                phase_ns as f64 * 100.0 / total_ns
            } else {
                0.0
            },
        }
    }
}
