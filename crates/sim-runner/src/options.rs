//! Configuration merging, provided by `sim-io`.

pub(crate) use sim_io::config::{ConfigText, prepare_config};

use crate::RunnerError;

pub fn parse_sweep(text: &str) -> Result<(&str, Vec<toml::Value>), RunnerError> {
    Ok(sim_io::config::parse_sweep(text)?)
}
