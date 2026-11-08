use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::desktop::{DesktopEntry, exec_basename};
use crate::env::Env;
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

        for (dir, needs_ext) in scan_dirs(cx.env) {
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
fn scan_dirs(env: &Env) -> Vec<(PathBuf, bool)> {
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
    if let Some(prog) = entry.exec_program.as_deref()
        && expand_in_system(env, prog) == location
    {
        return true;
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

/// True when the bytes start with an ELF header carrying the AppImage type-2 magic.
pub fn is_appimage(bytes: &[u8]) -> bool {
    bytes.len() >= 11 && bytes[0..4] == *b"\x7fELF" && bytes[8..11] == *b"AI\x02"
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
mod tests {
    use super::*;
    use crate::desktop::discover_desktop_entries;
    use crate::model::Installation;
    use crate::sources::ListOptions;
    use crate::testing::{FakeRunner, FixtureTree};

    fn list(t: &FixtureTree) -> SourceListing {
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let runner = FakeRunner::new();
        let opts = ListOptions::default();
        let cx = SourceContext {
            env: &env,
            runner: &runner,
            desktop: &desktop,
            opts: &opts,
        };
        AppImageSource.list(&cx).unwrap()
    }

    #[test]
    fn split_names() {
        let cases = [
            ("emobie_0.8.2_amd64.AppImage", "emobie", Some("0.8.2")),
            ("Grok_Bot_0.79.2.AppImage", "Grok_Bot", Some("0.79.2")),
            (
                "GutCheck-0.19.0-x86_64.AppImage",
                "GutCheck",
                Some("0.19.0"),
            ),
            ("Pass_Broker-x86_64.AppImage", "Pass_Broker", None),
            ("tool-v2.1.AppImage", "tool", Some("2.1")),
            ("Foo-1.0-x86_64.appimage", "Foo", Some("1.0")),
        ];
        for (file, name, ver) in cases {
            assert_eq!(
                split_appimage_name(file),
                (name.to_string(), ver.map(String::from)),
                "{file}"
            );
        }
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_name("Grok_Bot-1.x"), "grokbot1x");
    }

    #[test]
    fn magic_detection() {
        let mut ok = vec![0x7f, b'E', b'L', b'F', 0, 0, 0, 0, b'A', b'I', 0x02];
        ok.resize(16, 0);
        assert!(is_appimage(&ok));
        let mut elf = ok.clone();
        elf[8] = 0;
        assert!(!is_appimage(&elf));
        assert!(!is_appimage(&[0x7f, b'E', b'L']));
    }

    #[test]
    fn lists_appimages_in_applications_dir() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
        let l = list(&t);
        assert_eq!(l.apps.len(), 1);
        let a = &l.apps[0];
        assert_eq!(a.id, "appimage:GutCheck-0.19.0-x86_64.AppImage");
        assert_eq!(a.display_name, "GutCheck");
        assert_eq!(a.version.as_deref(), Some("0.19.0"));
        assert_eq!(a.installation, Installation::User);
        assert_eq!(a.install_size, Some(256));
        assert!(a.has_gui);
        let env = t.env();
        assert_eq!(
            a.location.as_deref(),
            Some(
                env.unsys(&env.home)
                    .join("Applications/GutCheck-0.19.0-x86_64.AppImage")
                    .as_path()
            )
        );
    }

    #[test]
    fn hostile_dir_entries_skipped() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Applications/real.AppImage");
        t.symlink("/home/user/Applications/broken.AppImage", "/nonexistent");
        t.symlink("/home/user/Applications/link.AppImage", "real.AppImage");
        t.mkdir("/home/user/Applications/x.AppImage");
        t.write("/home/user/Applications/empty.AppImage", b"");
        t.write(
            "/home/user/Applications/text.AppImage",
            b"hello world, not elf\n",
        );
        let l = list(&t);
        let ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["appimage:real.AppImage"]);
    }

    #[test]
    fn extension_required_only_in_downloads_and_local_bin() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Downloads/noext");
        t.appimage("/home/user/.local/bin/noext2");
        t.appimage("/home/user/.local/bin/yes.AppImage");
        t.appimage("/home/user/opt/noext3");
        let l = list(&t);
        let mut ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["appimage:noext3", "appimage:yes.AppImage"]);
    }

    #[test]
    fn links_desktop_by_exec_path() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Applications/Foo-1.0-x86_64.AppImage");
        t.desktop(
            "/home/user/.local/share/applications/whatever.desktop",
            "[Desktop Entry]\nType=Application\nName=Pretty Foo\nExec=~/Applications/Foo-1.0-x86_64.AppImage %U\nX-AppImage-Version=1.0\n",
        );
        t.desktop(
            "/home/user/.local/share/applications/other.desktop",
            "[Desktop Entry]\nType=Application\nName=Other\nExec=/usr/bin/other\n",
        );
        let l = list(&t);
        let a = &l.apps[0];
        assert_eq!(a.desktop_ids, ["whatever.desktop"]);
        assert_eq!(a.display_name, "Pretty Foo");
        assert_eq!(a.exec_names, ["Foo-1.0-x86_64.AppImage"]);
        assert!(l.claimed_desktop_ids.contains("whatever.desktop"));
        assert!(!l.claimed_desktop_ids.contains("other.desktop"));
    }

    #[test]
    fn links_absolute_exec_path() {
        let t = FixtureTree::new();
        t.appimage("/home/user/opt/Bar.AppImage");
        t.desktop(
            "/usr/share/applications/bar.desktop",
            "[Desktop Entry]\nType=Application\nName=Bar\nExec=/home/user/opt/Bar.AppImage\n",
        );
        let l = list(&t);
        assert_eq!(l.apps[0].desktop_ids, ["bar.desktop"]);
    }

    #[test]
    fn links_appofholding_style_entry() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
        t.appimage("/home/user/Applications/Pass_Broker-x86_64.AppImage");
        t.desktop(
            "/home/user/.local/share/applications/gutcheck.desktop",
            "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/usr/bin/appofholding run gutcheck %U\nX-AppImage-Version=0.19.0\n",
        );
        t.desktop(
            "/home/user/.local/share/applications/pass-broker.desktop",
            "[Desktop Entry]\nType=Application\nName=Pass Broker\nExec=/usr/bin/appofholding run pass-broker\nX-AppImage-Version=2.3\n",
        );
        t.desktop(
            "/usr/share/applications/gutcheck-native.desktop",
            "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/usr/bin/gutcheck\n",
        );
        let l = list(&t);
        let gut = l
            .apps
            .iter()
            .find(|a| a.name.starts_with("GutCheck"))
            .unwrap();
        assert_eq!(gut.desktop_ids, ["gutcheck.desktop"]);
        assert_eq!(gut.exec_names, ["appofholding"]);
        assert!(l.claimed_desktop_ids.contains("gutcheck.desktop"));
        assert!(!l.claimed_desktop_ids.contains("gutcheck-native.desktop"));
        let pb = l
            .apps
            .iter()
            .find(|a| a.name.starts_with("Pass_Broker"))
            .unwrap();
        assert_eq!(pb.desktop_ids, ["pass-broker.desktop"]);
        assert_eq!(pb.display_name, "Pass Broker");
        assert_eq!(pb.version.as_deref(), Some("2.3"));
    }

    #[test]
    fn appimagelauncher_destination_scanned() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/appimagelauncher.cfg",
            b"[AppImageLauncher]\nenable_daemon = false\ndestination = ~/Software\n",
        );
        t.appimage("/home/user/Software/Baz.AppImage");
        let l = list(&t);
        assert_eq!(l.apps.len(), 1);
        assert_eq!(l.apps[0].id, "appimage:Baz.AppImage");
    }

    #[test]
    fn launcher_destination_equal_to_default_not_duplicated() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/appimagelauncher.cfg",
            b"[AppImageLauncher]\ndestination = ~/Applications\n",
        );
        t.appimage("/home/user/Applications/Baz.AppImage");
        assert_eq!(list(&t).apps.len(), 1);
    }
}
