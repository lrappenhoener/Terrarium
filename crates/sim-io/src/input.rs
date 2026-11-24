use std::{ffi::OsStr, fs, io::Write, path::Path};

use serde::{Deserialize, de::DeserializeOwned};

use crate::Error;

#[derive(Deserialize)]
pub struct ScheduleEntry<C> {
    pub tick: u64,
    pub commands: Vec<C>,
}

#[derive(Deserialize)]
struct ScheduleFile<C> {
    schedule: Vec<ScheduleEntry<C>>,
}

enum InputFormat {
    Toml,
    Json,
}

impl InputFormat {
    fn from_path(path: &Path) -> Result<Self, Error> {
        match path.extension().and_then(OsStr::to_str) {
            Some("toml") => Ok(Self::Toml),
            Some("json") => Ok(Self::Json),
            _ => Err(Error::UnsupportedFormat(path.into())),
        }
    }

    fn parse<T: DeserializeOwned>(&self, path: &Path, text: &str) -> Result<T, Error> {
        let result = match self {
            Self::Toml => toml::from_str(text).map_err(Into::into),
            Self::Json => serde_json::from_str(text).map_err(Into::into),
        };
        result.map_err(|source| Error::Parse {
            path: path.into(),
            source,
        })
    }
}

pub fn read_input<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let format = InputFormat::from_path(path)?;
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        context: path.display().to_string(),
        source,
    })?;
    format.parse(path, &text)
}

pub fn load_schedule<C: DeserializeOwned>(
    path: &Path,
    ticks: u64,
    diagnostics: &mut dyn Write,
) -> Result<Vec<ScheduleEntry<C>>, Error> {
    let file = read_input::<ScheduleFile<C>>(path)?;
    prepare_schedule(file, ticks, diagnostics)
}

fn prepare_schedule<C>(
    mut file: ScheduleFile<C>,
    ticks: u64,
    diagnostics: &mut dyn Write,
) -> Result<Vec<ScheduleEntry<C>>, Error> {
    for entry in &file.schedule {
        if entry.tick >= ticks {
            writeln!(
                diagnostics,
                "Warnung: Commands bei Tick {} liegen außerhalb des Laufs (--ticks {ticks})",
                entry.tick
            )
            .map_err(|source| Error::Io {
                context: "stderr".into(),
                source,
            })?;
        }
    }

    file.schedule.sort_by_key(|entry| entry.tick);
    let mut prepared: Vec<ScheduleEntry<C>> = Vec::new();
    for entry in file
        .schedule
        .into_iter()
        .take_while(|entry| entry.tick < ticks)
    {
        if let Some(last) = prepared.last_mut().filter(|last| last.tick == entry.tick) {
            last.commands.extend(entry.commands);
        } else {
            prepared.push(entry);
        }
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    enum Command {
        Add { value: i32 },
    }

    #[test]
    fn schedule_sorts_and_merges_in_file_order_in_both_formats() {
        let toml_text = r#"
            [[schedule]]
            tick = 5
            commands = [{ Add = { value = 11 } }]

            [[schedule]]
            tick = 0
            commands = [{ Add = { value = 9 } }]

            [[schedule]]
            tick = 5
            commands = [{ Add = { value = 12 } }, { Add = { value = 13 } }]

            [[schedule]]
            tick = 10
            commands = [{ Add = { value = 99 } }]
        "#;
        let json_text = r#"{"schedule":[
            {"tick":5,"commands":[{"Add":{"value":11}}]},
            {"tick":0,"commands":[{"Add":{"value":9}}]},
            {"tick":5,"commands":[{"Add":{"value":12}},{"Add":{"value":13}}]},
            {"tick":10,"commands":[{"Add":{"value":99}}]}
        ]}"#;
        for (path, text) in [("commands.toml", toml_text), ("commands.json", json_text)] {
            let path = Path::new(path);
            let file = InputFormat::from_path(path)
                .unwrap()
                .parse::<ScheduleFile<Command>>(path, text)
                .unwrap();
            let mut warnings = Vec::new();
            let prepared = prepare_schedule(file, 10, &mut warnings).unwrap();
            assert_eq!(prepared.len(), 2);
            assert_eq!(prepared[0].tick, 0);
            assert_eq!(prepared[0].commands, [Command::Add { value: 9 }]);
            assert_eq!(prepared[1].tick, 5);
            assert_eq!(
                prepared[1].commands,
                [
                    Command::Add { value: 11 },
                    Command::Add { value: 12 },
                    Command::Add { value: 13 },
                ]
            );
            assert!(String::from_utf8(warnings).unwrap().contains("Tick 10"));
        }
    }

    #[test]
    fn unsupported_extension_is_rejected_before_reading_the_file() {
        assert!(matches!(
            read_input::<ScheduleFile<Command>>(Path::new("absent.txt")),
            Err(Error::UnsupportedFormat(_))
        ));
    }
}
