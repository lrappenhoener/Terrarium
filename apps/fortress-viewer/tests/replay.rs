// Integration tests may unwrap, as in unit tests.
#![allow(clippy::unwrap_used)]

//! A scripted viewer session is recorded and reproduced by the runner.

use std::path::{Path, PathBuf};

use fortress_sim::{BuildKind, DigMode, FortressSim};
use fortress_viewer::{
    script::{ScriptAction, ScriptStep, run_script},
    session::{Session, SessionConfig},
    tools::Tool,
};
use sim_core::space::Pos3;
use sim_runner::{ArtifactOptions, ReportFormat, RunOptions, TelemetryOptions, run_simulation};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("terrarium-viewer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn replay_options(replay: &Path) -> RunOptions {
    RunOptions {
        game: "fortress".into(),
        list_games: false,
        ticks: 0,
        seed: 0,
        config: None,
        commands: None,
        verify_determinism: false,
        hash_interval: 100,
        check_invariants_every: 100,
        report_interval: None,
        dump_hashes: None,
        report_format: ReportFormat::Text,
        overrides: vec![],
        budget_p99_us: None,
        telemetry: TelemetryOptions::default(),
        artifacts: ArtifactOptions {
            replay: Some(replay.into()),
            ..ArtifactOptions::default()
        },
    }
}

fn script() -> Vec<ScriptStep> {
    let drag = |tick, tool, from, to| ScriptStep {
        tick,
        action: ScriptAction::Drag { tool, from, to },
    };
    vec![
        drag(
            0,
            Tool::Stockpile,
            Pos3::new(1, 30, 14),
            Pos3::new(8, 40, 14),
        ),
        drag(
            3,
            Tool::Dig(DigMode::Dig),
            Pos3::new(14, 47, 14),
            Pos3::new(30, 49, 14),
        ),
        drag(
            3,
            Tool::Dig(DigMode::StairDown),
            Pos3::new(31, 48, 14),
            Pos3::new(31, 48, 14),
        ),
        drag(
            3,
            Tool::Dig(DigMode::StairUpDown),
            Pos3::new(31, 48, 13),
            Pos3::new(31, 48, 10),
        ),
        drag(
            250,
            Tool::Build(BuildKind::Wall),
            Pos3::new(16, 47, 14),
            Pos3::new(16, 47, 14),
        ),
        drag(
            400,
            Tool::Cancel,
            Pos3::new(28, 47, 14),
            Pos3::new(30, 49, 14),
        ),
    ]
}

#[test]
fn scripted_session_replays_identically_in_the_runner() {
    let dir = temp_dir("script");
    let mut session = Session::start(SessionConfig::new(42, dir.clone(), "script".into())).unwrap();
    run_script(&mut session, &script(), 777).unwrap();
    let hash = sim_host::Simulation::state_hash(session.sim());
    assert!(session.sim().view().counts().2 > 0 || hash != 0);
    let replays = session.finish().unwrap();
    assert_eq!(replays.len(), 1);
    let report = run_simulation::<FortressSim>(&replay_options(&replays[0])).unwrap();
    assert!(report.replay_verified);
    assert_eq!(report.tick, 777);
    assert_eq!(report.state_hash, format!("{hash:#018x}"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn loading_continues_the_recording_from_an_embedded_snapshot() {
    let dir = temp_dir("load");
    let mut config = SessionConfig::new(42, dir.clone(), "load".into());
    config.autosave_every = 150;
    let mut session = Session::start(config).unwrap();
    run_script(&mut session, &script(), 160).unwrap();
    let autosave = dir.join("load-autosave.snap");
    assert!(autosave.exists());
    let saved = session.last_save.clone().unwrap();
    assert_eq!(saved.tick, 150);
    assert!(saved.bytes > 0);
    let manual = dir.join("manual.snap");
    session.save(&manual).unwrap();
    run_script(&mut session, &[], 40).unwrap();
    session.load(&manual).unwrap();
    assert_eq!(session.sim().view().tick(), 160);
    run_script(&mut session, &script(), 300).unwrap();
    let hash = sim_host::Simulation::state_hash(session.sim());
    let replays = session.finish().unwrap();
    assert_eq!(replays.len(), 2);
    for replay in &replays {
        let report = run_simulation::<FortressSim>(&replay_options(replay)).unwrap();
        assert!(report.replay_verified, "{}", replay.display());
    }
    let last = run_simulation::<FortressSim>(&replay_options(&replays[1])).unwrap();
    assert_eq!(last.tick, 460);
    assert_eq!(last.state_hash, format!("{hash:#018x}"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "measurement, run with --release --ignored --nocapture"]
fn measure_save_load_and_autosave() {
    let dir = temp_dir("measure");
    let mut config = SessionConfig::new(42, dir.clone(), "measure".into());
    config.scenario = Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../games/fortress-sim/data/scenario_dig_in.toml"),
    );
    let mut session = Session::start(config).unwrap();
    for _ in 0..20_000 {
        session.step().unwrap();
        if session.sim().view().tick().is_multiple_of(6_000) {
            let save = session.last_save.clone().unwrap();
            println!(
                "autosave tick {} bytes {} ms {:.2}",
                save.tick, save.bytes, save.millis
            );
        }
    }
    let path = dir.join("final.snap");
    for _ in 0..3 {
        let save = session.save(&path).unwrap();
        let load_ms = session.load(&path).unwrap();
        println!(
            "save tick {} bytes {} ms {:.2} load ms {load_ms:.2}",
            save.tick, save.bytes, save.millis
        );
    }
    session.finish().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
