use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::desktop::{DesktopEntry, exec_basename};
use crate::env::{Env, display_path};
use crate::error::CoreError;
use crate::model::{App, Installation, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

const ARCH_TOKENS: [&str; 7] = [
    "x86_64", "amd64", "aarch64", "arm64", "i386", "i686", "armhf",
];
const SUFFIX: &str = ".appimage";

pub struct AppImageSource;

impl AppSource for AppImageSource {
    fn kind(&self) -> SourceKind {
        SourceKind::AppImage
    }

    fn available(&self, _env: &Env) -> bool {
        true
    }

    fn is_package_db(&self) -> bool {
        false
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let mut listing = SourceListing::default();
        let mut desktop: Vec<&DesktopEntry> = cx.desktop.iter().collect();
        desktop.sort_by(|a, b| a.id.cmp(&b.id));

        for (dir, needs_ext) in scan_dirs(cx.env, cx.config) {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<_> = rd.flatten().collect();
            files.sort_by_key(|e| e.file_name());
            for item in files {
                let file_name = item.file_name().to_string_lossy().into_owned();
                if needs_ext && !file_name.to_ascii_lowercase().ends_with(SUFFIX) {
                    continue;
                }
                let path = item.path();
                let Some(len) = appimage_len(&path) else {
                    continue;
                };
                listing
                    .apps
                    .push(build_app(cx.env, &desktop, &file_name, &path, len));
            }
        }
        for app in &listing.apps {
            listing
                .claimed_desktop_ids
                .extend(app.desktop_ids.iter().cloned());
        }
        listing.apps.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(listing)
    }
}

/// Directories to scan, with whether the `.AppImage` extension is required.
/// Duplicates (e.g. a launcher destination equal to a default) appear once.
/// appOFholding's library is left out: the AppImages there are its to list,
/// update and remove, so crumBS neither lists them as apps nor offers their
/// old versions to `appimage-old` nor uninstalls them by path.
fn scan_dirs(env: &Env, cfg: &Config) -> Vec<(PathBuf, bool)> {
    let held = appofholding_library(env);
    let mut dirs = all_scan_dirs(env, cfg);
    dirs.retain(|(d, _)| Some(d) != held.as_ref());
    dirs
}

/// appOFholding's library directory, when appOFholding is in use (its
/// database exists). It always keeps its AppImages in `~/Applications`.
pub fn appofholding_library(env: &Env) -> Option<PathBuf> {
    env.data_home
        .join("appofholding/db.sqlite")
        .is_file()
        .then(|| env.home.join("Applications"))
}

fn all_scan_dirs(env: &Env, cfg: &Config) -> Vec<(PathBuf, bool)> {
    let mut dirs = vec![
        (env.home.join("Applications"), false),
        (env.home.join(".local/bin"), true),
        (env.home.join("Downloads"), true),
        (env.home.join("opt"), false),
    ];
    if let Some(dest) = launcher_destination(env)
        && !dirs.iter().any(|(d, _)| *d == dest)
    {
        dirs.push((dest, false));
    }
    for extra in &cfg.appimage_dirs {
        let dir = env.sys(&extra.to_string_lossy());
        if !dirs.iter().any(|(d, _)| *d == dir) {
            dirs.push((dir, false));
        }
    }
    dirs
}

fn launcher_destination(env: &Env) -> Option<PathBuf> {
    let text = std::fs::read_to_string(env.config_home.join("appimagelauncher.cfg")).ok()?;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[AppImageLauncher]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == "destination" {
            let v = v.trim().trim_matches('"');
            return if v == "~" {
                Some(env.home.clone())
            } else if let Some(rest) = v.strip_prefix("~/") {
                Some(env.home.join(rest))
            } else if v.starts_with('/') {
                Some(env.sys(v))
            } else {
                None
            };
        }
    }
    None
}

/// Returns the file length when `path` is a regular (non-symlink) AppImage.
fn appimage_len(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let mut head = Vec::with_capacity(16);
    File::open(path)
        .ok()?
        .take(16)
        .read_to_end(&mut head)
        .ok()?;
    is_appimage(&head).then_some(meta.len())
}

/// Expands a leading `~` to the in-system home directory.
fn expand_in_system(env: &Env, s: &str) -> PathBuf {
    let home = env.unsys(&env.home);
    if s == "~" {
        home
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(s)
    }
}

fn links(env: &Env, entry: &DesktopEntry, location: &Path, norm_name: &str) -> bool {
    if let Some(prog) = entry.exec_program.as_deref() {
        let exec = expand_in_system(env, prog);
        if exec == location {
            return true;
        }
        if exec.is_absolute()
            && let (Ok(a), Ok(b)) = (
                env.sys(&exec.to_string_lossy()).canonicalize(),
                env.sys(&location.to_string_lossy()).canonicalize(),
            )
            && a == b
        {
            return true;
        }
    }
    if !entry.is_appimage_entry || norm_name.is_empty() {
        return false;
    }
    let id_stem = entry.id.strip_suffix(".desktop").unwrap_or(&entry.id);
    entry
        .name
        .as_deref()
        .is_some_and(|n| normalize_name(n) == norm_name)
        || normalize_name(id_stem) == norm_name
}

fn build_app(env: &Env, desktop: &[&DesktopEntry], file_name: &str, path: &Path, len: u64) -> App {
    let (base, file_version) = split_appimage_name(file_name);
    let location = env.unsys(path);
    let norm_name = normalize_name(&base);

    let mut app = App::new(SourceKind::AppImage, file_name);
    app.id = format!("appimage:{}", display_path(env, &location));
    app.installation = Installation::User;
    app.install_size = Some(len);
    app.has_gui = true;
    app.display_name = base;
    app.version = file_version;
    app.location = Some(location.clone());

    let mut named = false;
    for entry in desktop
        .iter()
        .filter(|e| links(env, e, &location, &norm_name))
    {
        app.desktop_ids.push(entry.id.clone());
        if !named && let Some(n) = entry.name.as_deref().filter(|n| !n.is_empty()) {
            app.display_name = n.to_string();
            named = true;
        }
        if app.version.is_none() {
            app.version = entry.appimage_version.clone();
        }
        if let Some(b) = exec_basename(entry)
            && !app.exec_names.contains(&b)
        {
            app.exec_names.push(b);
        }
    }
    app
}

/// True when the bytes start with an ELF header carrying the AppImage type-1
/// (`AI\x01`) or type-2 (`AI\x02`) magic.
pub fn is_appimage(bytes: &[u8]) -> bool {
    bytes.len() >= 11
        && bytes[0..4] == *b"\x7fELF"
        && (bytes[8..11] == *b"AI\x02" || bytes[8..11] == *b"AI\x01")
}

/// Splits an AppImage file name into a name and an optional version.
pub fn split_appimage_name(file_name: &str) -> (String, Option<String>) {
    let mut stem = file_name;
    if stem.len() >= SUFFIX.len() && stem.is_char_boundary(stem.len() - SUFFIX.len()) {
        let cut = stem.len() - SUFFIX.len();
        if stem[cut..].eq_ignore_ascii_case(SUFFIX) {
            stem = &stem[..cut];
        }
    }
    'arch: loop {
        let lower = stem.to_ascii_lowercase();
        for arch in ARCH_TOKENS {
            if let Some(head) = lower.strip_suffix(arch)
                && head.ends_with(['-', '_', '.'])
                && head.len() > 1
            {
                stem = &stem[..head.len() - 1];
                continue 'arch;
            }
        }
        break;
    }
    if let Some(i) = stem.rfind(['-', '_']) {
        let (rest, seg) = (&stem[..i], &stem[i + 1..]);
        let ver = match seg.strip_prefix('v') {
            Some(v) if v.starts_with(|c: char| c.is_ascii_digit()) => Some(v),
            _ if seg.starts_with(|c: char| c.is_ascii_digit()) => Some(seg),
            _ => None,
        };
        if let Some(v) = ver
            && !rest.is_empty()
        {
            return (rest.to_string(), Some(v.to_string()));
        }
    }
    (stem.to_string(), None)
}

/// Lowercases and drops `-`, `_`, `.` and spaces, for loose name matching.
pub fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests;
