//! Data-driven content from TOML files.
//!
//! Content comes in ordered layers: the base content first, then mods. Within a
//! layer, files are read in ascending path order, never in file system order. Each
//! file holds arrays of tables per kind, for example `[[material]]`, and every
//! entry has a string `id`. A later entry with an existing ID is an error unless it
//! sets `override = true`, which replaces the whole entry; an override without an
//! existing entry is an error as well.
//!
//! String IDs exist only while loading. Each kind maps its IDs in ascending order to
//! dense indices, so the indices do not depend on files, layers or their order.
//! Floating-point values are rejected because simulation state is integral.

// Errors carry file, kind, ID and key path for the author; they occur only while
// loading, so their size does not matter.
#![allow(clippy::result_large_err)]

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;
use sim_host::StateHasher;
use thiserror::Error;

/// One content file of a layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFile {
    pub layer: String,
    /// Relative path with `/` separators; it orders files within the layer.
    pub path: String,
    pub text: String,
}

/// Where an entry was defined.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Origin {
    pub layer: String,
    pub path: String,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.layer, self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContentError {
    #[error("Inhalt {path}: {message}")]
    Io { path: String, message: String },
    #[error("{origin}: TOML-Fehler: {message}")]
    Parse { origin: Origin, message: String },
    #[error("{origin}: {key}: {message}")]
    Format {
        origin: Origin,
        key: String,
        message: String,
    },
    #[error("{origin}: unbekannte Inhaltsart „{kind}“")]
    UnknownKind { origin: Origin, kind: String },
    #[error(
        "{second}: {kind} „{id}“ ist bereits in {first} definiert; zum Ersetzen `override = true` setzen"
    )]
    Duplicate {
        kind: String,
        id: String,
        first: Origin,
        second: Origin,
    },
    #[error(
        "{origin}: {kind} „{id}“ hat `override = true`, ersetzt aber keinen bestehenden Eintrag"
    )]
    OverrideWithoutTarget {
        kind: String,
        id: String,
        origin: Origin,
    },
    #[error("{origin}: {kind} „{id}“: {key}: {message}")]
    Entry {
        origin: Origin,
        kind: String,
        id: String,
        key: String,
        message: String,
    },
    #[error("{origin}: {kind} „{id}“: {key}: unbekannte Referenz auf {target_kind} „{target}“")]
    UnknownReference {
        origin: Origin,
        kind: String,
        id: String,
        key: String,
        target_kind: String,
        target: String,
    },
    #[error("{kind}: unzulässiger Zyklus über {}", path.join(" → "))]
    Cycle { kind: String, path: Vec<String> },
    #[error("{kind}: mehr als {max} Einträge")]
    TooMany { kind: String, max: usize },
}

/// Reads all `.toml` files below `dir` as one layer, ordered by relative path.
pub fn read_layer(layer: &str, dir: &Path) -> Result<Vec<ContentFile>, ContentError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ContentError> {
        let io = |error: std::io::Error| ContentError::Io {
            path: dir.display().to_string(),
            message: error.to_string(),
        };
        for entry in std::fs::read_dir(dir).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.extension().is_some_and(|e| e == "toml") {
                out.push(path);
            }
        }
        let _ = root;
        Ok(())
    }
    let mut paths = Vec::new();
    walk(dir, dir, &mut paths)?;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let text = std::fs::read_to_string(&path).map_err(|error| ContentError::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        files.push(ContentFile {
            layer: layer.into(),
            path: relative,
            text,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[derive(Debug, Clone)]
struct RawEntry {
    origin: Origin,
    table: toml::Table,
}

/// Merged content: per kind, entries in ascending ID order.
#[derive(Debug, Clone, Default)]
pub struct Content {
    kinds: BTreeMap<String, BTreeMap<String, RawEntry>>,
}

fn reject_floats(value: &toml::Value, key: &str, origin: &Origin) -> Result<(), ContentError> {
    match value {
        toml::Value::Float(_) => Err(ContentError::Format {
            origin: origin.clone(),
            key: key.into(),
            message: "Gleitkommazahlen sind nicht erlaubt; ganze Zahlen verwenden".into(),
        }),
        toml::Value::Datetime(_) => Err(ContentError::Format {
            origin: origin.clone(),
            key: key.into(),
            message: "Datumswerte sind nicht erlaubt".into(),
        }),
        toml::Value::Array(values) => values
            .iter()
            .enumerate()
            .try_for_each(|(i, v)| reject_floats(v, &format!("{key}[{i}]"), origin)),
        toml::Value::Table(table) => table
            .iter()
            .try_for_each(|(k, v)| reject_floats(v, &format!("{key}.{k}"), origin)),
        _ => Ok(()),
    }
}

impl Content {
    /// Merges layers in the given order. Files of each layer are merged in
    /// ascending path order, so the order of `layers[i]` does not matter.
    pub fn merge(layers: &[Vec<ContentFile>]) -> Result<Self, ContentError> {
        let mut content = Self::default();
        for layer in layers {
            let mut files: Vec<&ContentFile> = layer.iter().collect();
            files.sort_by(|a, b| a.path.cmp(&b.path));
            for file in files {
                content.add_file(file)?;
            }
        }
        Ok(content)
    }

    fn add_file(&mut self, file: &ContentFile) -> Result<(), ContentError> {
        let origin = Origin {
            layer: file.layer.clone(),
            path: file.path.clone(),
        };
        let table: toml::Table =
            toml::from_str(&file.text).map_err(|error| ContentError::Parse {
                origin: origin.clone(),
                message: error.to_string(),
            })?;
        for (kind, value) in table {
            let format = |key: String, message: &str| ContentError::Format {
                origin: origin.clone(),
                key,
                message: message.into(),
            };
            let toml::Value::Array(entries) = value else {
                return Err(format(kind, "erwartet eine Liste von Tabellen ([[art]])"));
            };
            for (i, entry) in entries.into_iter().enumerate() {
                let key = format!("{kind}[{i}]");
                let toml::Value::Table(mut table) = entry else {
                    return Err(format(key, "erwartet eine Tabelle"));
                };
                let id = match table.remove("id") {
                    Some(toml::Value::String(id)) if !id.is_empty() => id,
                    Some(_) => {
                        return Err(format(
                            format!("{key}.id"),
                            "erwartet einen nichtleeren Text",
                        ));
                    }
                    None => return Err(format(format!("{key}.id"), "Pflichtfeld fehlt")),
                };
                let replace = match table.remove("override") {
                    None => false,
                    Some(toml::Value::Boolean(flag)) => flag,
                    Some(_) => {
                        return Err(format(
                            format!("{key}.override"),
                            "erwartet true oder false",
                        ));
                    }
                };
                for (field, value) in &table {
                    reject_floats(value, &format!("{kind}.{id}.{field}"), &origin)?;
                }
                let entries = self.kinds.entry(kind.clone()).or_default();
                let raw = RawEntry {
                    origin: origin.clone(),
                    table,
                };
                match (entries.get(&id), replace) {
                    (Some(first), false) => {
                        return Err(ContentError::Duplicate {
                            kind,
                            id,
                            first: first.origin.clone(),
                            second: origin,
                        });
                    }
                    (None, true) => {
                        return Err(ContentError::OverrideWithoutTarget { kind, id, origin });
                    }
                    _ => {
                        entries.insert(id, raw);
                    }
                }
            }
        }
        Ok(())
    }

    /// Fails for kinds outside `known`, which catches misspelled table names.
    pub fn expect_kinds(&self, known: &[&str]) -> Result<(), ContentError> {
        for (kind, entries) in &self.kinds {
            if !known.contains(&kind.as_str()) {
                let origin = entries
                    .values()
                    .map(|e| e.origin.clone())
                    .min()
                    .unwrap_or(Origin {
                        layer: String::new(),
                        path: String::new(),
                    });
                return Err(ContentError::UnknownKind {
                    origin,
                    kind: kind.clone(),
                });
            }
        }
        Ok(())
    }

    /// IDs of a kind in index order.
    pub fn ids(&self, kind: &str) -> Vec<String> {
        self.kinds
            .get(kind)
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Deserializes all entries of a kind into a registry in index order.
    pub fn registry<T: DeserializeOwned>(&self, kind: &str) -> Result<Registry<T>, ContentError> {
        let empty = BTreeMap::new();
        let entries = self.kinds.get(kind).unwrap_or(&empty);
        if entries.len() > usize::from(u16::MAX) {
            return Err(ContentError::TooMany {
                kind: kind.into(),
                max: usize::from(u16::MAX),
            });
        }
        let mut registry = Registry {
            kind: kind.into(),
            ids: Vec::with_capacity(entries.len()),
            origins: Vec::with_capacity(entries.len()),
            items: Vec::with_capacity(entries.len()),
        };
        for (id, entry) in entries {
            let item = toml::Value::Table(entry.table.clone())
                .try_into::<T>()
                .map_err(|error| ContentError::Entry {
                    origin: entry.origin.clone(),
                    kind: kind.into(),
                    id: id.clone(),
                    key: "Felder".into(),
                    message: error.to_string().trim().replace('\n', " "),
                })?;
            registry.ids.push(id.clone());
            registry.origins.push(entry.origin.clone());
            registry.items.push(item);
        }
        Ok(registry)
    }

    /// Stable hash over the merged content: kinds and IDs in ascending order, keys
    /// sorted, origins and override markers excluded.
    pub fn fingerprint(&self) -> u64 {
        fn value(hash: &mut StateHasher, item: &toml::Value) {
            match item {
                toml::Value::String(text) => {
                    hash.write_u8(1);
                    hash.write_usize(text.len());
                    hash.write_bytes(text.as_bytes());
                }
                toml::Value::Integer(number) => {
                    hash.write_u8(2);
                    hash.write_u64(*number as u64);
                }
                toml::Value::Boolean(flag) => {
                    hash.write_u8(3);
                    hash.write_bool(*flag);
                }
                toml::Value::Array(values) => {
                    hash.write_u8(4);
                    hash.write_usize(values.len());
                    values.iter().for_each(|v| value(hash, v));
                }
                toml::Value::Table(table) => {
                    hash.write_u8(5);
                    table_hash(hash, table);
                }
                // Rejected while merging.
                toml::Value::Float(_) | toml::Value::Datetime(_) => hash.write_u8(0),
            }
        }
        fn table_hash(hash: &mut StateHasher, table: &toml::Table) {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            hash.write_usize(keys.len());
            for key in keys {
                hash.write_usize(key.len());
                hash.write_bytes(key.as_bytes());
                if let Some(v) = table.get(key) {
                    value(hash, v);
                }
            }
        }
        let mut hash = StateHasher::new();
        hash.write_usize(self.kinds.len());
        for (kind, entries) in &self.kinds {
            hash.write_usize(kind.len());
            hash.write_bytes(kind.as_bytes());
            hash.write_usize(entries.len());
            for (id, entry) in entries {
                hash.write_usize(id.len());
                hash.write_bytes(id.as_bytes());
                table_hash(&mut hash, &entry.table);
            }
        }
        hash.finish()
    }
}

/// Entries of one kind with dense indices in ascending ID order.
#[derive(Debug, Clone)]
pub struct Registry<T> {
    kind: String,
    ids: Vec<String>,
    origins: Vec<Origin>,
    items: Vec<T>,
}

impl<T> Registry<T> {
    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, index: u16) -> Option<&T> {
        self.items.get(usize::from(index))
    }

    pub fn id(&self, index: u16) -> &str {
        self.ids.get(usize::from(index)).map_or("", String::as_str)
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &T)> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| (i as u16, item))
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Index of an ID; for loading only.
    pub fn index_of(&self, id: &str) -> Option<u16> {
        self.ids
            .binary_search_by(|probe| probe.as_str().cmp(id))
            .ok()
            .map(|i| i as u16)
    }

    /// An error located at entry `index` and key path `key`.
    pub fn error(&self, index: u16, key: &str, message: impl Into<String>) -> ContentError {
        ContentError::Entry {
            origin: self.origin(index),
            kind: self.kind.clone(),
            id: self.id(index).into(),
            key: key.into(),
            message: message.into(),
        }
    }

    /// Fails with a located message unless `ok`.
    pub fn check(
        &self,
        index: u16,
        key: &str,
        ok: bool,
        message: &str,
    ) -> Result<(), ContentError> {
        if ok {
            Ok(())
        } else {
            Err(self.error(index, key, message))
        }
    }

    fn origin(&self, index: u16) -> Origin {
        self.origins
            .get(usize::from(index))
            .cloned()
            .unwrap_or(Origin {
                layer: String::new(),
                path: String::new(),
            })
    }

    /// Resolves a reference from entry `index` at key path `key` into `target`.
    pub fn resolve<U>(
        &self,
        index: u16,
        key: &str,
        target: &Registry<U>,
        id: &str,
    ) -> Result<u16, ContentError> {
        target
            .index_of(id)
            .ok_or_else(|| ContentError::UnknownReference {
                origin: self.origin(index),
                kind: self.kind.clone(),
                id: self.id(index).into(),
                key: key.into(),
                target_kind: target.kind.clone(),
                target: id.into(),
            })
    }

    /// Fails if the edges from `edges(index)` form a cycle among this kind's entries.
    /// The search visits entries in index order, so the reported cycle is stable.
    pub fn reject_cycles(&self, edges: impl Fn(u16) -> Vec<u16>) -> Result<(), ContentError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Open,
            Done,
        }
        let mut marks = vec![Mark::New; self.len()];
        let mut stack: Vec<(u16, Vec<u16>, usize)> = Vec::new();
        for start in 0..self.len() as u16 {
            if marks[usize::from(start)] != Mark::New {
                continue;
            }
            marks[usize::from(start)] = Mark::Open;
            stack.push((start, edges(start), 0));
            while let Some((node, next, position)) = stack.last_mut() {
                let Some(&target) = next.get(*position) else {
                    marks[usize::from(*node)] = Mark::Done;
                    stack.pop();
                    continue;
                };
                *position += 1;
                match marks.get(usize::from(target)) {
                    Some(Mark::New) => {
                        marks[usize::from(target)] = Mark::Open;
                        let following = edges(target);
                        stack.push((target, following, 0));
                    }
                    Some(Mark::Open) => {
                        let from = stack.iter().position(|(n, _, _)| *n == target).unwrap_or(0);
                        let mut path: Vec<String> = stack[from..]
                            .iter()
                            .map(|(n, _, _)| self.id(*n).to_string())
                            .collect();
                        path.push(self.id(target).into());
                        return Err(ContentError::Cycle {
                            kind: self.kind.clone(),
                            path,
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Material {
        hardness: u32,
        #[serde(default)]
        melts_into: Option<String>,
    }

    fn file(layer: &str, path: &str, text: &str) -> ContentFile {
        ContentFile {
            layer: layer.into(),
            path: path.into(),
            text: text.into(),
        }
    }

    fn base() -> Vec<ContentFile> {
        vec![
            file(
                "base",
                "rock/granite.toml",
                "[[material]]\nid = \"granite\"\nhardness = 26\n",
            ),
            file(
                "base",
                "metal.toml",
                "[[material]]\nid = \"iron\"\nhardness = 40\n\n[[material]]\nid = \"iron_ore\"\nhardness = 30\nmelts_into = \"iron\"\n",
            ),
            file(
                "base",
                "rock/basalt.toml",
                "[[material]]\nid = \"basalt\"\nhardness = 32\n",
            ),
        ]
    }

    #[test]
    fn file_order_does_not_change_content_indices_or_fingerprint() {
        let reference = Content::merge(&[base()]).unwrap();
        let ids = reference.ids("material");
        assert_eq!(ids, ["basalt", "granite", "iron", "iron_ore"]);
        let mut files = base();
        for rotation in 0..files.len() {
            files.rotate_left(1);
            let len = files.len();
            files.swap(0, rotation % len);
            let content = Content::merge(&[files.clone()]).unwrap();
            assert_eq!(content.ids("material"), ids);
            assert_eq!(content.fingerprint(), reference.fingerprint());
            let registry = content.registry::<Material>("material").unwrap();
            assert_eq!(registry.index_of("iron_ore"), Some(3));
            assert_eq!(registry.get(1).unwrap().hardness, 26);
        }
    }

    #[test]
    fn mods_add_entries_and_replace_only_with_override() {
        let added = file(
            "mod",
            "a.toml",
            "[[material]]\nid = \"marble\"\nhardness = 12\n",
        );
        let content = Content::merge(&[base(), vec![added]]).unwrap();
        assert_eq!(content.ids("material").len(), 5);

        let plain = file(
            "mod",
            "a.toml",
            "[[material]]\nid = \"granite\"\nhardness = 1\n",
        );
        let error = Content::merge(&[base(), vec![plain]]).unwrap_err();
        assert!(matches!(error, ContentError::Duplicate { .. }), "{error}");
        assert_eq!(
            error.to_string(),
            "mod/a.toml: material „granite“ ist bereits in base/rock/granite.toml definiert; zum Ersetzen `override = true` setzen"
        );

        let marked = file(
            "mod",
            "a.toml",
            "[[material]]\nid = \"granite\"\noverride = true\nhardness = 1\n",
        );
        let content = Content::merge(&[base(), vec![marked]]).unwrap();
        let registry = content.registry::<Material>("material").unwrap();
        assert_eq!(
            registry
                .get(registry.index_of("granite").unwrap())
                .unwrap()
                .hardness,
            1
        );

        let dangling = file(
            "mod",
            "a.toml",
            "[[material]]\nid = \"jade\"\noverride = true\nhardness = 1\n",
        );
        let error = Content::merge(&[base(), vec![dangling]]).unwrap_err();
        assert!(
            matches!(error, ContentError::OverrideWithoutTarget { .. }),
            "{error}"
        );
    }

    #[test]
    fn every_error_kind_names_file_and_key() {
        let merge = |text: &str| Content::merge(&[vec![file("base", "x.toml", text)]]);
        let parse = merge("[[material]\n").unwrap_err();
        assert!(
            parse.to_string().starts_with("base/x.toml: TOML-Fehler"),
            "{parse}"
        );
        let missing_id = merge("[[material]]\nhardness = 1\n").unwrap_err();
        assert_eq!(
            missing_id.to_string(),
            "base/x.toml: material[0].id: Pflichtfeld fehlt"
        );
        let float = merge("[[material]]\nid = \"a\"\nhardness = 1.5\n").unwrap_err();
        assert!(
            float
                .to_string()
                .contains("material.a.hardness: Gleitkommazahlen"),
            "{float}"
        );
        let not_list = merge("material = 3\n").unwrap_err();
        assert!(
            matches!(not_list, ContentError::Format { .. }),
            "{not_list}"
        );

        let content = merge("[[material]]\nid = \"a\"\n\n[[metal]]\nid = \"b\"\n").unwrap();
        let unknown_kind = content.expect_kinds(&["material"]).unwrap_err();
        assert_eq!(
            unknown_kind.to_string(),
            "base/x.toml: unbekannte Inhaltsart „metal“"
        );
        let missing_field = content.registry::<Material>("material").unwrap_err();
        assert!(
            missing_field.to_string().contains("material „a“"),
            "{missing_field}"
        );
        assert!(
            missing_field.to_string().contains("hardness"),
            "{missing_field}"
        );

        let content = merge("[[material]]\nid = \"a\"\nhardness = 1\ncolour = 2\n").unwrap();
        let unknown_field = content.registry::<Material>("material").unwrap_err();
        assert!(
            unknown_field.to_string().contains("colour"),
            "{unknown_field}"
        );

        let content =
            merge("[[material]]\nid = \"a\"\nhardness = 0\nmelts_into = \"zinc\"\n").unwrap();
        let registry = content.registry::<Material>("material").unwrap();
        let range = registry.check(
            0,
            "hardness",
            registry.get(0).unwrap().hardness > 0,
            "muss positiv sein",
        );
        assert_eq!(
            range.unwrap_err().to_string(),
            "base/x.toml: material „a“: hardness: muss positiv sein"
        );
        let reference = registry
            .resolve(0, "melts_into", &registry, "zinc")
            .unwrap_err();
        assert_eq!(
            reference.to_string(),
            "base/x.toml: material „a“: melts_into: unbekannte Referenz auf material „zinc“"
        );
    }

    #[test]
    fn cycles_are_reported_with_their_path() {
        let text = "[[material]]\nid = \"a\"\nhardness = 1\nmelts_into = \"b\"\n\n[[material]]\nid = \"b\"\nhardness = 1\nmelts_into = \"c\"\n\n[[material]]\nid = \"c\"\nhardness = 1\nmelts_into = \"a\"\n\n[[material]]\nid = \"d\"\nhardness = 1\n";
        let content = Content::merge(&[vec![file("base", "x.toml", text)]]).unwrap();
        let registry = content.registry::<Material>("material").unwrap();
        let edges = |i: u16| {
            registry
                .get(i)
                .and_then(|m| m.melts_into.as_deref())
                .and_then(|id| registry.index_of(id))
                .into_iter()
                .collect()
        };
        let error = registry.reject_cycles(edges).unwrap_err();
        assert_eq!(
            error.to_string(),
            "material: unzulässiger Zyklus über a → b → c → a"
        );
        let acyclic = |i: u16| if i == 0 { vec![1] } else { vec![] };
        registry.reject_cycles(acyclic).unwrap();
    }

    #[test]
    fn fingerprint_changes_with_every_content_change() {
        let reference = Content::merge(&[base()]).unwrap().fingerprint();
        let variants = [
            "[[material]]\nid = \"granite\"\nhardness = 27\n",
            "[[material]]\nid = \"granitee\"\nhardness = 26\n",
            "[[material]]\nid = \"granite\"\nhardness = 26\nmelts_into = \"iron\"\n",
            "[[rock]]\nid = \"granite\"\nhardness = 26\n",
        ];
        let mut seen = vec![reference];
        for text in variants {
            let mut files = base();
            files[0].text = text.into();
            let fingerprint = Content::merge(&[files]).unwrap().fingerprint();
            assert!(!seen.contains(&fingerprint), "{text}");
            seen.push(fingerprint);
        }
        // Moving an entry to another file or layer does not change the content.
        let mut files = base();
        let granite = files.remove(0);
        let moved = file("mod", "z.toml", &granite.text);
        assert_eq!(
            Content::merge(&[files, vec![moved]]).unwrap().fingerprint(),
            reference
        );
    }

    #[test]
    fn layers_are_read_from_directories_in_path_order() {
        let dir = std::env::temp_dir().join(format!("sim-core-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rock")).unwrap();
        for f in base() {
            std::fs::write(dir.join(&f.path), &f.text).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let files = read_layer("base", &dir).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["metal.toml", "rock/basalt.toml", "rock/granite.toml"]
        );
        assert_eq!(
            Content::merge(&[files]).unwrap().fingerprint(),
            Content::merge(&[base()]).unwrap().fingerprint()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
