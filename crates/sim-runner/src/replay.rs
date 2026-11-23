//! Replay payload, with opaque command groups and an output-independent start.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sim_host::Simulation;

use crate::{
    Checkpoint, RunnerError, Start,
    container::{self, Header, REPLAY, SNAPSHOT, invalid},
    input::ScheduleEntry,
    options::ConfigText,
};

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
struct CommandGroup {
    tick: u64,
    commands: Commands,
}

#[derive(Serialize, Deserialize)]
struct RecordedCheckpoint {
    tick: u64,
    hash: u64,
    subsystems: Vec<(String, u64)>,
}

#[derive(Serialize, Deserialize)]
struct Replay {
    start: Origin,
    target: u64,
    hash_interval: u64,
    commands: Vec<CommandGroup>,
    checkpoints: Vec<RecordedCheckpoint>,
}

pub(crate) struct Playback {
    pub header: Header,
    replay: Replay,
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

impl Playback {
    pub fn read(path: &Path) -> Result<Self, RunnerError> {
        let data = container::read(REPLAY, path)?;
        let (replay, rest) = postcard::take_from_bytes::<Replay>(&data.payload)
            .map_err(|e| invalid(e.to_string()))?;
        if !rest.is_empty()
            || data.header.tick > replay.target
            || replay
                .commands
                .windows(2)
                .any(|pair| pair[0].tick >= pair[1].tick)
            || replay
                .commands
                .iter()
                .any(|e| !(data.header.tick..replay.target).contains(&e.tick))
            || replay.checkpoints.is_empty()
            || replay
                .checkpoints
                .windows(2)
                .any(|pair| pair[0].tick >= pair[1].tick)
            || replay
                .checkpoints
                .iter()
                .any(|point| !(data.header.tick..=replay.target).contains(&point.tick))
        {
            return Err(invalid("invalid replay tick range or ordering"));
        }
        Ok(Self {
            header: data.header,
            replay,
        })
    }

    pub fn options(&self, opts: &mut crate::RunOptions) {
        opts.seed = self.header.seed;
        opts.ticks = self.replay.target;
        opts.hash_interval = self.replay.hash_interval;
    }

    pub fn start<S: Simulation>(&self) -> Result<Start<S::Config>, RunnerError> {
        match &self.replay.start {
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
        self.replay
            .commands
            .iter()
            .map(|entry| {
                let commands = match &entry.commands {
                    Commands::Binary(bytes) => {
                        let (decoded, rest) = postcard::take_from_bytes(bytes)
                            .map_err(|error| invalid(error.to_string()))?;
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
                };
                Ok(ScheduleEntry {
                    tick: entry.tick,
                    commands,
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
        for (index, expected) in self.replay.checkpoints.iter().enumerate() {
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
        if let Some(extra) = actual.get(self.replay.checkpoints.len()) {
            return Err(RunnerError::ReplayMismatch {
                from: previous,
                to: extra.tick,
                subsystems: Vec::new(),
            });
        }
        Ok(())
    }
}

pub(crate) fn record<S: Simulation>(
    path: &Path,
    opts: &crate::RunOptions,
    start: &Start<S::Config>,
    config: Option<ConfigText>,
    header: Header,
    schedule: &[ScheduleEntry<S::Command>],
    checkpoints: &[Checkpoint],
) -> Result<(), RunnerError> {
    let start = match start {
        Start::New(_) => {
            Origin::Config(config.ok_or_else(|| invalid("missing replay configuration"))?)
        }
        Start::Snapshot(data, _) => Origin::Snapshot(container::encode(
            SNAPSHOT,
            data.header.clone(),
            &data.payload,
        )?),
    };
    let end = checkpoints.last().map_or(header.tick, |p| p.tick);
    let commands = schedule
        .iter()
        .filter(|entry| (header.tick..end).contains(&entry.tick) && !entry.commands.is_empty())
        .map(|entry| {
            Ok(CommandGroup {
                tick: entry.tick,
                commands: encode_commands(&entry.commands)?,
            })
        })
        .collect::<Result<_, RunnerError>>()?;
    let replay = Replay {
        start,
        target: opts.ticks,
        hash_interval: opts.hash_interval,
        commands,
        checkpoints: checkpoints
            .iter()
            .map(|p| RecordedCheckpoint {
                tick: p.tick,
                hash: p.hash,
                subsystems: p
                    .subsystems
                    .iter()
                    .map(|&(name, hash)| (name.into(), hash))
                    .collect(),
            })
            .collect(),
    };
    let payload = postcard::to_allocvec(&replay).map_err(|e| invalid(e.to_string()))?;
    container::write(path, &container::encode(REPLAY, header, &payload)?)
}

pub fn replay_game(path: &Path) -> Result<String, RunnerError> {
    Ok(container::read(REPLAY, path)?.header.game)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        run_simulation,
        tests::{StableSim, options},
    };

    #[test]
    fn changed_command_with_recomputed_checksum_diverges() {
        let dir =
            std::env::temp_dir().join(format!("terrarium-replay-mutation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let schedule = dir.join("schedule.json");
        std::fs::write(&schedule, r#"{"schedule":[{"tick":1,"commands":[2]}]}"#).unwrap();
        let replay = dir.join("case.simr");
        let mut opts = options();
        opts.commands = Some(schedule);
        opts.artifacts.record_replay = Some(replay.clone());
        run_simulation::<StableSim>(&opts).unwrap();

        let data = container::read(REPLAY, &replay).unwrap();
        let mut payload: Replay = postcard::from_bytes(&data.payload).unwrap();
        payload.commands[0].commands = encode_commands(&[5usize]).unwrap();
        let encoded = postcard::to_allocvec(&payload).unwrap();
        container::write(
            &replay,
            &container::encode(REPLAY, data.header, &encoded).unwrap(),
        )
        .unwrap();
        opts.commands = None;
        opts.artifacts.record_replay = None;
        opts.artifacts.replay = Some(replay);
        assert!(matches!(
            run_simulation::<StableSim>(&opts),
            Err(RunnerError::ReplayMismatch { from: 0, to: 2, .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
