//! Replay playback checks and recording on top of the `sim-io` replay format.

use std::path::Path;

use sim_host::Simulation;
pub(crate) use sim_io::replay::Ending;
use sim_io::replay::{Origin, RecordSettings, ReplayFile, ReplayWriter};

use crate::{
    Checkpoint, RunnerError, Start,
    container::{self, Header, SNAPSHOT, invalid},
    input::ScheduleEntry,
    options::ConfigText,
};

pub(crate) struct Playback {
    pub header: Header,
    file: ReplayFile,
}

impl Playback {
    pub fn read(path: &Path) -> Result<Self, RunnerError> {
        let file = ReplayFile::read(path)?;
        Ok(Self {
            header: file.header.clone(),
            file,
        })
    }

    pub fn status(&self) -> String {
        self.file.status()
    }

    pub fn options(&self, opts: &mut crate::RunOptions) {
        opts.seed = self.header.seed;
        opts.ticks = self.file.target();
        opts.hash_interval = self.file.hash_interval;
    }

    pub fn start<S: Simulation>(&self) -> Result<Start<S::Config>, RunnerError> {
        match &self.file.origin {
            Origin::Config(text) => Ok(text.decode().map(Start::New)?),
            Origin::Snapshot(bytes) => {
                let data = sim_io::container::decode(SNAPSHOT, bytes)?;
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
        Ok(self.file.schedule()?)
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
        for (index, expected) in self.file.checkpoints.iter().enumerate() {
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
        if !self.file.partial()
            && let Some(extra) = actual.get(self.file.checkpoints.len())
        {
            return Err(RunnerError::ReplayMismatch {
                from: previous,
                to: extra.tick,
                subsystems: Vec::new(),
            });
        }
        Ok(())
    }

    pub fn explain_panic(&self, tick: u64) -> &'static str {
        self.file.explain_panic(tick)
    }
}

/// Opens a replay for the run's start, before the first step.
pub(crate) fn create_writer<S: Simulation>(
    path: &Path,
    opts: &crate::RunOptions,
    start: &Start<S::Config>,
    config: Option<&ConfigText>,
    header: Header,
) -> Result<ReplayWriter, RunnerError> {
    let origin = match start {
        Start::New(_) => Origin::Config(
            config
                .cloned()
                .ok_or_else(|| invalid("missing replay configuration"))?,
        ),
        Start::Snapshot(data, _) => Origin::Snapshot(sim_io::container::encode(
            SNAPSHOT,
            data.header.clone(),
            &data.payload,
        )?),
    };
    Ok(ReplayWriter::create(
        path,
        header,
        origin,
        &RecordSettings {
            target: opts.ticks,
            hash_interval: opts.hash_interval,
            flush_every: opts.artifacts.replay_flush_every,
            max_bytes: opts.artifacts.replay_max_bytes,
        },
    )?)
}

pub fn replay_game(path: &Path) -> Result<String, RunnerError> {
    Ok(sim_io::replay::replay_game(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::{
        RunOptions, run_simulation,
        tests::{StableSim, options},
    };
    use sim_io::replay::{Incomplete, Record, encode_commands, read_records, write_records};

    fn rewrite(path: &Path, mut edit: impl FnMut(&mut Record)) {
        let (header, records) = read_records(path).unwrap();
        assert!(records.incomplete.is_none());
        let mut records = records.records;
        for record in &mut records {
            edit(record);
        }
        write_records(path, &header, &records).unwrap();
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
        let full = ReplayFile::read(&replay).unwrap();
        assert!(matches!(
            full.ending,
            Some(Ending::Finished { tick: 500, .. })
        ));
        assert_eq!(play(&replay).unwrap().replay_status.unwrap(), full.status());

        let bytes = std::fs::read(&replay).unwrap();
        // Remove the end record and cut the 500 checkpoint in half.
        let cut = bytes.len() - 40;
        std::fs::write(&replay, &bytes[..cut]).unwrap();
        let partial = ReplayFile::read(&replay).unwrap();
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
        let playback = ReplayFile::read(&limited).unwrap();
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
        let playback = ReplayFile::read(&replay).unwrap();
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
