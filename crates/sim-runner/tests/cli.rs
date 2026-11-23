use std::{fs, path::Path, process::Command};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sim-runner"))
}

fn data_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/dummy-sim/data")
}

#[test]
fn verifies_dummy_with_example_files_and_outputs_json() {
    let data = data_dir();
    let output = cli()
        .args([
            "--ticks",
            "1501",
            "--verify-determinism",
            "--report-format",
            "json",
            "--report-interval",
            "1000",
        ])
        .arg("--config")
        .arg(data.join("config.toml"))
        .arg("--commands")
        .arg(data.join("commands.toml"))
        .output()
        .unwrap();

    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{diagnostics}");
    assert_eq!(diagnostics.lines().count(), 1);
    assert!(diagnostics.contains("Tick 1000"));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["tick"], 1501);
    assert_eq!(report["verified_checkpoints"], 16);
    assert_eq!(report["state_hash"].as_str().unwrap().len(), 18);
    let gauges = report["custom_gauges"].as_array().unwrap();
    assert!(
        gauges
            .iter()
            .any(|gauge| gauge[0] == "spawned_total" && gauge[1] == 12000.0)
    );
    assert!(
        gauges
            .iter()
            .any(|gauge| gauge[0] == "energy_drain" && gauge[1] == 0.8)
    );
}

#[test]
fn zero_ticks_dumps_final_checkpoint_and_warns_about_commands() {
    let csv = std::env::temp_dir().join(format!("sim-runner-hashes-{}.csv", std::process::id()));
    let output = cli()
        .args([
            "--ticks",
            "0",
            "--hash-interval",
            "0",
            "--verify-determinism",
            "--dump-hashes",
        ])
        .arg(&csv)
        .arg("--commands")
        .arg(data_dir().join("commands.toml"))
        .output()
        .unwrap();

    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{diagnostics}");
    assert_eq!(diagnostics.lines().count(), 2);
    assert!(diagnostics.contains("Tick 500") && diagnostics.contains("Tick 1500"));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let hash = stdout
        .lines()
        .find_map(|line| line.strip_prefix("state_hash "))
        .unwrap();
    let exported = fs::read_to_string(&csv).unwrap();
    fs::remove_file(csv).unwrap();
    assert_eq!(exported, format!("tick,hash_hex\n0,{hash}\n"));
    assert!(stdout.contains("Determinismus OK (1 Checkpoints)"));
}

#[test]
fn lists_games_and_uses_the_general_failure_exit_code() {
    let output = cli().arg("--list-games").output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|name| name == "dummy")
    );

    for args in [["--game", "invalid"], ["--ticks", "invalid"]] {
        let output = cli().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn jungle_sweep_exports_each_run_and_uses_nested_overrides() {
    let output = cli()
        .args([
            "--game",
            "jungle",
            "--ticks",
            "8",
            "--set",
            "world_size=16",
            "--set",
            "initial_predators=2",
            "--set",
            "predator.perception_radius=8",
            "--sweep",
            "initial_herbivores=3,5",
            "--profile-phases",
            "--max-step-allocs",
            "0",
            "--alloc-warmup",
            "0",
            "--report-format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sweep: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sweep["key"], "initial_herbivores");
    assert_eq!(sweep["rows"][0]["active_entities"], 5);
    assert_eq!(sweep["rows"][1]["active_entities"], 7);
    assert_eq!(sweep["rows"][0]["step_allocations_total"], 0);
    assert_eq!(sweep["rows"][1]["phases"].as_array().unwrap().len(), 7);

    let typo = cli()
        .args(["--game", "jungle", "--set", "predator.typo=8"])
        .output()
        .unwrap();
    assert_eq!(typo.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&typo.stderr).contains("typo"));
}

#[test]
fn sweep_zips_columns_and_rejects_unequal_lengths() {
    let run = |populations| {
        cli()
            .args([
                "--ticks",
                "1",
                "--sweep",
                "world_width=10,20",
                "--sweep",
                populations,
                "--report-format",
                "json",
            ])
            .output()
            .unwrap()
    };
    let output = run("initial_units=3,5");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["keys"],
        serde_json::json!(["world_width", "initial_units"])
    );
    assert_eq!(report["rows"].as_array().unwrap().len(), 2);
    assert_eq!(report["rows"][0]["active_entities"], 3);
    assert_eq!(report["rows"][1]["active_entities"], 5);
    let invalid = run("initial_units=3");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("equal lengths"));
}

#[test]
fn exceeding_p99_budget_exits_with_code_five() {
    let result = cli()
        .args(["--ticks", "2", "--budget-p99-us", "0"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(5));
}

#[test]
fn dummy_rejects_snapshot_flags_cleanly() {
    let result = cli()
        .args([
            "--game",
            "dummy",
            "--save-snapshot-at",
            "1",
            "--snapshot-dir",
            "unused",
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported feature: snapshots"));
}

#[test]
fn save_load_combines_with_determinism_and_reports_roundtrips() {
    let output = cli()
        .args([
            "--game",
            "jungle",
            "--ticks",
            "68",
            "--set",
            "world_size=16",
            "--set",
            "initial_herbivores=10",
            "--set",
            "initial_predators=2",
            "--verify-save-load",
            "--verify-determinism",
            "--roundtrip-at",
            "1",
            "--roundtrip-at",
            "64",
            "--max-step-allocs",
            "0",
            "--report-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["verified_save_load"], 2);
    assert_eq!(report["verified_checkpoints"], 1);
}
