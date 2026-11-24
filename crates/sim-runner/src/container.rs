//! Snapshot container, provided by `sim-io`.

pub(crate) use sim_io::container::{
    Container, Header, SNAPSHOT, differing, read, require_snapshots, restore, snapshot_bytes, write,
};

use crate::RunnerError;

pub(crate) fn invalid(message: impl Into<String>) -> RunnerError {
    sim_host::SimError::Snapshot(message.into()).into()
}
