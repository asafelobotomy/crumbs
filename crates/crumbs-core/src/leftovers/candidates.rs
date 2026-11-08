//! Enumerates the per-user paths that may be leftovers (spec §4).

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::Config;
use crate::desktop::{DesktopEntry, parse_desktop, split_exec};
use crate::env::Env;
use crate::safety::{
    Fingerprint, Protection, fingerprint, is_allowed_user_target_by, protected_paths,
};
use crate::sources::appimage::normalize_name;

use super::FindingKind;
use super::score::strip_version;

/// A path that may be a leftover. `path` is in-system and canonical.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) path: PathBuf,
    pub(crate) kind: FindingKind,
    pub(crate) fingerprint: Fingerprint,
    pub(crate) subject: Subject,
}

/// What a candidate is matched by.
#[derive(Debug, Clone)]
pub(crate) enum Subject {
    /// A directory or file name (a dot entry's leading `.` stripped).
    Name(NameKey),
    /// `~/snap/<name>`. Never matched in M2 (no snap source); orphans only.
    // The name is read by orphan detection.
    #[allow(dead_code)]
    Snap(String),
    /// `~/.var/app/<id>`.
    Flatpak(String),
    /// An autostart or local `.desktop` file.
    Launcher(DesktopEntry),
    /// An icon file's stem.
    Icon(String),
    /// A user unit's `ExecStart=` program, as written.
    Service(Option<String>),
    /// `mimeapps.list` lines that name at least one desktop ID.
    Mime(Vec<MimeLine>),
    /// A path that only a rule names.
    RuleOnly,
}

/// A candidate name with its normalised forms, computed once.
#[derive(Debug, Clone)]
pub(crate) struct NameKey {
    pub(crate) raw: String,
    pub(crate) norm: String,
    /// `normalize_name(strip_version(raw))`.
    pub(crate) unversioned: String,
}

impl NameKey {
    pub(crate) fn new(raw: &str) -> NameKey {
        NameKey {
            raw: raw.to_string(),
            norm: normalize_name(raw),
            unversioned: normalize_name(strip_version(raw)),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MimeLine {
    pub(crate) line: String,
    pub(crate) desktop_ids: Vec<String>,
}

/// The in-system base directories candidates are enumerated from, made
/// canonical. `None` for a base that cannot be (contains `..`).
pub(crate) struct Bases {
    pub(crate) home: Option<PathBuf>,
    pub(crate) config: Option<PathBuf>,
    pub(crate) cache: Option<PathBuf>,
    pub(crate) data: Option<PathBuf>,
    pub(crate) state: Option<PathBuf>,
}

impl Bases {
    pub(crate) fn of(env: &Env) -> Bases {
        let c = |p: &Path| lexical(&env.unsys(p));
        Bases {
            home: c(&env.home),
            config: c(&env.config_home),
            cache: c(&env.cache_home),
            data: c(&env.data_home),
            state: c(&env.state_home),
        }
    }

    fn xdg(&self) -> [(&Option<PathBuf>, FindingKind); 4] {
        [
            (&self.config, FindingKind::Config),
            (&self.cache, FindingKind::Cache),
            (&self.data, FindingKind::Data),
            (&self.state, FindingKind::State),
        ]
    }

    /// Locations whose entries are enumerated one by one, so the location
    /// itself (or an ancestor of it) must never become a finding.
    pub(crate) fn containers(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        let mut add = |base: &Option<PathBuf>, rels: &[&str]| {
            if let Some(b) = base {
                v.extend(rels.iter().map(|r| b.join(r)));
            }
        };
        add(
            &self.data,
            &["applications", "icons", "Trash", "flatpak", "mime"],
        );
        add(&self.cache, &["thumbnails"]);
        add(
            &self.config,
            &["autostart", "systemd/user", "mimeapps.list"],
        );
        add(&self.home, &[".var/app", "snap", ".local"]);
        v
    }

    /// Kind for a path under no particular candidate location (rule paths):
    /// the deepest base it lies in.
    pub(crate) fn kind_for(&self, path: &Path) -> FindingKind {
        let mut best: Option<(usize, FindingKind)> = None;
        let home_rel = |rel: &str| self.home.as_ref().map(|h| h.join(rel));
        let bases = self.xdg().map(|(b, k)| (b.clone(), k));
        let extra = [
            (home_rel(".var/app"), FindingKind::FlatpakData),
            (home_rel("snap"), FindingKind::SnapData),
        ];
        for (base, kind) in bases.into_iter().chain(extra) {
            if let Some(b) = base
                && path.starts_with(&b)
                && path != b
            {
                let depth = b.components().count();
                if best.is_none_or(|(d, _)| depth > d) {
                    best = Some((depth, kind));
                }
            }
        }
        best.map_or(FindingKind::DotDir, |(_, k)| k)
    }
}

/// Lexically canonical form: absolute, no `.`, no trailing `/`. `None` for
/// relative paths and paths containing `..`.
pub(crate) fn lexical(p: &Path) -> Option<PathBuf> {
    if !p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(n) => out.push(n),
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// A candidate for `path` when it is an allowed user target (checked against
/// `protected`, from [`protected_paths`]) that can be fingerprinted; `None`
/// otherwise (fail closed).
pub(crate) fn admit(
    env: &Env,
    cfg: &Config,
    protected: &[(PathBuf, Protection)],
    path: PathBuf,
    kind: FindingKind,
    subject: Subject,
) -> Option<Candidate> {
    if !is_allowed_user_target_by(env, cfg, protected, &path) {
        return None;
    }
    let fingerprint = fingerprint(env, &path).ok()?;
    Some(Candidate {
        path,
        kind,
        fingerprint,
        subject,
    })
}

/// `(in-system path, file name)` of the entries of an in-system directory,
/// sorted. Names that are not UTF-8 are skipped.
fn children(env: &Env, dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(rd) = fs::read_dir(env.sys(&dir.to_string_lossy())) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, String)> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .map(|n| (dir.join(&n), n))
        .collect();
    out.sort();
    out
}

fn read(env: &Env, in_system: &Path) -> Option<Vec<u8>> {
    fs::read(env.sys(&in_system.to_string_lossy())).ok()
}

fn is_real_dir(env: &Env, in_system: &Path) -> bool {
    fs::symlink_metadata(env.sys(&in_system.to_string_lossy())).is_ok_and(|m| m.is_dir())
}

/// Every candidate, each path at most once.
pub(crate) fn enumerate(env: &Env, cfg: &Config) -> Vec<Candidate> {
    let bases = Bases::of(env);
    let containers = bases.containers();
    let mut raw: Vec<(PathBuf, FindingKind, Subject)> = Vec::new();

    for (base, kind) in bases.xdg() {
        let Some(base) = base else { continue };
        for (path, name) in children(env, base) {
            if !holds_container(&containers, &path) {
                raw.push((path, kind, Subject::Name(NameKey::new(&name))));
            }
        }
    }
    if let Some(home) = &bases.home {
        let skip: Vec<&PathBuf> = [&bases.config, &bases.cache, &bases.data, &bases.state]
            .into_iter()
            .flatten()
            .collect();
        for (path, name) in children(env, home) {
            let Some(bare) = name.strip_prefix('.') else {
                continue;
            };
            if skip.contains(&&path) || holds_container(&containers, &path) {
                continue;
            }
            raw.push((path, FindingKind::DotDir, Subject::Name(NameKey::new(bare))));
        }
        for (path, id) in children(env, &home.join(".var/app")) {
            raw.push((path, FindingKind::FlatpakData, Subject::Flatpak(id)));
        }
        for (path, name) in children(env, &home.join("snap")) {
            raw.push((path, FindingKind::SnapData, Subject::Snap(name)));
        }
    }
    if let Some(config) = &bases.config {
        launchers(
            env,
            &config.join("autostart"),
            FindingKind::Autostart,
            &mut raw,
        );
        services(env, &config.join("systemd/user"), &mut raw);
        let mime = config.join("mimeapps.list");
        if let Some(bytes) = read(env, &mime) {
            raw.push((
                mime,
                FindingKind::MimeAssoc,
                Subject::Mime(mime_lines(&bytes)),
            ));
        }
    }
    if let Some(data) = &bases.data {
        launchers(
            env,
            &data.join("applications"),
            FindingKind::DesktopEntry,
            &mut raw,
        );
        icons(env, &data.join("icons"), &mut raw);
    }

    let protected = protected_paths(env, cfg);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    for (path, kind, subject) in raw {
        if seen.insert(path.clone()) {
            out.extend(admit(env, cfg, &protected, path, kind, subject));
        }
    }
    out
}

/// True when `path` is a container location or an ancestor of one.
pub(crate) fn holds_container(containers: &[PathBuf], path: &Path) -> bool {
    containers.iter().any(|c| c.starts_with(path))
}

fn launchers(
    env: &Env,
    dir: &Path,
    kind: FindingKind,
    raw: &mut Vec<(PathBuf, FindingKind, Subject)>,
) {
    for (path, name) in children(env, dir) {
        if !name.ends_with(".desktop") {
            continue;
        }
        let Some(bytes) = read(env, &path) else {
            continue;
        };
        let entry = parse_desktop(&name, &path, &bytes);
        raw.push((path, kind, Subject::Launcher(entry)));
    }
}

fn services(env: &Env, dir: &Path, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
    for (path, name) in children(env, dir) {
        if !name.ends_with(".service") {
            continue;
        }
        let Some(bytes) = read(env, &path) else {
            continue;
        };
        let program = exec_start_program(&String::from_utf8_lossy(&bytes));
        raw.push((path, FindingKind::SystemdUnit, Subject::Service(program)));
    }
}

/// The program of the first `ExecStart=` in `[Service]`, without systemd's
/// `-@:+!` prefixes or a leading `env VAR=…`.
pub(crate) fn exec_start_program(text: &str) -> Option<String> {
    let mut in_service = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_service = line == "[Service]";
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if !in_service || k.trim() != "ExecStart" {
            continue;
        }
        let toks = split_exec(v.trim().trim_start_matches(['-', '@', ':', '+', '!']));
        let mut it = toks.into_iter().peekable();
        if it
            .peek()
            .is_some_and(|t| Path::new(t).file_name().is_some_and(|n| n == "env"))
        {
            it.next();
            while it.peek().is_some_and(|t| t.contains('=')) {
                it.next();
            }
        }
        return it.next();
    }
    None
}

fn mime_lines(bytes: &[u8]) -> Vec<MimeLine> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.starts_with('[') || t.starts_with('#') {
                return None;
            }
            let (_, v) = t.split_once('=')?;
            let desktop_ids: Vec<String> = v
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            (!desktop_ids.is_empty()).then(|| MimeLine {
                line: line.to_string(),
                desktop_ids,
            })
        })
        .collect()
}

/// Icon files directly in `icons/` or anywhere under `icons/hicolor/`.
/// Other theme directories belong to that theme, not to an app.
fn icons(env: &Env, dir: &Path, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
    let mut stack: Vec<PathBuf> = Vec::new();
    for (path, name) in children(env, dir) {
        if is_real_dir(env, &path) {
            if name == "hicolor" {
                stack.push(path);
            }
        } else {
            push_icon(path, raw);
        }
    }
    while let Some(d) = stack.pop() {
        for (path, _) in children(env, &d) {
            if is_real_dir(env, &path) {
                stack.push(path);
            } else {
                push_icon(path, raw);
            }
        }
    }
}

fn push_icon(path: PathBuf, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let stem = stem.to_string();
        raw.push((path, FindingKind::Icon, Subject::Icon(stem)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_table() {
        let l = |s: &str| lexical(Path::new(s)).map(|p| p.display().to_string());
        assert_eq!(l("/a/b/").as_deref(), Some("/a/b"));
        assert_eq!(l("/a/./b").as_deref(), Some("/a/b"));
        assert_eq!(l("/a//b").as_deref(), Some("/a/b"));
        assert_eq!(l("/a/../b"), None);
        assert_eq!(l("a/b"), None);
    }

    #[test]
    fn exec_start_table() {
        let p = |s: &str| exec_start_program(s);
        assert_eq!(
            p("[Service]\nExecStart=-/usr/bin/foo --x\n").as_deref(),
            Some("/usr/bin/foo")
        );
        assert_eq!(
            p("[Service]\nExecStart=/usr/bin/env A=1 B=2 bar\n").as_deref(),
            Some("bar")
        );
        assert_eq!(p("[Unit]\nExecStart=/usr/bin/foo\n"), None);
        assert_eq!(
            p("[Service]\nExecStart=\"/opt/My App/run\" x\n").as_deref(),
            Some("/opt/My App/run")
        );
    }
}
