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

use crate::{
    Error,
    config::ConfigText,
    container::{self, Header, REPLAY, invalid},
    input::ScheduleEntry,
    io_at,
};

pub const FORMAT_VERSION: u16 = 2;
const RECORD_PREFIX: usize = 12;
/// Space kept free below `--replay-max-bytes` for the closing limit record.
const LIMIT_RESERVE: u64 = 64;

#[derive(Serialize, Deserialize)]
pub enum Origin {
    Config(ConfigText),
    Snapshot(Vec<u8>),
}

#[derive(Serialize, Deserialize)]
pub enum Commands {
    Binary(Vec<u8>),
    Json(Vec<u8>),
    Toml(String),
}

#[derive(Serialize, Deserialize)]
pub struct RecordedCheckpoint {
    pub tick: u64,
    pub hash: u64,
    pub subsystems: Vec<(String, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ending {
    Finished { tick: u64, hash: u64 },
    Panic { tick: u64, message: String },
    Failed { message: String },
    LimitReached { tick: u64 },
}

#[derive(Serialize, Deserialize)]
pub enum Record {
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

pub fn encode_commands<C: Serialize + serde::de::DeserializeOwned>(
    command: &[C],
) -> Result<Commands, Error> {
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

pub fn decode_commands<C: serde::de::DeserializeOwned>(
    commands: &Commands,
) -> Result<Vec<C>, Error> {
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

pub fn encode_record(record: &Record) -> Result<Vec<u8>, Error> {
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
pub enum Incomplete {
    Truncated { valid_bytes: u64 },
    Corrupt { valid_bytes: u64 },
}

pub struct Records {
    pub records: Vec<Record>,
    pub incomplete: Option<Incomplete>,
}

pub fn split_records(mut rest: &[u8], offset: u64) -> Records {
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

pub struct CommandGroup {
    pub tick: u64,
    pub commands: Commands,
}

/// A parsed replay file. Files without a closing record (after a crash) are
/// accepted up to their last complete record.
pub struct ReplayFile {
    pub header: Header,
    pub origin: Origin,
    pub recorded_target: u64,
    pub hash_interval: u64,
    pub commands: Vec<CommandGroup>,
    pub checkpoints: Vec<RecordedCheckpoint>,
    pub ending: Option<Ending>,
    pub incomplete: Option<Incomplete>,
}

impl ReplayFile {
    pub fn read(path: &Path) -> Result<Self, Error> {
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
        let mut file = Self {
            header,
            origin,
            recorded_target: target,
            hash_interval,
            commands: Vec::new(),
            checkpoints: Vec::new(),
            ending: None,
            incomplete,
        };
        let start = file.header.tick;
        if start > target {
            return Err(invalid("replay starts after its target tick"));
        }
        for record in records {
            match record {
                Record::Start { .. } => return Err(invalid("repeated replay start record")),
                Record::Commands { tick, commands } => {
                    if !(start..target).contains(&tick)
                        || file.commands.last().is_some_and(|last| last.tick >= tick)
                    {
                        return Err(invalid("invalid replay command tick or ordering"));
                    }
                    file.commands.push(CommandGroup { tick, commands });
                }
                Record::Checkpoint(point) => {
                    if !(start..=target).contains(&point.tick)
                        || file
                            .checkpoints
                            .last()
                            .is_some_and(|last| last.tick >= point.tick)
                    {
                        return Err(invalid("invalid replay checkpoint tick or ordering"));
                    }
                    file.checkpoints.push(point);
                }
                Record::End(ending) => file.ending = Some(ending),
            }
        }
        Ok(file)
    }

    /// Recordings without a regular end are compared only up to their last checkpoint.
    pub fn partial(&self) -> bool {
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

    pub fn schedule<C: serde::de::DeserializeOwned>(&self) -> Result<Vec<ScheduleEntry<C>>, Error> {
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

/// Settings of a recording that do not come from the simulation.
#[derive(Debug, Clone)]
pub struct RecordSettings {
    pub target: u64,
    pub hash_interval: u64,
    /// Ticks between flushes when no checkpoint is due.
    pub flush_every: u64,
    pub max_bytes: Option<u64>,
}

/// Writes a replay while the run progresses: commands before each step,
/// checkpoints with a flush, and a closing record.
pub struct ReplayWriter {
    out: BufWriter<File>,
    path: PathBuf,
    written: u64,
    max_bytes: Option<u64>,
    flush_every: u64,
    last_flush: u64,
    stopped: bool,
}

impl ReplayWriter {
    pub fn create(
        path: &Path,
        header: Header,
        origin: Origin,
        settings: &RecordSettings,
    ) -> Result<Self, Error> {
        let tick = header.tick;
        let file = File::create(path).map_err(|err| io_at(path, err))?;
        let mut writer = Self {
            out: BufWriter::new(file),
            path: path.into(),
            written: 0,
            max_bytes: settings.max_bytes,
            flush_every: settings.flush_every,
            last_flush: tick,
            stopped: false,
        };
        let prefix = container::encode_prefix(REPLAY, FORMAT_VERSION, &header)?;
        writer.raw(&prefix)?;
        // Header and start are always written; the size limit applies to later records.
        writer.raw(&encode_record(&Record::Start {
            origin,
            target: settings.target,
            hash_interval: settings.hash_interval,
        })?)?;
        writer.flush(tick)?;
        Ok(writer)
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.out
            .write_all(bytes)
            .map_err(|err| io_at(&self.path, err))?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn flush(&mut self, tick: u64) -> Result<(), Error> {
        self.last_flush = tick;
        self.out.flush().map_err(|err| io_at(&self.path, err))
    }

    fn record(&mut self, tick: u64, record: &Record) -> Result<(), Error> {
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
    ) -> Result<(), Error> {
        if commands.is_empty() || self.stopped {
            return Ok(());
        }
        let commands = encode_commands(commands)?;
        self.record(tick, &Record::Commands { tick, commands })
    }

    pub fn checkpoint(
        &mut self,
        tick: u64,
        hash: u64,
        subsystems: &[(&str, u64)],
    ) -> Result<(), Error> {
        self.record(
            tick,
            &Record::Checkpoint(RecordedCheckpoint {
                tick,
                hash,
                subsystems: subsystems
                    .iter()
                    .map(|&(name, hash)| (name.into(), hash))
                    .collect(),
            }),
        )?;
        self.flush(tick)
    }

    /// Flushes after at most `flush_every` ticks without a checkpoint.
    pub fn tick_done(&mut self, tick: u64) -> Result<(), Error> {
        if tick.saturating_sub(self.last_flush) >= self.flush_every.max(1) {
            self.flush(tick)?;
        }
        Ok(())
    }

    pub fn finish(mut self, tick: u64, ending: Ending) -> Result<(), Error> {
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

pub fn replay_game(path: &Path) -> Result<String, Error> {
    let bytes = std::fs::read(path).map_err(|err| io_at(path, err))?;
    Ok(container::decode_prefix(REPLAY, FORMAT_VERSION, &bytes)?
        .0
        .game)
}

/// Low-level access for tools and tests: header and all complete records.
pub fn read_records(path: &Path) -> Result<(Header, Records), Error> {
    let bytes = std::fs::read(path).map_err(|err| io_at(path, err))?;
    let (header, rest) = container::decode_prefix(REPLAY, FORMAT_VERSION, &bytes)?;
    Ok((header, split_records(rest, 0)))
}

/// Writes a complete replay from records, recomputing every checksum.
pub fn write_records(path: &Path, header: &Header, records: &[Record]) -> Result<(), Error> {
    let mut out = container::encode_prefix(REPLAY, FORMAT_VERSION, header)?;
    for record in records {
        out.extend(encode_record(record)?);
    }
    std::fs::write(path, out).map_err(|err| io_at(path, err))
}
