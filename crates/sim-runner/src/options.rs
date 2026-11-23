use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{RunnerError, input::read_input};

fn assignment(text: &str) -> Result<(&str, &str), RunnerError> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| invalid(format!("expected key=value, got {text:?}")))?;
    if key.split('.').any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    }) {
        return Err(invalid(format!("invalid dotted key {key:?}")));
    }
    Ok((key, value))
}

fn invalid(message: String) -> RunnerError {
    RunnerError::InvalidOptions(message)
}

fn parse_toml(text: &str) -> Result<toml::Value, RunnerError> {
    let mut table: toml::Table =
        toml::from_str(&format!("value = {text}")).map_err(|error| invalid(error.to_string()))?;
    if table.len() != 1 {
        return Err(invalid("expected exactly one TOML literal".into()));
    }
    table
        .remove("value")
        .ok_or_else(|| invalid("expected TOML value".into()))
}

pub(crate) fn load_config<T: DeserializeOwned + Default>(
    path: Option<&Path>,
    overrides: &[String],
) -> Result<T, RunnerError> {
    if overrides.is_empty() {
        return path.map(read_input).unwrap_or_else(|| Ok(T::default()));
    }
    if path
        .and_then(Path::extension)
        .is_some_and(|ext| ext == "json")
    {
        let mut table: Map<String, Value> = path.map(read_input).transpose()?.unwrap_or_default();
        for text in overrides {
            let (key, literal) = assignment(text)?;
            let value = serde_json::to_value(parse_toml(literal)?)
                .map_err(|error| invalid(error.to_string()))?;
            let mut target = &mut table;
            let mut parts = key.split('.').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_some() {
                    target = target
                        .entry(part)
                        .or_insert_with(|| Value::Object(Map::new()))
                        .as_object_mut()
                        .ok_or_else(|| invalid(format!("{part} in {key} is not a table")))?;
                } else {
                    target.insert(part.into(), value);
                    break;
                }
            }
        }
        serde_json::from_value(Value::Object(table))
            .map_err(|error| invalid(format!("configuration: {error}")))
    } else {
        let mut table: toml::Table = path.map(read_input).transpose()?.unwrap_or_default();
        for text in overrides {
            let (key, literal) = assignment(text)?;
            let value = parse_toml(literal)?;
            let mut target = &mut table;
            let mut parts = key.split('.').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_some() {
                    target = target
                        .entry(part)
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                        .as_table_mut()
                        .ok_or_else(|| invalid(format!("{part} in {key} is not a table")))?;
                } else {
                    target.insert(part.into(), value);
                    break;
                }
            }
        }
        toml::Value::Table(table)
            .try_into()
            .map_err(|error| invalid(format!("configuration: {error}")))
    }
}

pub fn parse_sweep(text: &str) -> Result<(&str, Vec<toml::Value>), RunnerError> {
    let (key, values) = assignment(text)?;
    if let toml::Value::Array(values) = parse_toml(&format!("[{values}]"))?
        && !values.is_empty()
    {
        return Ok((key, values));
    }
    Err(invalid("a sweep needs at least one value".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Config {
        group: Group,
        seed: u64,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Group {
        count: u32,
        rate: u32,
    }

    #[test]
    fn nested_overrides_combine_with_configs_and_reject_typos() {
        let temp = std::env::temp_dir();
        for (ext, body) in [
            ("toml", "seed = 44\n[group]\ncount = 3"),
            (
                "json",
                r#"{"seed":18446744073709551615,"group":{"count":3}}"#,
            ),
        ] {
            let path = temp.join(format!("terrarium-overrides-{}.{ext}", std::process::id()));
            std::fs::write(&path, body).unwrap();
            let cfg: Config = load_config(Some(&path), &["group.rate=8".into()]).unwrap();
            assert_eq!((cfg.group.count, cfg.group.rate), (3, 8));
            assert_eq!(cfg.seed, if ext == "toml" { 44 } else { u64::MAX });
            assert!(
                load_config::<Config>(Some(&path), &["group.typo=8".into()])
                    .unwrap_err()
                    .to_string()
                    .contains("typo")
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn sweep_parses_toml_arrays_and_quoted_commas() {
        assert_eq!(parse_sweep("group.count=1, 10, 100").unwrap().1.len(), 3);
        assert_eq!(
            parse_sweep(r#"group.values="x,y","z""#).unwrap().1[0].as_str(),
            Some("x,y")
        );
        assert!(parse_sweep("a=").is_err() && parse_sweep("a..b=1,2").is_err());
    }
}
