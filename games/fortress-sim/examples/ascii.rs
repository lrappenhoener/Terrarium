//! Prints one level of a fortress snapshot: `ascii <snapshot> <z>`.
//! The snapshot is a runner container: magic, version, header length, JSON header
//! and the game payload.

use std::process::ExitCode;

use fortress_sim::FortressSim;
use sim_host::Simulation;

fn run() -> Result<String, String> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(z)) = (args.next(), args.next()) else {
        return Err("usage: ascii <snapshot> <z>".into());
    };
    let z: i32 = z.parse().map_err(|e| format!("invalid level: {e}"))?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    let prefix = bytes.get(..14).ok_or("truncated snapshot")?;
    if &prefix[..8] != b"SIMSNAP\0" {
        return Err("not a snapshot container".into());
    }
    let header = u32::from_le_bytes([prefix[10], prefix[11], prefix[12], prefix[13]]) as usize;
    let payload = bytes
        .get(14 + header..)
        .ok_or("truncated snapshot header")?;
    let sim = FortressSim::load_snapshot(payload).map_err(|e| e.to_string())?;
    sim.render_level(z)
        .ok_or_else(|| format!("level {z} is outside the world"))
}

fn main() -> ExitCode {
    match run() {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
