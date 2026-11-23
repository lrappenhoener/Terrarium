use std::path::Path;

use sim_host::{NoopProbe, SimError, Simulation};

use crate::{Checkpoint, RunOptions, RunnerError, Start, container, input::ScheduleEntry, io_at};

pub(crate) struct Difference<'a> {
    pub previous: Option<u64>,
    pub tick: u64,
    pub a: &'a Checkpoint,
    pub b: &'a Checkpoint,
}

pub(crate) fn compare<'a>(
    first: &'a [Checkpoint],
    second: &'a [Checkpoint],
) -> Option<Difference<'a>> {
    let mut previous = None;
    for (a, b) in first.iter().zip(second) {
        if a.tick != b.tick || a.hash != b.hash || a.subsystems != b.subsystems {
            return Some(Difference {
                previous,
                tick: a.tick.min(b.tick),
                a,
                b,
            });
        }
        previous = Some(a.tick);
    }
    if first.len() == second.len() {
        return None;
    }
    let (a, b) = first.last().zip(second.last())?;
    Some(Difference {
        previous,
        tick: a.tick.min(b.tick),
        a,
        b,
    })
}

pub(crate) fn subsystem_names(
    a: &[(&'static str, u64)],
    b: &[(&'static str, u64)],
) -> Vec<&'static str> {
    let mut subsystems: Vec<_> = a
        .iter()
        .filter(|entry| !b.contains(entry))
        .chain(b.iter().filter(|entry| !a.contains(entry)))
        .map(|&(name, _)| name)
        .collect();
    subsystems.sort_unstable();
    subsystems.dedup();
    subsystems
}

impl Difference<'_> {
    pub fn interval(&self) -> String {
        format_interval(self.previous, self.tick)
    }

    pub fn error(&self) -> RunnerError {
        RunnerError::DeterminismMismatch {
            tick: self.tick,
            first: self.a.hash,
            second: self.b.hash,
            subsystems: subsystem_names(&self.a.subsystems, &self.b.subsystems),
            interval: self.interval(),
            details: String::new(),
        }
    }
}

fn format_interval(from: Option<u64>, to: u64) -> String {
    from.map_or_else(
        || format!("start through {to}"),
        |from| format!("({from}, {to}]"),
    )
}

pub(crate) struct Exact {
    pub tick: u64,
    pub first: u64,
    pub second: u64,
    pub subsystems: Vec<&'static str>,
    pub details: String,
}

pub(crate) fn dump<S: Simulation>(sim: &S) -> Result<Option<String>, RunnerError> {
    let mut contents = String::new();
    match sim.debug_dump(&mut contents) {
        Ok(()) => Ok(Some(contents)),
        Err(SimError::Unsupported(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn details(
    dir: &Path,
    tick: u64,
    first: Result<Option<String>, RunnerError>,
    second: Result<Option<String>, RunnerError>,
) -> String {
    first
        .and_then(|first| second.and_then(|second| write_dumps(dir, tick, first, second)))
        .unwrap_or_else(|error| {
            format!(
                "
Could not produce state dumps: {error}"
            )
        })
}

fn write_dumps(
    dir: &Path,
    tick: u64,
    first: Option<String>,
    second: Option<String>,
) -> Result<String, RunnerError> {
    let (Some(a), Some(b)) = (first, second) else {
        return Ok(String::new());
    };
    std::fs::create_dir_all(dir).map_err(|e| io_at(dir, e))?;
    let a_path = dir.join(format!("tick-{tick}-a.txt"));
    let b_path = dir.join(format!("tick-{tick}-b.txt"));
    container::write(&a_path, a.as_bytes())?;
    container::write(&b_path, b.as_bytes())?;
    let differences = keyed_diff(&a, &b, 20);
    Ok(format!(
        "\ndumps: {} and {}\n{}",
        a_path.display(),
        b_path.display(),
        differences.join("\n")
    ))
}

struct Search<'a, S: Simulation> {
    opts: &'a RunOptions,
    start: &'a Start<S::Config>,
    schedule: &'a [ScheduleEntry<S::Command>],
    roundtrips: &'a [u64],
    lower: Option<u64>,
    upper: u64,
}

impl<S: Simulation> Search<'_, S> {
    fn scan(&self, snapshot: Option<&[u8]>) -> Result<Option<Exact>, RunnerError> {
        let make_sim = || -> Result<S, RunnerError> {
            match snapshot {
                Some(bytes) => Ok(S::load_snapshot(bytes)?),
                None => self.start.instantiate::<S>(self.opts),
            }
        };
        let mut a = make_sim()?;
        let mut b = make_sim()?;
        let mut index = self.schedule.partition_point(|entry| entry.tick < a.tick());
        let start_tick = a.tick();
        let mut probe = NoopProbe;
        loop {
            let tick = a.tick();
            if self.roundtrips.binary_search(&tick).is_ok()
                && (snapshot.is_none() || tick != start_tick)
            {
                let mut bytes = Vec::new();
                b.save_snapshot(&mut bytes)?;
                drop(b);
                b = S::load_snapshot(&bytes)?;
            }
            if self.lower.is_none_or(|lower| tick > lower) || tick == self.upper {
                let (first, second) = (a.state_hash(), b.state_hash());
                let subsystems = subsystem_names(&a.subsystem_hashes(), &b.subsystem_hashes());
                if first != second
                    || a.tick() != b.tick()
                    || a.is_finished() != b.is_finished()
                    || !subsystems.is_empty()
                {
                    let details = details(&self.opts.artifacts.diff_dir, tick, dump(&a), dump(&b));
                    return Ok(Some(Exact {
                        tick,
                        first,
                        second,
                        subsystems,
                        details,
                    }));
                }
            }
            if tick >= self.upper || a.is_finished() || b.is_finished() {
                return Ok(None);
            }
            let commands = match self.schedule.get(index) {
                Some(entry) if entry.tick == tick => {
                    index += 1;
                    entry.commands.as_slice()
                }
                _ => &[],
            };
            a.step_profiled(commands, &mut probe);
            b.step_profiled(commands, &mut probe);
            if a.tick() != tick + 1 || b.tick() != tick + 1 {
                return Err(RunnerError::ContractViolation(
                    "fine comparison must step exactly one tick".into(),
                ));
            }
        }
    }
}

pub(crate) fn locate<S: Simulation>(
    opts: &RunOptions,
    start: &Start<S::Config>,
    schedule: &[ScheduleEntry<S::Command>],
    roundtrips: &[u64],
    baseline: &[Checkpoint],
    interval: &Difference<'_>,
) -> Result<Option<Exact>, RunnerError> {
    let search = Search::<S> {
        opts,
        start,
        schedule,
        roundtrips,
        lower: interval.previous,
        upper: interval.tick,
    };
    let anchor = interval
        .previous
        .and_then(|tick| baseline.iter().find(|point| point.tick == tick))
        .and_then(|point| point.snapshot.as_deref());
    // Starting from the same logical state can erase a defect in derived state.
    // Recreate the original variants if that shortcut does not reproduce it.
    if let Some(anchor) = anchor
        && let Ok(Some(exact)) = search.scan(Some(anchor))
    {
        return Ok(Some(exact));
    }
    search.scan(None)
}

/// Splits `key<TAB>value` lines. A line without a tab is its own key with an empty value.
/// Repeated keys receive an occurrence suffix so that each entry stays addressable.
fn keyed_lines(dump: &str) -> Vec<(String, &str)> {
    let mut seen = std::collections::BTreeMap::<&str, usize>::new();
    dump.lines()
        .map(|line| {
            let (key, value) = line.split_once('\t').unwrap_or((line, ""));
            let count = seen.entry(key).or_insert(0);
            *count += 1;
            let key = if *count == 1 {
                key.to_owned()
            } else {
                format!("{key}#{count}")
            };
            (key, value)
        })
        .collect()
}

fn entry(key: &str, value: &str) -> String {
    if value.is_empty() {
        key.to_owned()
    } else {
        format!("{key}\t{value}")
    }
}

/// Compares dumps by stable key: entries only in A, only in B, and differing values.
/// Reports follow A's line order, then B's order for its extra keys.
pub(crate) fn keyed_diff(a: &str, b: &str, limit: usize) -> Vec<String> {
    use std::collections::BTreeMap;
    let left = keyed_lines(a);
    let right = keyed_lines(b);
    let left_map: BTreeMap<&str, &str> = left.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let right_map: BTreeMap<&str, &str> = right.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut differences = Vec::new();
    let mut total = 0usize;
    let mut report = |message: String| {
        total += 1;
        if differences.len() < limit {
            differences.push(message);
        }
    };
    for (key, value) in &left {
        match right_map.get(key.as_str()) {
            None => report(format!("only in A: {}", entry(key, value))),
            Some(other) if other != value => {
                report(format!("differs {key}: A {value} | B {other}"));
            }
            Some(_) => {}
        }
    }
    for (key, value) in &right {
        if !left_map.contains_key(key.as_str()) {
            report(format!("only in B: {}", entry(key, value)));
        }
    }
    if total > differences.len() {
        differences.push(format!(
            "... {} more differences",
            total - differences.len()
        ));
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::keyed_diff;

    #[test]
    fn an_extra_entity_is_reported_once_instead_of_shifting_all_lines() {
        let a = "meta\ttick=5\nentity:1:0\tx=1\nentity:3:0\tx=3\nentity:4:0\tx=4\n";
        let b =
            "meta\ttick=5\nentity:1:0\tx=1\nentity:2:0\tx=2\nentity:3:0\tx=3\nentity:4:0\tx=4\n";
        assert_eq!(keyed_diff(a, b, 20), ["only in B: entity:2:0\tx=2"]);
        let c = "meta\ttick=5\nentity:1:0\tx=9\nentity:3:0\tx=3\n";
        assert_eq!(
            keyed_diff(a, c, 20),
            [
                "differs entity:1:0: A x=1 | B x=9",
                "only in A: entity:4:0\tx=4"
            ]
        );
    }

    #[test]
    fn lines_without_tabs_compare_as_whole_lines() {
        let diff = keyed_diff("same\nold line\n", "same\nnew line\n", 20);
        assert_eq!(diff, ["only in A: old line", "only in B: new line"]);
        let many: String = (0..30).map(|i| format!("k{i}\t{i}\n")).collect();
        let diff = keyed_diff(&many, "", 5);
        assert_eq!(diff.len(), 6);
        assert_eq!(diff[5], "... 25 more differences");
    }
}
