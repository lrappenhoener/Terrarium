use std::{path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    for directory in [
        "../../crates",
        "../../games",
        "../../Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={directory}");
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(path) = git(&["symbolic-ref", "-q", "HEAD"])
        .and_then(|reference| git(&["rev-parse", "--git-path", &reference]))
        && Path::new(&path).exists()
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let id = git(&["rev-parse", "HEAD"]).map_or_else(
        || "unknown".to_owned(),
        |mut id| {
            if git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|status| !status.is_empty())
            {
                id.push_str("-dirty");
            }
            id
        },
    );
    println!("cargo:rustc-env=SIM_IO_BUILD_ID={id}");
}
