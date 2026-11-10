//! Rules: which per-user leftovers belong to which apps.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::env::Env;
use crate::error::CoreError;
use crate::model::{App, SourceKind};
use crate::sources::appimage::normalize_name;

const BUILTIN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/rules/apps.toml"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOrigin {
    Builtin,
    User(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub packages: Vec<String>,
    pub flatpaks: Vec<String>,
    pub appimages: Vec<String>,
    pub desktops: Vec<String>,
    /// Raw path templates, as written in the rules file.
    pub paths: Vec<String>,
    pub origin: RuleOrigin,
}

#[derive(Debug, Default, Deserialize)]
struct RawFile {
    #[serde(default)]
    app: Vec<toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMatch {
    #[serde(default)]
    package: Vec<String>,
    #[serde(default)]
    flatpak: Vec<String>,
    #[serde(default)]
    appimage: Vec<String>,
    #[serde(default)]
    desktop: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawApp {
    #[serde(rename = "match", default)]
    matcher: RawMatch,
    #[serde(default)]
    paths: Vec<String>,
}

fn parse_error(what: &str, message: impl Into<String>) -> CoreError {
    CoreError::Parse {
        what: what.to_string(),
        message: message.into(),
    }
}

/// Parses one rules file. Bad rules are reported and skipped; the file's other
/// rules are kept. Only a file that is not valid TOML loses all its rules.
fn parse_rules(text: &str, what: &str, origin: &RuleOrigin) -> (Vec<Rule>, Vec<CoreError>) {
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    let file: RawFile = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => return (rules, vec![parse_error(what, e.to_string())]),
    };
    for (i, value) in file.app.into_iter().enumerate() {
        let n = i + 1;
        let raw: RawApp = match value.try_into() {
            Ok(r) => r,
            Err(e) => {
                errors.push(parse_error(what, format!("rule {n}: {e}")));
                continue;
            }
        };
        let m = raw.matcher;
        if m.package.is_empty()
            && m.flatpak.is_empty()
            && m.appimage.is_empty()
            && m.desktop.is_empty()
        {
            errors.push(parse_error(
                what,
                format!(
                    "rule {n}: needs at least one match key (package, flatpak, appimage, desktop)"
                ),
            ));
            continue;
        }
        if raw.paths.is_empty() {
            errors.push(parse_error(
                what,
                format!("rule {n}: needs at least one path"),
            ));
            continue;
        }
        for path in raw
            .paths
            .iter()
            .filter(|p| p.starts_with("/etc/") || p.starts_with("/opt/"))
        {
            if expand_system_rule_path(path).is_none() {
                errors.push(parse_error(
                    what,
                    format!(
                        "rule path {path} ignored: only /etc/<name> or /opt/<name> are allowed outside home"
                    ),
                ));
            }
        }
        rules.push(Rule {
            packages: m.package,
            flatpaks: m.flatpak,
            appimages: m.appimage,
            desktops: m.desktop,
            paths: raw.paths,
            origin: origin.clone(),
        });
    }
    (rules, errors)
}

/// Loads the builtin rules, then `{config_home}/crumbs/rules.d/*.toml` sorted
/// by file name. A bad user file or rule is returned as an error and skipped.
pub fn load_rules(env: &Env) -> (Vec<Rule>, Vec<CoreError>) {
    let (mut rules, mut errors) = parse_rules(BUILTIN, "builtin rules", &RuleOrigin::Builtin);

    let dir = env.config_home.join("crumbs/rules.d");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (rules, errors),
        Err(source) => {
            errors.push(CoreError::Io { path: dir, source });
            return (rules, errors);
        }
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "toml") && path.is_file() {
                    files.push(path);
                }
            }
            Err(source) => errors.push(CoreError::Io {
                path: dir.clone(),
                source,
            }),
        }
    }
    files.sort();
    for path in files {
        let shown = env.unsys(&path);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(source) => {
                errors.push(CoreError::Io {
                    path: shown,
                    source,
                });
                continue;
            }
        };
        let (r, e) = parse_rules(
            &text,
            &shown.display().to_string(),
            &RuleOrigin::User(shown.clone()),
        );
        rules.extend(r);
        errors.extend(e);
    }
    (rules, errors)
}

/// Whether the rule's match keys select this app.
pub fn rule_matches(rule: &Rule, app: &App) -> bool {
    match app.source {
        SourceKind::Pacman
        | SourceKind::Dpkg
        | SourceKind::Rpm
        | SourceKind::Snap
        | SourceKind::Unmanaged => {
            if rule.packages.contains(&app.name) {
                return true;
            }
        }
        SourceKind::Flatpak => {
            let id = app.rdns_id.as_deref().unwrap_or(&app.name);
            if rule.flatpaks.iter().any(|f| f == id) {
                return true;
            }
        }
        SourceKind::AppImage => {
            let norm = normalize_name(&app.name);
            if rule.appimages.iter().any(|a| normalize_name(a) == norm) {
                return true;
            }
        }
    }
    app.desktop_ids.iter().any(|d| {
        let id = d.strip_suffix(".desktop").unwrap_or(d);
        rule.desktops.iter().any(|r| r == id)
    })
}

/// Rules that apply to an app. User rules take precedence: if any match, only
/// the matching user rules are returned.
pub fn rules_for<'a>(rules: &'a [Rule], app: &App) -> Vec<&'a Rule> {
    let matching: Vec<&Rule> = rules.iter().filter(|r| rule_matches(r, app)).collect();
    let user: Vec<&Rule> = matching
        .iter()
        .copied()
        .filter(|r| matches!(r.origin, RuleOrigin::User(_)))
        .collect();
    if user.is_empty() { matching } else { user }
}

/// Expands `~`, `$HOME` and `$XDG_*_HOME` (leading token only) to an in-system
/// path. `None` unless the result is strictly under home with no `..`. A
/// trailing `*` is left in place; see [`resolve_rule_paths`].
pub fn expand_rule_path(env: &Env, raw: &str) -> Option<PathBuf> {
    let (base, rest) = if let Some(rest) = raw.strip_prefix('~') {
        (env.unsys(&env.home), rest)
    } else if let Some(rest) = raw.strip_prefix('$') {
        let end = rest.find('/').unwrap_or(rest.len());
        let base = match &rest[..end] {
            "HOME" => &env.home,
            "XDG_CONFIG_HOME" => &env.config_home,
            "XDG_CACHE_HOME" => &env.cache_home,
            "XDG_DATA_HOME" => &env.data_home,
            "XDG_STATE_HOME" => &env.state_home,
            _ => return None,
        };
        (env.unsys(base), &rest[end..])
    } else if raw.starts_with('/') {
        (PathBuf::from("/"), raw)
    } else {
        return None;
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let path = base.join(rest.trim_start_matches('/'));
    let home = env.unsys(&env.home);
    let clean = path
        .components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    (clean && path != home && path.starts_with(&home)).then_some(path)
}

/// A rule path naming a system leftover: exactly `/etc/<name>` or
/// `/opt/<name>` (a trailing `/` is fine) with a name that
/// [`crate::safety::is_system_leftover_name`] accepts. Anything else,
/// including wildcards, `..` and deeper paths, is `None`.
pub fn expand_system_rule_path(raw: &str) -> Option<PathBuf> {
    let rest = raw
        .strip_prefix("/etc/")
        .map(|r| ("/etc", r))
        .or_else(|| raw.strip_prefix("/opt/").map(|r| ("/opt", r)))?;
    let name = rest.1.strip_suffix('/').unwrap_or(rest.1);
    let ok = !name.is_empty()
        && !name.contains(['/', '*'])
        && name != "."
        && name != ".."
        && crate::safety::is_system_leftover_name(std::ffi::OsStr::new(name));
    ok.then(|| Path::new(rest.0).join(name))
}

/// Expands every path of the rule and returns those that exist. A trailing `*`
/// in the final component matches every existing sibling starting with the
/// prefix (never an empty prefix).
pub fn resolve_rule_paths(env: &Env, rule: &Rule) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in &rule.paths {
        let Some(path) = expand_rule_path(env, raw) else {
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let found = match name.strip_suffix('*') {
            None => vec![path],
            Some("") => Vec::new(),
            Some(prefix) => glob_siblings(env, &path, prefix),
        };
        for p in found {
            if std::fs::symlink_metadata(env.sys(&p.to_string_lossy())).is_ok() && !out.contains(&p)
            {
                out.push(p);
            }
        }
    }
    out
}

fn glob_siblings(env: &Env, pattern: &Path, prefix: &str) -> Vec<PathBuf> {
    let Some(parent) = pattern.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(env.sys(&parent.to_string_lossy())) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort();
    names.into_iter().map(|n| parent.join(n)).collect()
}

#[cfg(test)]
mod tests;
