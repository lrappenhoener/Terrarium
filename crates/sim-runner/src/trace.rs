use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sim_host::Simulation;

use crate::RunnerError;

pub(crate) struct Trace {
    path: PathBuf,
    writer: BufWriter<File>,
    live: Vec<u64>,
}

#[derive(Serialize)]
struct Record {
    tick: u64,
    entity: u64,
    fields: Option<BTreeMap<String, String>>,
}

fn io_error(path: &Path, source: std::io::Error) -> RunnerError {
    RunnerError::Io {
        context: path.display().to_string(),
        source,
    }
}

impl Trace {
    pub fn new(path: &Path, ids: &[u64]) -> Result<Self, RunnerError> {
        let mut live = ids.to_vec();
        live.sort_unstable();
        live.dedup();
        let writer = BufWriter::new(File::create(path).map_err(|error| io_error(path, error))?);
        Ok(Self {
            path: path.into(),
            writer,
            live,
        })
    }

    pub fn observe<S: Simulation>(&mut self, sim: &S) -> Result<(), RunnerError> {
        let mut output = Ok(());
        self.live.retain(|&entity| {
            if output.is_err() {
                return true;
            }
            let fields = sim
                .describe_entity(entity)
                .map(|fields| fields.into_iter().collect());
            let record = Record {
                tick: sim.tick(),
                entity,
                fields,
            };
            output = serde_json::to_writer(&mut self.writer, &record)
                .map_err(std::io::Error::from)
                .and_then(|()| self.writer.write_all(b"\n"));
            record.fields.is_some()
        });
        output.map_err(|error| io_error(&self.path, error))
    }

    pub fn finish(&mut self) -> Result<(), RunnerError> {
        self.writer
            .flush()
            .map_err(|error| io_error(&self.path, error))
    }
}
