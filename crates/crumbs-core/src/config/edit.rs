//! Changing `config.toml` one key at a time (`crumbs config set|add|remove|reset`).
//!
//! Every value is checked before anything is written, more strictly than [`load_config`]
//! reads it: a value load would drop or replace with a warning is refused here. The file is
//! rewritten whole through a temporary file and a rename, so a failed write leaves the old
//! file. Keys this crumbs does not know are kept; comments are not.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{journal_size_bytes, load_config};
use crate::bloat::Category;
use crate::env::Env;
use crate::error::CoreError;

/// The keys `config` can change, and what kind of value each takes.
pub const KEYS: [(&str, KeyKind); 12] = [
    ("protect", KeyKind::Paths),
    ("exclude", KeyKind::Paths),
    ("appimage_dirs", KeyKind::Paths),
    ("project_dirs", KeyKind::Paths),
    ("categories_off", KeyKind::Categories),
    ("categories_on", KeyKind::Categories),
    ("temp_age_days", KeyKind::Days),
    ("log_age_days", KeyKind::Days),
    ("build_age_days", KeyKind::Days),
    ("snapshots_keep", KeyKind::Days),
    ("pkg_cache_keep", KeyKind::Count),
    ("journal_max", KeyKind::Size),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyKind {
    /// A list of paths or patterns, each starting with `/` or `~/`.
    Paths,
    /// A list of category names.
    Categories,
    /// A whole number, at least 1.
    Days,
    /// A whole number, at least 0.
    Count,
    /// A journal size: digits and an optional K, M, G or T, above 0.
    Size,
}

/// One change to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Set a number or size key.
    Set { key: String, value: String },
    /// Add an entry to a list key (no change when it is already there).
    Add { key: String, value: String },
    /// Take an entry out of a list key (no change when it is not there).
    Remove { key: String, value: String },
    /// Take the key out of the file, so its default applies; `all` empties the file.
    Reset { key: String },
}

/// Where the config file lives.
pub fn config_path(env: &Env) -> PathBuf {
    env.config_home.join("crumbs/config.toml")
}

fn refuse(message: String) -> CoreError {
    CoreError::Parse {
        what: "config change".into(),
        message,
    }
}

fn kind_of(key: &str) -> Result<KeyKind, CoreError> {
    KEYS.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, kind)| *kind)
        .ok_or_else(|| {
            let names: Vec<&str> = KEYS.iter().map(|(k, _)| *k).collect();
            refuse(format!(
                "unknown key '{key}' (one of: {})",
                names.join(", ")
            ))
        })
}

/// Checks one list entry; returns it as it will be written.
fn list_entry(kind: KeyKind, key: &str, value: &str) -> Result<String, CoreError> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(refuse(format!("{key}: '{value}' is not a usable entry")));
    }
    match kind {
        KeyKind::Paths if value.starts_with('/') || value.starts_with("~/") || value == "~" => {
            Ok(value.to_string())
        }
        KeyKind::Paths => Err(refuse(format!("{key}: '{value}' must start with / or ~/"))),
        KeyKind::Categories => value
            .parse::<Category>()
            .map(|c| c.as_str().to_string())
            .map_err(|_| refuse(format!("{key}: unknown category '{value}'"))),
        _ => Err(refuse(format!("{key} is not a list; use set"))),
    }
}

/// Checks a number or size value; returns it as TOML.
fn scalar(kind: KeyKind, key: &str, value: &str) -> Result<toml::Value, CoreError> {
    let number = || {
        value
            .parse::<i64>()
            .map_err(|_| refuse(format!("{key}: '{value}' is not a whole number")))
    };
    match kind {
        KeyKind::Days => match number()? {
            n if n >= 1 => Ok(toml::Value::Integer(n)),
            _ => Err(refuse(format!("{key} must be at least 1"))),
        },
        KeyKind::Count => match number()? {
            n if (0..=i64::from(u32::MAX)).contains(&n) => Ok(toml::Value::Integer(n)),
            _ => Err(refuse(format!("{key} must be at least 0"))),
        },
        KeyKind::Size if journal_size_bytes(value).is_some_and(|b| b > 0) => {
            Ok(toml::Value::String(value.to_string()))
        }
        KeyKind::Size => Err(refuse(format!(
            "{key}: '{value}' is not a size (digits and an optional K, M, G or T)"
        ))),
        _ => Err(refuse(format!("{key} is a list; use add or remove"))),
    }
}

fn read_table(path: &Path) -> Result<toml::Table, CoreError> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| CoreError::Parse {
            what: "config.toml".into(),
            message: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(source) => Err(CoreError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The list at `key`, as strings. `categories_off` left out of the file means
/// its default, `["dev-cache"]`, so adding to it keeps that.
fn list_of(table: &toml::Table, key: &str) -> Result<Vec<String>, CoreError> {
    match table.get(key) {
        None if key == "categories_off" => Ok(vec![Category::DevCache.as_str().to_string()]),
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| refuse(format!("{key} in config.toml holds a non-string")))
            })
            .collect(),
        Some(_) => Err(refuse(format!("{key} in config.toml is not a list"))),
    }
}

/// Writes `text` to `path` through a temporary file beside it.
fn write_atomic(path: &Path, text: &str) -> Result<(), CoreError> {
    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |source| CoreError::Io { path: p, source }
    };
    let dir = path.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let tmp = dir.join(format!(".config.toml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(path)(e)
    })
}

/// Applies `edit` to the config file and returns whether the file changed. Nothing is
/// written when the value is refused, or when the change would leave the file as it is.
pub fn apply_edit(env: &Env, edit: &Edit) -> Result<bool, CoreError> {
    let path = config_path(env);
    let mut table = read_table(&path)?;
    let before = table.clone();
    match edit {
        Edit::Set { key, value } => {
            let kind = kind_of(key)?;
            table.insert(key.clone(), scalar(kind, key, value)?);
        }
        Edit::Add { key, value } | Edit::Remove { key, value } => {
            let kind = kind_of(key)?;
            let entry = list_entry(kind, key, value)?;
            let mut list = list_of(&table, key)?;
            if matches!(edit, Edit::Add { .. }) {
                if !list.contains(&entry) {
                    list.push(entry);
                }
            } else {
                list.retain(|e| *e != entry);
            }
            let changed = list_of(&before, key)? != list;
            if changed || table.contains_key(key) {
                table.insert(
                    key.clone(),
                    toml::Value::Array(list.into_iter().map(toml::Value::String).collect()),
                );
            }
        }
        Edit::Reset { key } if key == "all" => table.clear(),
        Edit::Reset { key } => {
            kind_of(key)?;
            table.remove(key);
        }
    }
    if table == before {
        return Ok(false);
    }
    let text = toml::to_string(&table).map_err(|e| refuse(e.to_string()))?;
    write_atomic(&path, &text)?;
    // What was written must load as it did before, with no new warnings.
    load_config(env)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    fn edit(env: &Env, e: Edit) -> Result<bool, CoreError> {
        apply_edit(env, &e)
    }
    fn set(key: &str, value: &str) -> Edit {
        Edit::Set {
            key: key.into(),
            value: value.into(),
        }
    }
    fn add(key: &str, value: &str) -> Edit {
        Edit::Add {
            key: key.into(),
            value: value.into(),
        }
    }
    fn remove(key: &str, value: &str) -> Edit {
        Edit::Remove {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn set_add_remove_reset_round_trip() {
        let t = FixtureTree::new();
        let env = t.env();
        assert!(edit(&env, set("temp_age_days", "3")).unwrap());
        assert!(edit(&env, add("protect", "~/Games")).unwrap());
        assert!(
            !edit(&env, add("protect", "~/Games")).unwrap(),
            "already there"
        );
        assert!(edit(&env, add("categories_off", "thumbnails")).unwrap());
        assert!(edit(&env, set("journal_max", "1G")).unwrap());
        let cfg = load_config(&env).unwrap();
        assert_eq!(cfg.temp_age_days, 3);
        assert_eq!(cfg.journal_max, "1G");
        assert_eq!(cfg.protect.len(), 1);
        assert!(cfg.protect[0].ends_with("Games"));
        assert_eq!(
            cfg.categories_off,
            [Category::DevCache, Category::Thumbnails],
            "the default dev-cache is kept when adding"
        );
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);

        assert!(edit(&env, remove("protect", "~/Games")).unwrap());
        assert!(load_config(&env).unwrap().protect.is_empty());
        assert!(
            edit(
                &env,
                Edit::Reset {
                    key: "temp_age_days".into()
                }
            )
            .unwrap()
        );
        assert_eq!(load_config(&env).unwrap().temp_age_days, 7);
        assert!(edit(&env, Edit::Reset { key: "all".into() }).unwrap());
        assert_eq!(load_config(&env).unwrap().journal_max, "500M");
    }

    #[test]
    fn bad_values_are_refused_and_nothing_is_written() {
        let t = FixtureTree::new();
        let env = t.env();
        for bad in [
            set("temp_age_days", "0"),
            set("temp_age_days", "soon"),
            set("pkg_cache_keep", "-1"),
            set("journal_max", "lots"),
            set("protect", "/x"),
            add("protect", "relative/path"),
            add("exclude", "/tmp/a\nb"),
            add("categories_on", "no-such-category"),
            add("temp_age_days", "3"),
            set("no_such_key", "1"),
            Edit::Reset {
                key: "no_such_key".into(),
            },
        ] {
            assert!(edit(&env, bad.clone()).is_err(), "{bad:?} was accepted");
        }
        assert!(
            !config_path(&env).exists(),
            "a refused change wrote the file"
        );
    }

    #[test]
    fn unknown_keys_in_the_file_are_kept() {
        let t = FixtureTree::new();
        let env = t.env();
        let path = config_path(&env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "future_key = \"keep me\"\nlog_age_days = 20\n").unwrap();
        assert!(edit(&env, set("log_age_days", "30")).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("future_key = \"keep me\""), "{text}");
        assert_eq!(load_config(&env).unwrap().log_age_days, 30);
    }
}
