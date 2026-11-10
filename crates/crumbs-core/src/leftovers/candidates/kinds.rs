//! Launchers, services and icons found as leftover candidates.

use super::*;

pub(super) fn launchers(
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

pub(super) fn services(env: &Env, dir: &Path, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
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

pub(super) fn mime_lines(bytes: &[u8]) -> Vec<MimeLine> {
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
pub(super) fn icons(env: &Env, dir: &Path, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
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

pub(super) fn push_icon(path: PathBuf, raw: &mut Vec<(PathBuf, FindingKind, Subject)>) {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let stem = stem.to_string();
        raw.push((path, FindingKind::Icon, Subject::Icon(stem)));
    }
}
