//! Format shared by snapshots and replays.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use sim_host::{SimError, Simulation, StateHasher};

use crate::{RunnerError, io_at};

pub(crate) const SNAPSHOT: [u8; 8] = *b"SIMSNAP\0";
pub(crate) const REPLAY: [u8; 8] = *b"SIMRPLY\0";
const VERSION: u16 = 1;
const BUILD_ID: &str = env!("SIM_RUNNER_BUILD_ID");
const MAX_HEADER: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Header {
    pub game: String,
    pub snapshot_version: u32,
    pub sim_host_version: String,
    pub build_id: String,
    pub content_fingerprint: u64,
    pub tick: u64,
    pub seed: u64,
    pub state_hash: u64,
    pub subsystems: Vec<(String, u64)>,
    pub payload_len: u64,
    pub payload_checksum: u64,
}

impl Header {
    pub fn capture<S: Simulation>(game: &str, seed: u64, sim: &S) -> Self {
        Self {
            game: game.into(),
            snapshot_version: S::snapshot_version(),
            sim_host_version: sim_host::VERSION.into(),
            build_id: BUILD_ID.into(),
            content_fingerprint: sim.content_fingerprint(),
            tick: sim.tick(),
            seed,
            state_hash: sim.state_hash(),
            subsystems: sim
                .subsystem_hashes()
                .into_iter()
                .map(|(name, hash)| (name.into(), hash))
                .collect(),
            payload_len: 0,
            payload_checksum: 0,
        }
    }

    pub fn warn_build(&self, writer: &mut dyn std::io::Write) -> Result<(), RunnerError> {
        if self.build_id != BUILD_ID {
            writeln!(
                writer,
                "Hinweis: Datei-Build {}, Runner-Build {}",
                self.build_id, BUILD_ID
            )
            .map_err(|source| RunnerError::Io {
                context: "stderr".into(),
                source,
            })?;
        }
        Ok(())
    }
}

pub(crate) struct Container {
    pub header: Header,
    pub payload: Vec<u8>,
    pub archive_len: u64,
}

pub(crate) fn invalid(message: impl Into<String>) -> RunnerError {
    SimError::Snapshot(message.into()).into()
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = StateHasher::new();
    hash.write_bytes(bytes);
    hash.finish()
}

pub(crate) fn checksum_bytes(bytes: &[u8]) -> u64 {
    checksum(bytes)
}

/// Magic, format version, header length and JSON header shared by both file kinds.
pub(crate) fn encode_prefix(
    magic: [u8; 8],
    version: u16,
    header: &Header,
) -> Result<Vec<u8>, RunnerError> {
    let metadata = serde_json::to_vec(header).map_err(|e| invalid(e.to_string()))?;
    if metadata.len() > MAX_HEADER {
        return Err(invalid("container header too large"));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&magic);
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&metadata);
    Ok(bytes)
}

pub(crate) fn decode_prefix(
    magic: [u8; 8],
    version: u16,
    bytes: &[u8],
) -> Result<(Header, &[u8]), RunnerError> {
    let (prefix, rest) = bytes
        .split_first_chunk::<14>()
        .ok_or_else(|| invalid("truncated container prefix"))?;
    if prefix[..8] != magic {
        return Err(invalid("incorrect container magic"));
    }
    if u16::from_le_bytes([prefix[8], prefix[9]]) != version {
        return Err(invalid("unsupported container version"));
    }
    let header_len = u32::from_le_bytes([prefix[10], prefix[11], prefix[12], prefix[13]]) as usize;
    let (header, payload) = rest
        .split_at_checked(header_len)
        .filter(|_| header_len <= MAX_HEADER)
        .ok_or_else(|| invalid("invalid container header length"))?;
    let header: Header = serde_json::from_slice(header).map_err(|e| invalid(e.to_string()))?;
    Ok((header, payload))
}

pub(crate) fn encode(
    magic: [u8; 8],
    mut header: Header,
    payload: &[u8],
) -> Result<Vec<u8>, RunnerError> {
    header.payload_len = payload.len() as u64;
    header.payload_checksum = checksum(payload);
    let mut bytes = encode_prefix(magic, VERSION, &header)?;
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

pub(crate) fn decode(magic: [u8; 8], bytes: &[u8]) -> Result<Container, RunnerError> {
    let (header, payload) = decode_prefix(magic, VERSION, bytes)?;
    if header.payload_len != payload.len() as u64 || header.payload_checksum != checksum(payload) {
        return Err(invalid("payload length or checksum mismatch"));
    }
    Ok(Container {
        header,
        payload: payload.to_vec(),
        archive_len: bytes.len() as u64,
    })
}

pub(crate) fn read(magic: [u8; 8], path: &Path) -> Result<Container, RunnerError> {
    let bytes = fs::read(path).map_err(|err| io_at(path, err))?;
    decode(magic, &bytes)
}

pub(crate) fn require_snapshots<S: Simulation>() -> Result<(), RunnerError> {
    if S::snapshot_version() == 0 {
        return Err(SimError::Unsupported("snapshots").into());
    }
    Ok(())
}

pub(crate) fn restore<S: Simulation>(game: &str, snapshot: &Container) -> Result<S, RunnerError> {
    require_snapshots::<S>()?;
    let header = &snapshot.header;
    if header.game != game {
        return Err(invalid(format!(
            "snapshot is for {}, selected game is {game}",
            header.game
        )));
    }
    if !(S::min_snapshot_version()..=S::snapshot_version()).contains(&header.snapshot_version) {
        return Err(invalid(format!(
            "snapshot version {} is incompatible with {}",
            header.snapshot_version,
            S::snapshot_version()
        )));
    }
    let sim = S::load_snapshot(&snapshot.payload)?;
    for warning in sim.load_warnings() {
        let _ = std::io::Write::write_all(
            &mut std::io::stderr().lock(),
            format!("Warnung: {warning}\n").as_bytes(),
        );
    }
    if sim.tick() != header.tick {
        return Err(invalid(format!(
            "loaded tick {}, expected {}",
            sim.tick(),
            header.tick
        )));
    }
    let actual = sim.state_hash();
    let subsystems = differing(
        header
            .subsystems
            .iter()
            .map(|(name, hash)| (name.as_str(), *hash)),
        sim.subsystem_hashes(),
    );
    if actual != header.state_hash || !subsystems.is_empty() {
        return Err(invalid(format!(
            "loaded state hash {actual:#018x} differs from {:#018x}; differing subsystems: {}",
            header.state_hash,
            subsystems.join(", ")
        )));
    }
    if sim.content_fingerprint() != header.content_fingerprint {
        return Err(invalid("loaded content fingerprint differs from snapshot"));
    }
    Ok(sim)
}

pub(crate) fn differing<'a>(
    first: impl IntoIterator<Item = (&'a str, u64)>,
    second: impl IntoIterator<Item = (&'a str, u64)>,
) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let first: BTreeMap<_, _> = first.into_iter().collect();
    let second: BTreeMap<_, _> = second.into_iter().collect();
    first
        .keys()
        .chain(second.keys())
        .copied()
        .filter(|name| first.get(name) != second.get(name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(crate) fn snapshot_bytes<S: Simulation>(
    game: &str,
    seed: u64,
    sim: &S,
) -> Result<Vec<u8>, RunnerError> {
    require_snapshots::<S>()?;
    let mut payload = Vec::new();
    sim.save_snapshot(&mut payload)?;
    encode(SNAPSHOT, Header::capture(game, seed, sim), &payload)
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<(), RunnerError> {
    use std::{
        fs::OpenOptions,
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static FILE_ID: AtomicU64 = AtomicU64::new(0);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        FILE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = std::path::PathBuf::from(name);
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|err| io_at(&temp, err))?;
    let result = out.write_all(bytes).and_then(|()| out.sync_all());
    drop(out);
    if let Err(err) = result.and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(io_at(path, err));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_host::SimMetrics;

    pub(crate) struct Broken(pub u64);

    impl Simulation for Broken {
        type Config = ();
        type Command = ();
        fn new(_: u64, _: ()) -> Result<Self, SimError> {
            Ok(Self(7))
        }
        fn step(&mut self, _: &[()]) {
            self.0 += 1;
        }
        fn tick(&self) -> u64 {
            self.0
        }
        fn is_finished(&self) -> bool {
            false
        }
        fn metrics(&self) -> SimMetrics {
            SimMetrics {
                active_entities: 0,
                custom_gauges: Vec::new(),
            }
        }
        fn state_hash(&self) -> u64 {
            self.0
        }
        fn subsystem_hashes(&self) -> Vec<(&'static str, u64)> {
            vec![("discarded", self.0)]
        }
        fn check_invariants(&self) -> Result<(), SimError> {
            Ok(())
        }
        fn snapshot_version() -> u32 {
            1
        }
        fn save_snapshot(&self, out: &mut Vec<u8>) -> Result<(), SimError> {
            out.extend_from_slice(&[0]); // deliberately omits the state
            Ok(())
        }
        fn load_snapshot(_: &[u8]) -> Result<Self, SimError> {
            Ok(Self(7))
        }
    }

    #[test]
    fn snapshot_container_roundtrip_and_validation() {
        let bytes = snapshot_bytes("test", 42, &Broken(7)).unwrap();
        let mut decoded = decode(SNAPSHOT, &bytes).unwrap();
        assert_eq!(decoded.header.seed, 42);
        assert_eq!(decoded.header.tick, 7);
        assert_eq!(restore::<Broken>("test", &decoded).unwrap().0, 7);
        assert!(
            restore::<Broken>("other", &decoded)
                .err()
                .unwrap()
                .to_string()
                .contains("selected game")
        );
        decoded.header.snapshot_version += 1;
        assert!(
            restore::<Broken>("test", &decoded)
                .err()
                .unwrap()
                .to_string()
                .contains("version")
        );
        let mut corrupt = bytes;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(
            decode(SNAPSHOT, &corrupt)
                .err()
                .unwrap()
                .to_string()
                .contains("checksum")
        );
        assert!(decode(SNAPSHOT, &corrupt[..8]).is_err());
    }

    #[test]
    fn save_load_detects_an_immediate_difference() {
        let error = crate::reload(&crate::tests::options(), Broken(8))
            .err()
            .unwrap();
        assert!(matches!(
            error,
            RunnerError::SaveLoadMismatch { tick: 8, .. }
        ));
        assert!(error.to_string().contains("immediately after loading"));
    }

    #[test]
    fn lost_state_names_differing_subsystems() {
        let mut header = Header::capture("test", 42, &Broken(7));
        header.state_hash = 8;
        header.subsystems[0].1 = 8;
        let bytes = encode(SNAPSHOT, header, &[0]).unwrap();
        let data = decode(SNAPSHOT, &bytes).unwrap();
        let error = restore::<Broken>("test", &data).err().unwrap().to_string();
        assert!(error.contains("state hash") && error.contains("discarded"));
    }
}
