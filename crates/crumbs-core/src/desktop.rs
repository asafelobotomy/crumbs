//! Desktop entry parsing and discovery.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::env::Env;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesktopEntry {
    pub id: String,
    pub path: PathBuf,
    pub name: Option<String>,
    pub exec: Option<String>,
    pub exec_program: Option<String>,
    pub exec_args: Vec<String>,
    pub try_exec: Option<String>,
    pub wm_class: Option<String>,
    pub no_display: bool,
    pub hidden: bool,
    pub is_application: bool,
    pub appimage_version: Option<String>,
    pub is_appimage_entry: bool,
    pub is_flatpak_entry: bool,
}

const FIELD_CODES: &str = "fFuUdDnNickvm";

pub fn parse_desktop(id: &str, path: &Path, bytes: &[u8]) -> DesktopEntry {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let mut keys: HashMap<&str, &str> = HashMap::new();
    let mut appimage_entry = false;
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.contains('[') {
            continue;
        }
        if k.starts_with("X-AppImage") {
            appimage_entry = true;
        }
        keys.entry(k).or_insert(v);
    }

    let get = |k: &str| keys.get(k).map(|v| v.to_string());
    let flag = |k: &str| keys.get(k).is_some_and(|v| *v == "true");

    let exec = get("Exec");
    let (exec_program, exec_args) = exec
        .as_deref()
        .map(program_and_args)
        .unwrap_or((None, Vec::new()));

    DesktopEntry {
        id: id.to_string(),
        path: path.to_path_buf(),
        name: get("Name"),
        exec,
        exec_program,
        exec_args,
        try_exec: get("TryExec"),
        wm_class: get("StartupWMClass"),
        no_display: flag("NoDisplay"),
        hidden: flag("Hidden"),
        is_application: keys.get("Type") == Some(&"Application"),
        appimage_version: get("X-AppImage-Version"),
        is_appimage_entry: appimage_entry,
        is_flatpak_entry: keys.contains_key("X-Flatpak"),
    }
}

fn is_env_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn program_and_args(exec: &str) -> (Option<String>, Vec<String>) {
    let mut toks = split_exec(exec);
    let mut skip = 0;
    if toks.first().map(String::as_str) == Some("env") {
        skip = 1;
        while toks.get(skip).is_some_and(|t| is_env_assignment(t)) {
            skip += 1;
        }
    }
    toks.drain(..skip);
    if toks.is_empty() {
        return (None, Vec::new());
    }
    let program = toks.remove(0);
    (Some(program), toks)
}

/// Tokenizes an `Exec` value per the Desktop Entry spec, dropping field codes.
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    // True once the token has real content (text or a quoted section).
    let mut live = false;
    let mut in_quote = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quote = !in_quote;
                live = true;
            }
            '\\' if in_quote => match chars.peek() {
                Some(&n) if matches!(n, '"' | '`' | '$' | '\\') => {
                    cur.push(n);
                    chars.next();
                }
                _ => cur.push('\\'),
            },
            '%' => match chars.peek() {
                Some('%') => {
                    cur.push('%');
                    live = true;
                    chars.next();
                }
                Some(&n) if FIELD_CODES.contains(n) => {
                    chars.next();
                }
                _ => {
                    cur.push('%');
                    live = true;
                }
            },
            c if c.is_whitespace() && !in_quote => {
                if live {
                    out.push(std::mem::take(&mut cur));
                    live = false;
                }
            }
            c => {
                cur.push(c);
                live = true;
            }
        }
    }
    if live {
        out.push(cur);
    }
    out
}

fn collect_dir(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        let path = item.path();
        // `DirEntry::file_type` does not follow symlinks, so symlinked
        // directories are skipped (no loops) while symlinked files are read.
        let Ok(ft) = item.file_type() else {
            continue;
        };
        if ft.is_dir() {
            collect_dir(&path, &format!("{rel}{name}-"), out);
        } else if name.ends_with(".desktop") && (ft.is_file() || path.is_file()) {
            out.push((format!("{rel}{name}"), path));
        }
    }
}

/// Finds desktop entries; earlier directories shadow later ones by ID.
pub fn discover_desktop_entries(env: &Env) -> Vec<DesktopEntry> {
    let dirs = std::iter::once(&env.data_home).chain(env.data_dirs.iter());
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for base in dirs {
        let mut found = Vec::new();
        collect_dir(&base.join("applications"), "", &mut found);
        for (id, path) in found {
            if seen.contains(&id) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            seen.insert(id.clone());
            entries.push(parse_desktop(&id, &path, &bytes));
        }
    }
    entries
}

pub fn exec_basename(e: &DesktopEntry) -> Option<String> {
    let prog = e.exec_program.as_deref()?;
    Path::new(prog)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    fn parse(s: &str) -> DesktopEntry {
        parse_desktop("t.desktop", Path::new("/x/t.desktop"), s.as_bytes())
    }

    #[test]
    fn parses_main_group_only() {
        let e = parse(
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=foo\n\n[Desktop Action new]\nName=New\nExec=other\n",
        );
        assert_eq!(e.exec.as_deref(), Some("foo"));
        assert_eq!(e.name.as_deref(), Some("Foo"));
        assert!(e.is_application);
    }

    #[test]
    fn ignores_localized_keys() {
        let e = parse("[Desktop Entry]\nName[de]=Feuerfuchs\nName=Firefox\n");
        assert_eq!(e.name.as_deref(), Some("Firefox"));
    }

    #[test]
    fn handles_bom_and_crlf() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"[Desktop Entry]\r\nName=Foo\r\nExec=foo %U\r\nNoDisplay=true\r\n");
        let e = parse_desktop("t.desktop", Path::new("/t"), &b);
        assert_eq!(e.name.as_deref(), Some("Foo"));
        assert_eq!(e.exec.as_deref(), Some("foo %U"));
        assert!(e.no_display);
    }

    #[test]
    fn first_duplicate_key_wins() {
        let e = parse("[Desktop Entry]\nName=One\nName=Two\n");
        assert_eq!(e.name.as_deref(), Some("One"));
    }

    #[test]
    fn exec_env_prefix() {
        let e = parse("[Desktop Entry]\nExec=env FOO=1 BAR=2 /opt/x/x %U\n");
        assert_eq!(e.exec_program.as_deref(), Some("/opt/x/x"));
        assert!(e.exec_args.is_empty());
    }

    #[test]
    fn exec_quoted_path() {
        let e = parse("[Desktop Entry]\nExec=\"/opt/My App/app\" %f\n");
        assert_eq!(e.exec_program.as_deref(), Some("/opt/My App/app"));
    }

    #[test]
    fn exec_flatpak_run() {
        let e = parse("[Desktop Entry]\nExec=flatpak run org.x.Y\n");
        assert_eq!(e.exec_program.as_deref(), Some("flatpak"));
        assert_eq!(e.exec_args, ["run", "org.x.Y"]);
    }

    #[test]
    fn exec_percent_escape() {
        let e = parse("[Desktop Entry]\nExec=foo 100%%\n");
        assert_eq!(e.exec_args, ["100%"]);
    }

    #[test]
    fn empty_exec_has_no_program() {
        let e = parse("[Desktop Entry]\nExec=   \n");
        assert_eq!(e.exec_program, None);
    }

    #[test]
    fn appimage_keys_detected() {
        let e = parse("[Desktop Entry]\nX-AppImage-Version=0.19.0\n");
        assert!(e.is_appimage_entry);
        assert_eq!(e.appimage_version.as_deref(), Some("0.19.0"));
    }

    #[test]
    fn flags_and_wm_class() {
        let e = parse("[Desktop Entry]\nType=Link\nHidden=true\nStartupWMClass=Foo\nTryExec=foo\n");
        assert!(!e.is_application);
        assert!(e.hidden);
        assert_eq!(e.wm_class.as_deref(), Some("Foo"));
        assert_eq!(e.try_exec.as_deref(), Some("foo"));
    }

    #[test]
    fn user_entry_shadows_system() {
        let t = FixtureTree::new();
        t.desktop(
            "/home/user/.local/share/applications/foo.desktop",
            "[Desktop Entry]\nName=User\n",
        );
        t.desktop(
            "/usr/share/applications/foo.desktop",
            "[Desktop Entry]\nName=System\n",
        );
        let env = t.env();
        let all = discover_desktop_entries(&env);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name.as_deref(), Some("User"));
        assert!(all[0].path.starts_with(&env.data_home));
    }

    #[test]
    fn subdir_id_uses_dash() {
        let t = FixtureTree::new();
        t.desktop(
            "/usr/share/applications/kde4/kate.desktop",
            "[Desktop Entry]\nName=Kate\n",
        );
        let all = discover_desktop_entries(&t.env());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "kde4-kate.desktop");
    }

    #[test]
    fn directory_symlinks_not_followed() {
        let t = FixtureTree::new();
        t.desktop(
            "/usr/share/applications/a.desktop",
            "[Desktop Entry]\nName=A\n",
        );
        t.symlink("/usr/share/applications/loop", "/usr/share/applications");
        t.symlink("/usr/share/applications/b.desktop", "a.desktop");
        let mut ids: Vec<_> = discover_desktop_entries(&t.env())
            .into_iter()
            .map(|e| e.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["a.desktop", "b.desktop"]);
    }

    #[test]
    fn basename_of_program() {
        let e = parse("[Desktop Entry]\nExec=/opt/x/bin/app %U\n");
        assert_eq!(exec_basename(&e).as_deref(), Some("app"));
    }
}
