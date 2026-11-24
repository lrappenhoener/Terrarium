//! The simulation side of the viewer. A `Session` owns the simulation, applies
//! commands at the tick they are stepped, records every session as a replay and
//! saves or loads snapshots between ticks. `spawn` runs it in its own thread.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use fortress_sim::{FortressCommand, FortressConfig, FortressSim};
use sim_core::space::Pos3;
use sim_host::Simulation;
use sim_io::{
    config::ConfigText,
    container::{self, Header, SNAPSHOT},
    input::{ScheduleEntry, load_schedule},
    replay::{Ending, Origin, RecordSettings, ReplayWriter},
};
use thiserror::Error;

use crate::{frame::Mailbox, tools::Speed};

pub const GAME: &str = "fortress";

#[derive(Debug, Error)]
pub enum ViewerError {
    #[error(transparent)]
    Io(#[from] sim_io::Error),
    #[error(transparent)]
    Sim(#[from] sim_host::SimError),
    #[error("simulation panicked at tick {tick}: {message}")]
    Panic { tick: u64, message: String },
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub seed: u64,
    pub config: FortressConfig,
    pub scenario: Option<PathBuf>,
    pub load: Option<PathBuf>,
    /// Directory for replays and saves.
    pub session_dir: PathBuf,
    /// Base name of this session's files, typically a wall-clock timestamp.
    pub name: String,
    pub hash_interval: u64,
    /// Ticks between autosaves; zero disables them.
    pub autosave_every: u64,
}

impl SessionConfig {
    pub fn new(seed: u64, session_dir: PathBuf, name: String) -> Self {
        Self {
            seed,
            config: FortressConfig::default(),
            scenario: None,
            load: None,
            session_dir,
            name,
            hash_interval: 100,
            // Five minutes at the base rate of 20 ticks per second.
            autosave_every: 6_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveReport {
    pub path: PathBuf,
    pub tick: u64,
    pub bytes: u64,
    pub millis: f64,
}

pub struct Session {
    sim: FortressSim,
    seed: u64,
    schedule: Vec<ScheduleEntry<FortressCommand>>,
    next_scheduled: usize,
    pending: Vec<FortressCommand>,
    commands: Vec<FortressCommand>,
    writer: Option<ReplayWriter>,
    replays: Vec<PathBuf>,
    config: SessionConfig,
    segment: u32,
    durations: VecDeque<u64>,
    /// All step durations of this session, for summaries.
    history: Vec<u64>,
    pub last_save: Option<SaveReport>,
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".into())
}

impl Session {
    pub fn start(config: SessionConfig) -> Result<Self, ViewerError> {
        std::fs::create_dir_all(&config.session_dir)
            .map_err(|err| sim_io::io_at(&config.session_dir, err))?;
        let (sim, origin) = match &config.load {
            Some(path) => {
                let data = container::read(SNAPSHOT, path)?;
                let sim = container::restore::<FortressSim>(GAME, &data)?;
                let origin =
                    Origin::Snapshot(container::encode(SNAPSHOT, data.header, &data.payload)?);
                (sim, origin)
            }
            None => {
                let text = serde_json::to_string(&config.config)
                    .map_err(|err| ViewerError::Other(err.to_string()))?;
                (
                    FortressSim::new(config.seed, config.config.clone())?,
                    Origin::Config(ConfigText::Json(text)),
                )
            }
        };
        let seed = sim.view().seed();
        let schedule = match &config.scenario {
            Some(path) => load_schedule(path, u64::MAX, &mut std::io::sink())?,
            None => Vec::new(),
        };
        let next_scheduled = schedule.partition_point(|entry| entry.tick < sim.tick());
        let mut session = Self {
            sim,
            seed,
            schedule,
            next_scheduled,
            pending: Vec::new(),
            commands: Vec::new(),
            writer: None,
            replays: Vec::new(),
            config,
            segment: 0,
            durations: VecDeque::with_capacity(1000),
            history: Vec::with_capacity(1 << 16),
            last_save: None,
        };
        session.open_replay(origin)?;
        Ok(session)
    }

    fn open_replay(&mut self, origin: Origin) -> Result<(), ViewerError> {
        let suffix = if self.segment == 0 {
            String::new()
        } else {
            format!("-{}", self.segment)
        };
        self.segment += 1;
        let path = self
            .config
            .session_dir
            .join(format!("{}{suffix}.simr", self.config.name));
        let header = Header::capture(GAME, self.seed, &self.sim);
        self.writer = Some(ReplayWriter::create(
            &path,
            header,
            origin,
            &RecordSettings {
                target: u64::MAX,
                hash_interval: self.config.hash_interval,
                flush_every: 100,
                max_bytes: None,
            },
        )?);
        self.replays.push(path);
        Ok(())
    }

    fn close_replay(&mut self, ending: Ending) -> Result<(), ViewerError> {
        if let Some(mut writer) = self.writer.take() {
            let tick = self.sim.tick();
            if !matches!(ending, Ending::Panic { .. })
                && (self.config.hash_interval == 0
                    || !tick.is_multiple_of(self.config.hash_interval))
            {
                writer.checkpoint(tick, self.sim.state_hash(), &self.sim.subsystem_hashes())?;
            }
            writer.finish(tick, ending)?;
        }
        Ok(())
    }

    pub fn sim(&self) -> &FortressSim {
        &self.sim
    }

    pub fn replays(&self) -> &[PathBuf] {
        &self.replays
    }

    /// Queues commands for the next stepped tick.
    pub fn queue(&mut self, commands: impl IntoIterator<Item = FortressCommand>) {
        self.pending.extend(commands);
    }

    /// Applies scenario commands, then queued commands, and steps one tick.
    pub fn step(&mut self) -> Result<(), ViewerError> {
        let tick = self.sim.tick();
        self.commands.clear();
        while let Some(entry) = self.schedule.get(self.next_scheduled) {
            if entry.tick > tick {
                break;
            }
            if entry.tick == tick {
                self.commands.extend(entry.commands.iter().copied());
            }
            self.next_scheduled += 1;
        }
        self.commands.append(&mut self.pending);
        if let Some(writer) = &mut self.writer {
            writer.commands(tick, &self.commands)?;
        }
        let start = Instant::now();
        let sim = &mut self.sim;
        let commands = &self.commands;
        let result = catch_unwind(AssertUnwindSafe(|| sim.step(commands)));
        let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if let Err(payload) = result {
            let message = panic_message(payload.as_ref());
            self.close_replay(Ending::Panic {
                tick,
                message: message.clone(),
            })?;
            return Err(ViewerError::Panic { tick, message });
        }
        if self.durations.len() == 1000 {
            self.durations.pop_front();
        }
        self.durations.push_back(elapsed);
        self.history.push(elapsed);
        let now = self.sim.tick();
        if let Some(writer) = &mut self.writer {
            if self.config.hash_interval != 0 && now.is_multiple_of(self.config.hash_interval) {
                writer.checkpoint(now, self.sim.state_hash(), &self.sim.subsystem_hashes())?;
            }
            writer.tick_done(now)?;
        }
        if self.config.autosave_every != 0 && now.is_multiple_of(self.config.autosave_every) {
            let path = self
                .config
                .session_dir
                .join(format!("{}-autosave.snap", self.config.name));
            self.save(&path)?;
        }
        Ok(())
    }

    /// p99 of the last up to 1000 step durations in microseconds.
    pub fn p99_us(&self) -> f64 {
        percentile_us(self.durations.iter().copied().collect())
    }

    /// p99 over all steps of the session in microseconds.
    pub fn p99_all_us(&self) -> f64 {
        percentile_us(self.history.clone())
    }
}

fn percentile_us(mut values: Vec<u64>) -> f64 {
    {
        if values.is_empty() {
            return 0.0;
        }
        values.sort_unstable();
        let rank = (values.len() * 99).div_ceil(100).max(1) - 1;
        values[rank] as f64 / 1000.0
    }
}

impl Session {
    pub fn save(&mut self, path: &Path) -> Result<SaveReport, ViewerError> {
        let begin = Instant::now();
        let bytes = container::snapshot_bytes(GAME, self.seed, &self.sim)?;
        container::write(path, &bytes)?;
        let report = SaveReport {
            path: path.into(),
            tick: self.sim.tick(),
            bytes: bytes.len() as u64,
            millis: begin.elapsed().as_secs_f64() * 1000.0,
        };
        self.last_save = Some(report.clone());
        Ok(report)
    }

    /// Replaces the simulation with a saved state and continues the recording in a
    /// new replay that embeds the snapshot as its start.
    pub fn load(&mut self, path: &Path) -> Result<f64, ViewerError> {
        let begin = Instant::now();
        let data = container::read(SNAPSHOT, path)?;
        let sim = container::restore::<FortressSim>(GAME, &data)?;
        let origin = Origin::Snapshot(container::encode(SNAPSHOT, data.header, &data.payload)?);
        let tick = self.sim.tick();
        let hash = self.sim.state_hash();
        self.close_replay(Ending::Finished { tick, hash })?;
        self.sim = sim;
        self.seed = self.sim.view().seed();
        self.pending.clear();
        self.next_scheduled = self
            .schedule
            .partition_point(|entry| entry.tick < self.sim.tick());
        self.open_replay(origin)?;
        Ok(begin.elapsed().as_secs_f64() * 1000.0)
    }

    /// Closes the replay and returns the paths of all replay segments.
    pub fn finish(mut self) -> Result<Vec<PathBuf>, ViewerError> {
        let tick = self.sim.tick();
        let hash = self.sim.state_hash();
        self.close_replay(Ending::Finished { tick, hash })?;
        Ok(self.replays)
    }
}

/// Messages from the UI thread.
#[derive(Debug, Clone)]
pub enum Control {
    Commands(Vec<FortressCommand>),
    Speed(Speed),
    Step,
    Save(PathBuf),
    Load(PathBuf),
    Inspect(Option<u64>),
    Select(Option<u64>),
    Regions(Option<i32>),
    Temperature(Option<i32>),
    /// Stop when the tick is reached, for benchmarks.
    StopAt(u64),
    /// Pause when the tick is reached, for screenshots.
    PauseAt(u64),
    Quit,
}

/// Results the UI thread may print after the simulation thread ends.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub ticks: u64,
    pub step_seconds: f64,
    pub wall_seconds: f64,
    /// Sum of `step()` durations only.
    pub sim_seconds: f64,
    pub p99_us: f64,
    pub replays: Vec<PathBuf>,
    pub error: Option<String>,
}

struct Publisher {
    versions: Vec<u32>,
    fluid_versions: Vec<u32>,
    fluid_scratch: Vec<u8>,
    temperature: Option<i32>,
    scratch: Vec<sim_core::space::Tile>,
    selected: Option<u64>,
    inspect: Option<u64>,
    regions: Option<i32>,
    last: Option<Instant>,
}

impl Publisher {
    fn publish(
        &mut self,
        session: &Session,
        mailbox: &Mailbox,
        speed: Speed,
        tps: f64,
        force: bool,
    ) {
        // At high speed the UI cannot show every tick; publish at most every 15 ms.
        if !force
            && self
                .last
                .is_some_and(|last| last.elapsed() < Duration::from_millis(15))
        {
            return;
        }
        self.last = Some(Instant::now());
        let view = session.sim.view();
        let chunks = view.chunk_count();
        self.versions.resize(chunks, u32::MAX);
        let p99 = session.p99_us();
        mailbox.publish(|frame| {
            frame.dims = view.dims();
            frame.chunk_changes.resize(chunks, 0);
            for chunk in 0..chunks as u32 {
                let version = view.chunk_version(chunk);
                if self.versions[chunk as usize] == version {
                    continue;
                }
                if self.versions[chunk as usize] != u32::MAX {
                    frame.chunk_changes[chunk as usize] += 1;
                }
                self.versions[chunk as usize] = version;
                if let Some((low, high)) = view.chunk_bounds(chunk) {
                    let scratch = &mut self.scratch;
                    scratch.clear();
                    view.chunk_tiles(chunk, |_, tile| scratch.push(*tile));
                    frame.store_chunk(chunk, low, high, scratch.iter().copied());
                }
            }
            self.fluid_versions.resize(chunks, u32::MAX);
            for chunk in 0..chunks as u32 {
                let version = view.fluid_chunk_version(chunk);
                if self.fluid_versions[chunk as usize] == version {
                    continue;
                }
                self.fluid_versions[chunk as usize] = version;
                if let Some((low, high)) = view.chunk_bounds(chunk) {
                    let scratch = &mut self.fluid_scratch;
                    scratch.clear();
                    view.chunk_fluids(chunk, |_, value| scratch.push(*value));
                    frame.store_fluids(chunk, low, high, scratch.iter().copied());
                }
            }
            frame.activity.clear();
            frame
                .activity
                .extend((0..chunks as u32).map(|chunk| view.chunk_activity(chunk)));
            frame.support_pending = view.support_pending();
            frame.temperatures = self.temperature.map(|z| {
                let (w, h, _) = view.dims();
                (
                    z,
                    (0..h)
                        .flat_map(|y| (0..w).map(move |x| Pos3::new(x, y, z)))
                        .map(|p| view.temperature(p))
                        .collect(),
                )
            });
            let (residents, items, jobs) = view.counts();
            frame.stats = crate::frame::Stats {
                tick: view.tick(),
                speed: speed.label(),
                tps,
                p99_us: p99,
                residents,
                items,
                open_jobs: jobs,
            };
            view.residents(&mut frame.residents);
            view.items(&mut frame.items);
            view.jobs(&mut frame.jobs);
            frame.stockpiles.clear();
            frame.stockpiles.extend_from_slice(view.stockpiles());
            match self.selected {
                Some(id) => view.path(id, &mut frame.selected_path),
                None => frame.selected_path.clear(),
            }
            if let Some(id) = self.inspect.take() {
                frame.description = view.describe(id);
            }
            frame.regions = self.regions.map(|z| {
                let (w, h, _) = view.dims();
                (
                    z,
                    (0..h)
                        .flat_map(|y| (0..w).map(move |x| Pos3::new(x, y, z)))
                        .map(|p| view.debug_region(p))
                        .collect(),
                )
            });
        });
    }
}

/// Runs the session in its own thread. Drawing never influences the tick rate:
/// the thread paces itself by wall-clock targets and only publishes frames.
pub fn spawn(
    mut session: Session,
    mailbox: Arc<Mailbox>,
    base_tps: f64,
    initial: Speed,
) -> (Sender<Control>, JoinHandle<RunSummary>) {
    let (sender, receiver) = channel::<Control>();
    let handle = std::thread::spawn(move || {
        run(&mut session, &receiver, &mailbox, base_tps, initial).finish(session)
    });
    (sender, handle)
}

struct Outcome {
    ticks: u64,
    step_seconds: f64,
    wall_seconds: f64,
    error: Option<String>,
}

impl Outcome {
    fn finish(self, session: Session) -> RunSummary {
        let p99_us = session.p99_all_us();
        let sim_seconds = session.history.iter().sum::<u64>() as f64 / 1e9;
        let (replays, error) = match session.finish() {
            Ok(replays) => (replays, self.error),
            Err(err) => (
                Vec::new(),
                Some(self.error.unwrap_or_else(|| err.to_string())),
            ),
        };
        RunSummary {
            ticks: self.ticks,
            step_seconds: self.step_seconds,
            wall_seconds: self.wall_seconds,
            sim_seconds,
            p99_us,
            replays,
            error,
        }
    }
}

fn run(
    session: &mut Session,
    receiver: &Receiver<Control>,
    mailbox: &Mailbox,
    base_tps: f64,
    initial: Speed,
) -> Outcome {
    let mut speed = initial;
    let mut stop_at = None;
    let mut pause_at = None;
    let mut publisher = Publisher {
        versions: Vec::new(),
        fluid_versions: Vec::new(),
        fluid_scratch: Vec::new(),
        temperature: None,
        scratch: Vec::new(),
        selected: None,
        inspect: None,
        regions: None,
        last: None,
    };
    let mut ticks = 0u64;
    let mut step_seconds = 0.0;
    let mut window = (Instant::now(), 0u64, 0.0f64);
    let mut next_due = Instant::now();
    let mut single = false;
    let started = Instant::now();
    let fail = |error: ViewerError, ticks, step_seconds| Outcome {
        ticks,
        step_seconds,
        wall_seconds: started.elapsed().as_secs_f64(),
        error: Some(error.to_string()),
    };
    publisher.publish(session, mailbox, speed, 0.0, true);
    loop {
        loop {
            let message = match receiver.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Outcome {
                        ticks,
                        step_seconds,
                        wall_seconds: started.elapsed().as_secs_f64(),
                        error: None,
                    };
                }
            };
            match message {
                Control::Commands(commands) => session.queue(commands),
                Control::Speed(new) => {
                    speed = new;
                    next_due = Instant::now();
                }
                Control::Step => single = true,
                Control::Save(path) => {
                    let text = match session.save(&path) {
                        Ok(r) => format!(
                            "Gespeichert: {} ({} Bytes, {:.1} ms)",
                            r.path.display(),
                            r.bytes,
                            r.millis
                        ),
                        Err(err) => format!("Speichern fehlgeschlagen: {err}"),
                    };
                    mailbox.publish(|f| f.messages.push(text));
                    publisher.versions.clear();
                    publisher.fluid_versions.clear();
                }
                Control::Load(path) => {
                    let text = match session.load(&path) {
                        Ok(ms) => format!("Geladen: {} ({ms:.1} ms)", path.display()),
                        Err(err) => format!("Laden fehlgeschlagen: {err}"),
                    };
                    mailbox.publish(|f| f.messages.push(text));
                    publisher.versions.clear();
                    publisher.fluid_versions.clear();
                }
                Control::Inspect(id) => publisher.inspect = id,
                Control::Select(id) => publisher.selected = id,
                Control::Regions(level) => publisher.regions = level,
                Control::Temperature(level) => publisher.temperature = level,
                Control::StopAt(tick) => stop_at = Some(tick),
                Control::PauseAt(tick) => pause_at = Some(tick),
                Control::Quit => {
                    return Outcome {
                        ticks,
                        step_seconds,
                        wall_seconds: started.elapsed().as_secs_f64(),
                        error: None,
                    };
                }
            }
        }
        if stop_at.is_some_and(|tick| session.sim.tick() >= tick) {
            publisher.publish(session, mailbox, speed, window.2, true);
            mailbox.publish(|f| f.finished = true);
            return Outcome {
                ticks,
                step_seconds,
                wall_seconds: started.elapsed().as_secs_f64(),
                error: None,
            };
        }
        if pause_at.is_some_and(|tick| session.sim.tick() >= tick) {
            pause_at = None;
            speed = Speed::Paused;
            publisher.publish(session, mailbox, speed, window.2, true);
        }
        let rate = speed.ticks_per_second(base_tps);
        let due = match rate {
            None => true,
            Some(r) if r > 0.0 => Instant::now() >= next_due,
            Some(_) => false,
        };
        if due || single {
            single = false;
            let begin = Instant::now();
            if let Err(error) = session.step() {
                mailbox.publish(|f| f.messages.push(error.to_string()));
                return fail(error, ticks, step_seconds);
            }
            step_seconds += begin.elapsed().as_secs_f64();
            ticks += 1;
            window.1 += 1;
            if let Some(r) = rate.filter(|r| *r > 0.0) {
                next_due += Duration::from_secs_f64(1.0 / r);
                if next_due + Duration::from_millis(250) < Instant::now() {
                    next_due = Instant::now();
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
        let elapsed = window.0.elapsed().as_secs_f64();
        if elapsed >= 0.25 {
            window.2 = window.1 as f64 / elapsed;
            window = (Instant::now(), 0, window.2);
        }
        publisher.publish(session, mailbox, speed, window.2, false);
    }
}
