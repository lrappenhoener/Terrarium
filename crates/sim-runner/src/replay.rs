//! Streaming replay format with an output-independent start.
//!
//! A replay starts with the shared container prefix (format version 2) and continues
//! with records. Each record is `length: u32 LE`, `checksum: u64 LE` (FNV-1a over the
//! body) and a Postcard body. The first record describes the start, followed by
//! commands per tick and hash checkpoints in tick order. A final record states how the
//! recording ended. Readers stop at the first incomplete or corrupt record, so files
//! of interrupted runs remain playable up to their last complete record.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sim_host::Simulation;

use crate::{
    Checkpoint, RunnerError, Start,
    container::{self, Header, REPLAY, SNAPSHOT, invalid},
    input::ScheduleEntry,
    io_at,
    options::ConfigText,
};

const FORMAT_VERSION: u16 = 2;
const RECORD_PREFIX: usize = 12;
/// Space kept free below `--replay-max-bytes` for the closing limit record.
const LIMIT_RESERVE: u64 = 64;

#[derive(Serialize, Deserialize)]
enum Origin {
    Config(ConfigText),
    Snapshot(Vec<u8>),
}

#[derive(Serialize, Deserialize)]
enum Commands {
    Binary(Vec<u8>),
    Json(Vec<u8>),
    Toml(String),
}

#[derive(Serialize, Deserialize)]
struct RecordedCheckpoint {
    tick: u64,
    hash: u64,
    subsystems: Vec<(String, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Ending {
    Finished { tick: u64, hash: u64 },
    Panic { tick: u64, message: String },
    Failed { message: String },
    LimitReached { tick: u64 },
}

#[derive(Serialize, Deserialize)]
enum Record {
    Start {
        origin: Origin,
        target: u64,
        hash_interval: u64,
    },
    Commands {
        tick: u64,
        commands: Commands,
    },
    Checkpoint(RecordedCheckpoint),
    End(Ending),
}

#[derive(Serialize, Deserialize)]
struct CommandList<C> {
    commands: C,
}

fn encode_commands<C: Serialize + serde::de::DeserializeOwned>(
    command: &[C],
) -> Result<Commands, RunnerError> {
    if let Ok(bytes) = postcard::to_allocvec(command)
        && postcard::take_from_bytes::<Vec<C>>(&bytes).is_ok_and(|(_, rest)| rest.is_empty())
    {
        return Ok(Commands::Binary(bytes));
    }
    let bytes = serde_json::to_vec(command).map_err(|error| invalid(error.to_string()))?;
    if serde_json::from_slice::<Vec<C>>(&bytes).is_ok() {
        return Ok(Commands::Json(bytes));
    }
    let text = toml::to_string(&CommandList { commands: command })
        .map_err(|error| invalid(error.to_string()))?;
    toml::from_str::<CommandList<Vec<C>>>(&text).map_err(|error| invalid(error.to_string()))?;
    Ok(Commands::Toml(text))
}

fn decode_commands<C: serde::de::DeserializeOwned>(
    commands: &Commands,
) -> Result<Vec<C>, RunnerError> {
    Ok(match commands {
        Commands::Binary(bytes) => {
            let (decoded, rest) =
                postcard::take_from_bytes(bytes).map_err(|error| invalid(error.to_string()))?;
            if !rest.is_empty() {
                return Err(invalid("trailing commands"));
            }
            decoded
        }
        Commands::Json(bytes) => {
            serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?
        }
        Commands::Toml(text) => {
            toml::from_str::<CommandList<Vec<C>>>(text)
                .map_err(|error| invalid(error.to_string()))?
                .commands
        }
    })
}

fn encode_record(record: &Record) -> Result<Vec<u8>, RunnerError> {
    let body = postcard::to_allocvec(record).map_err(|e| invalid(e.to_string()))?;
    let len = u32::try_from(body.len()).map_err(|_| invalid("replay record too large"))?;
    let mut bytes = Vec::with_capacity(RECORD_PREFIX + body.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&container::checksum_bytes(&body).to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Why reading stopped before a closing record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incomplete {
    Truncated { valid_bytes: u64 },
    Corrupt { valid_bytes: u64 },
}

struct Records {
    records: Vec<Record>,
    incomplete: Option<Incomplete>,
}

fn split_records(mut rest: &[u8], offset: u64) -> Records {
    let mut records = Vec::new();
    let mut valid = offset;
    let incomplete = loop {
        if rest.is_empty() {
            break None;
        }
        let Some((prefix, body)) = rest.split_first_chunk::<RECORD_PREFIX>() else {
            break Some(Incomplete::Truncated { valid_bytes: valid });
        };
        let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
        let mut sum = [0; 8];
        sum.copy_from_slice(&prefix[4..]);
        let Some((body, next)) = body.split_at_checked(len) else {
            break Some(Incomplete::Truncated { valid_bytes: valid });
        };
        if container::checksum_bytes(body) != u64::from_le_bytes(sum) {
            break Some(Incomplete::Corrupt { valid_bytes: valid });
        }
        let Ok((record, trailing)) = postcard::take_from_bytes::<Record>(body) else {
            break Some(Incomplete::Corrupt { valid_bytes: valid });
        };
        if !trailing.is_empty() {
            break Some(Incomplete::Corrupt { valid_bytes: valid });
        }
        let stop = matches!(record, Record::End(_));
        records.push(record);
        valid += (RECORD_PREFIX + len) as u64;
        rest = next;
        if stop {
            break None;
        }
    };
    Records {
        records,
        incomplete,
    }
}

struct CommandGroup {
    tick: u64,
    commands: Commands,
}

pub(crate) struct Playback {
    pub header: Header,
    origin: Origin,
    recorded_target: u64,
    hash_interval: u64,
    commands: Vec<CommandGroup>,
    checkpoints: Vec<RecordedCheckpoint>,
    pub ending: Option<Ending>,
    pub incomplete: Option<Incomplete>,
}

impl Playback {
    pub fn read(path: &Path) -> Result<Self, RunnerError> {
        let bytes = std::fs::read(path).map_err(|err| io_at(path, err))?;
        let (header, rest) = container::decode_prefix(REPLAY, FORMAT_VERSION, &bytes)?;
        let offset = (bytes.len() - rest.len()) as u64;
        let Records {
            records,
            incomplete,
        } = split_records(rest, offset);
        let mut records = records.into_iter();
        let Some(Record::Start {
            origin,
            target,
            hash_interval,
        }) = records.next()
        else {
            return Err(invalid("replay lacks a complete start record"));
        };
        let mut playback = Self {
            header,
            origin,
            recorded_target: target,
            hash_interval,
            commands: Vec::new(),
            checkpoints: Vec::new(),
            ending: None,
            incomplete,
        };
        let start = playback.header.tick;
        if start > target {
            return Err(invalid("replay starts after its target tick"));
        }
        for record in records {
            match record {
                Record::Start { .. } => return Err(invalid("repeated replay start record")),
                Record::Commands { tick, commands } => {
                    if !(start..target).contains(&tick)
                        || playback
                            .commands
                            .last()
                            .is_some_and(|last| last.tick >= tick)
                    {
                        return Err(invalid("invalid replay command tick or ordering"));
                    }
                    playback.commands.push(CommandGroup { tick, commands });
                }
                Record::Checkpoint(point) => {
                    if !(start..=target).contains(&point.tick)
                        || playback
                            .checkpoints
                            .last()
                            .is_some_and(|last| last.tick >= point.tick)
                    {
                        return Err(invalid("invalid replay checkpoint tick or ordering"));
                    }
                    playback.checkpoints.push(point);
                }
                Record::End(ending) => playback.ending = Some(ending),
            }
        }
        Ok(playback)
    }

    /// Recordings without a regular end are compared only up to their last checkpoint.
    fn partial(&self) -> bool {
        !matches!(
            self.ending,
            Some(Ending::Finished { .. } | Ending::Failed { .. })
        )
    }

    /// Last tick the recording covers. Interrupted files end after their last record.
    pub fn target(&self) -> u64 {
        match &self.ending {
            Some(Ending::Finished { .. } | Ending::Failed { .. }) => self.recorded_target,
            Some(Ending::Panic { tick, .. }) => tick.saturating_add(1).min(self.recorded_target),
            Some(Ending::LimitReached { .. }) | None => self
                .checkpoints
                .last()
                .map(|point| point.tick)
                .into_iter()
                .chain(self.commands.last().map(|group| group.tick + 1))
                .chain(std::iter::once(self.header.tick))
                .max()
                .unwrap_or(self.header.tick),
        }
    }

    pub fn status(&self) -> String {
        let target = self.target();
        match (&self.ending, &self.incomplete) {
            (Some(Ending::Finished { tick, .. }), _) => format!("vollständig bis Tick {tick}"),
            (Some(Ending::Failed { message }), _) => {
                format!("vollständig, Aufzeichnung endete mit Fehler: {message}")
            }
            (Some(Ending::Panic { tick, message }), _) => {
                format!("Panic bei Tick {tick} aufgezeichnet: {message}")
            }
            (Some(Ending::LimitReached { tick }), _) => {
                format!("Größengrenze bei Tick {tick} erreicht, abgespielt bis Tick {target}")
            }
            (None, Some(Incomplete::Truncated { valid_bytes })) => format!(
                "unvollständig (abgeschnittener Rest nach {valid_bytes} Bytes ignoriert), abgespielt bis Tick {target}"
            ),
            (None, Some(Incomplete::Corrupt { valid_bytes })) => format!(
                "unvollständig (fehlerhafter Datensatz nach {valid_bytes} Bytes), abgespielt bis Tick {target}"
            ),
            (None, None) => {
                format!("unvollständig (kein Abschlussdatensatz), abgespielt bis Tick {target}")
            }
        }
    }

    pub fn options(&self, opts: &mut crate::RunOptions) {
        opts.seed = self.header.seed;
        opts.ticks = self.target();
        opts.hash_interval = self.hash_interval;
    }

    pub fn start<S: Simulation>(&self) -> Result<Start<S::Config>, RunnerError> {
        match &self.origin {
            Origin::Config(text) => text.decode().map(Start::New),
            Origin::Snapshot(bytes) => {
                let data = container::decode(SNAPSHOT, bytes)?;
                if data.header.seed != self.header.seed || data.header.tick != self.header.tick {
                    return Err(invalid(
                        "embedded snapshot tick or seed differs from replay",
                    ));
                }
                Ok(Start::Snapshot(data, std::time::Duration::ZERO))
            }
        }
    }

    pub fn schedule<C: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<Vec<ScheduleEntry<C>>, RunnerError> {
        let target = self.target();
        self.commands
            .iter()
            .filter(|group| group.tick < target)
            .map(|group| {
                Ok(ScheduleEntry {
                    tick: group.tick,
                    commands: decode_commands(&group.commands)?,
                })
            })
            .collect()
    }

    pub fn check_start<S: Simulation>(&self, sim: &S, force: bool) -> Result<(), RunnerError> {
        if sim.content_fingerprint() != self.header.content_fingerprint && !force {
            return Err(invalid(
                "replay content fingerprint differs; use --force to compare anyway",
            ));
        }
        let subsystems = container::differing(
            self.header
                .subsystems
                .iter()
                .map(|(name, hash)| (name.as_str(), *hash)),
            sim.subsystem_hashes(),
        );
        if sim.tick() != self.header.tick
            || sim.state_hash() != self.header.state_hash
            || !subsystems.is_empty()
        {
            return Err(RunnerError::ReplayMismatch {
                from: self.header.tick,
                to: self.header.tick,
                subsystems,
            });
        }
        Ok(())
    }

    pub fn check(&self, actual: &[Checkpoint]) -> Result<(), RunnerError> {
        let mut previous = self.header.tick;
        for (index, expected) in self.checkpoints.iter().enumerate() {
            let current = actual.get(index);
            let subsystems = container::differing(
                expected
                    .subsystems
                    .iter()
                    .map(|(name, hash)| (name.as_str(), *hash)),
                current
                    .into_iter()
                    .flat_map(|point| point.subsystems.iter().copied()),
            );
            if !subsystems.is_empty()
                || current
                    .is_none_or(|point| point.tick != expected.tick || point.hash != expected.hash)
            {
                return Err(RunnerError::ReplayMismatch {
                    from: previous,
                    to: expected.tick.min(current.map_or(expected.tick, |p| p.tick)),
                    subsystems,
                });
            }
            previous = expected.tick;
        }
        if !self.partial()
            && let Some(extra) = actual.get(self.checkpoints.len())
        {
            return Err(RunnerError::ReplayMismatch {
                from: previous,
                to: extra.tick,
                subsystems: Vec::new(),
            });
        }
        Ok(())
    }

    /// Compares a reproduced panic with the recorded one.
    pub fn explain_panic(&self, tick: u64) -> &'static str {
        match &self.ending {
            Some(Ending::Panic { tick: recorded, .. }) if *recorded == tick => {
                "reproduziert wie aufgezeichnet"
            }
            Some(Ending::Panic { .. }) => "weicht vom aufgezeichneten Panic-Tick ab",
            _ => "nicht aufgezeichnet",
        }
    }
}

pub(crate) struct ReplayWriter {
    out: BufWriter<File>,
    path: PathBuf,
    written: u64,
    max_bytes: Option<u64>,
    flush_every: u64,
    last_flush: u64,
    stopped: bool,
}

impl ReplayWriter {
    pub fn create<S: Simulation>(
        path: &Path,
        opts: &crate::RunOptions,
        start: &Start<S::Config>,
        config: Option<&ConfigText>,
        header: Header,
    ) -> Result<Self, RunnerError> {
        let origin = match start {
            Start::New(_) => Origin::Config(
                config
                    .cloned()
                    .ok_or_else(|| invalid("missing replay configuration"))?,
            ),
            Start::Snapshot(data, _) => Origin::Snapshot(container::encode(
                SNAPSHOT,
                data.header.clone(),
                &data.payload,
            )?),
        };
        let tick = header.tick;
        let file = File::create(path).map_err(|err| io_at(path, err))?;
        let mut writer = Self {
            out: BufWriter::new(file),
            path: path.into(),
            written: 0,
            max_bytes: opts.artifacts.replay_max_bytes,
            flush_every: opts.artifacts.replay_flush_every,
            last_flush: tick,
            stopped: false,
        };
        let prefix = container::encode_prefix(REPLAY, FORMAT_VERSION, &header)?;
        writer.raw(&prefix)?;
        // Header and start are always written; the size limit applies to later records.
        writer.raw(&encode_record(&Record::Start {
            origin,
            target: opts.ticks,
            hash_interval: opts.hash_interval,
        })?)?;
        writer.flush(tick)?;
        Ok(writer)
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), RunnerError> {
        self.out
            .write_all(bytes)
            .map_err(|err| io_at(&self.path, err))?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn flush(&mut self, tick: u64) -> Result<(), RunnerError> {
        self.last_flush = tick;
        self.out.flush().map_err(|err| io_at(&self.path, err))
    }

    fn record(&mut self, tick: u64, record: &Record) -> Result<(), RunnerError> {
        if self.stopped {
            return Ok(());
        }
        let bytes = encode_record(record)?;
        if let Some(limit) = self.max_bytes
            && self.written + bytes.len() as u64 + LIMIT_RESERVE > limit
        {
            self.stopped = true;
            self.raw(&encode_record(&Record::End(Ending::LimitReached { tick }))?)?;
            self.flush(tick)?;
            let _ = writeln!(
                std::io::stderr().lock(),
                "Warnung: Replay-Grenze von {limit} Bytes bei Tick {tick} erreicht; Aufzeichnung beendet"
            );
            return Ok(());
        }
        self.raw(&bytes)
    }

    /// Records the commands of a tick before it is stepped.
    pub fn commands<C: Serialize + serde::de::DeserializeOwned>(
        &mut self,
        tick: u64,
        commands: &[C],
    ) -> Result<(), RunnerError> {
        if commands.is_empty() || self.stopped {
            return Ok(());
        }
        let commands = encode_commands(commands)?;
        self.record(tick, &Record::Commands { tick, commands })
    }

    pub fn checkpoint(&mut self, point: &Checkpoint) -> Result<(), RunnerError> {
        self.record(
            point.tick,
            &Record::Checkpoint(RecordedCheckpoint {
                tick: point.tick,
                hash: point.hash,
                subsystems: point
                    .subsystems
                    .iter()
                    .map(|&(name, hash)| (name.into(), hash))
                    .collect(),
            }),
        )?;
        self.flush(point.tick)
    }

    /// Flushes after at most `flush_every` ticks without a checkpoint.
    pub fn tick_done(&mut self, tick: u64) -> Result<(), RunnerError> {
        if tick.saturating_sub(self.last_flush) >= self.flush_every.max(1) {
            self.flush(tick)?;
        }
        Ok(())
    }

    pub fn finish(mut self, tick: u64, ending: Ending) -> Result<(), RunnerError> {
        if !self.stopped {
            self.raw(&encode_record(&Record::End(ending))?)?;
        }
        self.flush(tick)?;
        let file = self
            .out
            .into_inner()
            .map_err(|err| io_at(&self.path, err.into_error()))?;
        file.sync_all().map_err(|err| io_at(&self.path, err))
    }
}

pub fn replay_game(path: &Path) -> Result<String, RunnerError> {
    let bytes = std::fs::read(path).map_err(|err| io_at(path, err))?;
    Ok(container::decode_prefix(REPLAY, FORMAT_VERSION, &bytes)?
        .0
        .game)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RunOptions, run_simulation,
        tests::{StableSim, options},
    };

    fn rewrite(path: &Path, mut edit: impl FnMut(&mut Record)) {
        let bytes = std::fs::read(path).unwrap();
        let (header, rest) = container::decode_prefix(REPLAY, FORMAT_VERSION, &bytes).unwrap();
        let mut out = container::encode_prefix(REPLAY, FORMAT_VERSION, &header).unwrap();
        let Records {
            records,
            incomplete,
        } = split_records(rest, 0);
        assert!(incomplete.is_none());
        for mut record in records {
            edit(&mut record);
            out.extend(encode_record(&record).unwrap());
        }
        std::fs::write(path, out).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("terrarium-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn changed_command_with_recomputed_checksum_diverges() {
        let dir = temp_dir("replay-mutation");
        let schedule = dir.join("schedule.json");
        std::fs::write(&schedule, r#"{"schedule":[{"tick":1,"commands":[2]}]}"#).unwrap();
        let replay = dir.join("case.simr");
        let mut opts = options();
        opts.commands = Some(schedule);
        opts.artifacts.record_replay = Some(replay.clone());
        run_simulation::<StableSim>(&opts).unwrap();

        rewrite(&replay, |record| {
            if let Record::Commands { commands, .. } = record {
                *commands = encode_commands(&[5usize]).unwrap();
            }
        });
        opts.commands = None;
        opts.artifacts.record_replay = None;
        opts.artifacts.replay = Some(replay);
        assert!(matches!(
            run_simulation::<StableSim>(&opts),
            Err(RunnerError::ReplayMismatch { from: 0, to: 2, .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn record_counter(dir: &Path, name: &str, opts: impl FnOnce(&mut RunOptions)) -> PathBuf {
        let replay = dir.join(name);
        let mut run = RunOptions {
            ticks: 500,
            hash_interval: 100,
            ..options()
        };
        run.artifacts.record_replay = Some(replay.clone());
        opts(&mut run);
        run_simulation::<crate::tests::Counter>(&run).unwrap();
        replay
    }

    fn play(replay: &Path) -> Result<crate::RunReport, RunnerError> {
        let mut opts = RunOptions {
            ticks: 500,
            hash_interval: 100,
            ..options()
        };
        opts.artifacts.replay = Some(replay.into());
        run_simulation::<crate::tests::Counter>(&opts)
    }

    #[test]
    fn truncated_replay_plays_to_its_last_complete_record() {
        let dir = temp_dir("replay-truncated");
        let replay = record_counter(&dir, "full.simr", |_| {});
        let full = Playback::read(&replay).unwrap();
        assert!(matches!(
            full.ending,
            Some(Ending::Finished { tick: 500, .. })
        ));
        assert_eq!(play(&replay).unwrap().replay_status.unwrap(), full.status());

        let bytes = std::fs::read(&replay).unwrap();
        // Remove the end record and cut the 500 checkpoint in half.
        let cut = bytes.len() - 40;
        std::fs::write(&replay, &bytes[..cut]).unwrap();
        let partial = Playback::read(&replay).unwrap();
        assert_eq!(partial.ending, None);
        assert!(matches!(
            partial.incomplete,
            Some(Incomplete::Truncated { .. })
        ));
        assert_eq!(partial.checkpoints.last().unwrap().tick, 400);
        let report = play(&replay).unwrap();
        assert_eq!(report.tick, 400);
        assert!(report.replay_verified);
        assert!(report.replay_status.unwrap().starts_with("unvollständig"));

        let mut corrupt = bytes[..cut].to_vec();
        let index = corrupt.len() - 5;
        corrupt[index] ^= 0xff;
        std::fs::write(&replay, &corrupt).unwrap();
        assert!(play(&replay).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn size_limit_stops_recording_with_a_closing_record() {
        let dir = temp_dir("replay-limit");
        let full = record_counter(&dir, "full.simr", |_| {});
        let full_len = std::fs::metadata(&full).unwrap().len();
        let limited = record_counter(&dir, "limited.simr", |opts| {
            opts.artifacts.replay_max_bytes = Some(full_len - 60);
        });
        assert!(std::fs::metadata(&limited).unwrap().len() <= full_len - 60);
        let playback = Playback::read(&limited).unwrap();
        assert!(matches!(playback.ending, Some(Ending::LimitReached { .. })));
        let report = play(&limited).unwrap();
        assert!(report.replay_status.unwrap().contains("Größengrenze"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_panic_is_recorded_and_reproduced_at_tick_123() {
        let dir = temp_dir("replay-panic");
        let replay = dir.join("panic.simr");
        let mut opts = RunOptions {
            ticks: 500,
            hash_interval: 50,
            ..options()
        };
        opts.artifacts.record_replay = Some(replay.clone());
        opts.artifacts.replay_flush_every = 1000;
        let error = run_simulation::<crate::tests::Panicking>(&opts).unwrap_err();
        assert!(matches!(
            &error,
            RunnerError::SimulationPanic { tick: 123, message } if message.contains("deliberate panic")
        ));
        let playback = Playback::read(&replay).unwrap();
        assert!(matches!(
            &playback.ending,
            Some(Ending::Panic { tick: 123, message }) if message.contains("deliberate panic")
        ));
        assert_eq!(playback.target(), 124);

        opts.artifacts.record_replay = None;
        opts.artifacts.replay = Some(replay);
        let error = run_simulation::<crate::tests::Panicking>(&opts).unwrap_err();
        assert!(matches!(
            &error,
            RunnerError::SimulationPanic { tick: 123, message }
                if message.contains("reproduziert wie aufgezeichnet")
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
