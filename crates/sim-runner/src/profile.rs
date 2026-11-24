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

#[derive(Debug, Serialize)]
pub struct WorstTick {
    pub tick: u64,
    pub duration_us: f64,
    pub phases_us: Vec<(&'static str, f64)>,
}

struct TimedTick {
    key: (u64, std::cmp::Reverse<u64>),
    spans: [Span; LIMIT],
    count: usize,
}

impl PartialEq for TimedTick {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for TimedTick {}
impl PartialOrd for TimedTick {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimedTick {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

pub(crate) struct WorstTicks {
    heap: std::collections::BinaryHeap<std::cmp::Reverse<TimedTick>>,
    limit: usize,
}

impl WorstTicks {
    pub fn new(limit: u64) -> Result<Self, RunnerError> {
        let entries = reserve(limit, "worst tick samples")?;
        Ok(Self {
            heap: entries.into(),
            limit: limit as usize,
        })
    }

    pub fn record(&mut self, tick: u64, ns: u64, recorder: &Recorder) {
        use std::cmp::Reverse;
        let key = (ns, Reverse(tick));
        if self.limit == 0 {
            return;
        }
        if self.heap.len() == self.limit {
            if self
                .heap
                .peek()
                .is_some_and(|existing| existing.0.key >= key)
            {
                return;
            }
            self.heap.pop();
        }
        self.heap.push(Reverse(TimedTick {
            key,
            spans: recorder.spans,
            count: recorder.count,
        }));
    }

    pub fn report(self) -> Vec<WorstTick> {
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|entry| {
                let tick = entry.0;
                WorstTick {
                    tick: tick.key.1.0,
                    duration_us: tick.key.0 as f64 / 1000.0,
                    phases_us: tick.spans[..tick.count]
                        .iter()
                        .map(|span| (span.name, span.ns as f64 / 1000.0))
                        .collect(),
                }
            })
            .collect()
    }
}

/// Phase durations of every tick of one run, for `--repeat`.
#[derive(Default)]
pub(crate) struct TickPhases {
    names: Vec<&'static str>,
    offsets: Vec<usize>,
    spans: Vec<(usize, u64)>,
}

impl TickPhases {
    pub fn record(&mut self, recorder: &Recorder) {
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        for span in &recorder.spans[..recorder.count] {
            let index = self
                .names
                .iter()
                .position(|name| *name == span.name)
                .unwrap_or_else(|| {
                    self.names.push(span.name);
                    self.names.len() - 1
                });
            self.spans.push((index, span.ns));
        }
        self.offsets.push(self.spans.len());
    }

    pub fn names(&self) -> &[&'static str] {
        &self.names
    }

    /// Nanoseconds of `name` in the tick with index `tick`, zero if absent.
    pub fn get(&self, tick: usize, name: &str) -> u64 {
        let (Some(&from), Some(&to)) = (self.offsets.get(tick), self.offsets.get(tick + 1)) else {
            return 0;
        };
        self.spans[from..to]
            .iter()
            .find(|(index, _)| self.names[*index] == name)
            .map_or(0, |&(_, ns)| ns)
    }
}
