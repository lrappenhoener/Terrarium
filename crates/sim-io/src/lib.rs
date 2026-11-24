//! Game-independent file formats: snapshot containers, crash-safe replays and
//! configuration merging. Depends only on `sim-host`, so clients can read and
//! write these files without the runner and its game registry.

pub mod config;
pub mod container;
pub mod input;
pub mod replay;

use std::{io, path::Path, path::PathBuf};

use sim_host::SimError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error ({context}): {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("cannot parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("unsupported input format for {} (expected .toml or .json)", .0.display())]
    UnsupportedFormat(PathBuf),
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error("invalid options: {0}")]
    InvalidOptions(String),
}

pub fn io_at(path: &Path, source: io::Error) -> Error {
    Error::Io {
        context: path.display().to_string(),
        source,
    }
}
