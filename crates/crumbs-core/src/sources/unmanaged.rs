//! Desktop entries that no package manager, Flatpak or AppImage claims.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::command::TRUSTED_DIRS;
use crate::desktop::{DesktopEntry, exec_basename};
use crate::env::Env;
use crate::error::CoreError;
use crate::model::{App, Installation, SourceKind};

use super::appimage::is_appimage;
use super::{AppSource, SourceContext, SourceListing};

pub struct UnmanagedSource;

impl AppSource for UnmanagedSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Unmanaged
    }

    fn available(&self, _env: &Env) -> bool {
        true
    }

    fn is_package_db(&self) -> bool {
        false
    }

    /// Unmanaged apps depend on the other sources' ownership data, so
    /// `discover` calls [`list_unmanaged`] instead.
    fn list(&self, _cx: &SourceContext) -> Result<SourceListing, CoreError> {
        Ok(SourceListing::default())
    }
}

/// Lists desktop entries not claimed by `claimed` whose program is not in `owned`.
pub fn list_unmanaged(
    cx: &SourceContext,
    owned: &HashSet<PathBuf>,
    claimed: &HashSet<String>,
) -> SourceListing {
    let mut listing = SourceListing::default();
    let mut entries: Vec<&DesktopEntry> = cx.desktop.iter().collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    for entry in entries {
        if let Some(app) = unmanaged_app(cx.env, entry, owned, claimed) {
            listing.apps.push(app);
        }
    }
    listing
}

fn unmanaged_app(
    env: &Env,
    entry: &DesktopEntry,
    owned: &HashSet<PathBuf>,
    claimed: &HashSet<String>,
) -> Option<App> {
    if !entry.is_application
        || entry.no_display
        || entry.hidden
        || entry.is_appimage_entry
        || entry.is_flatpak_entry
        || claimed.contains(&entry.id)
    {
        return None;
    }
    let program = entry.exec_program.as_deref()?;
    if Path::new(program)
        .file_name()
        .is_some_and(|n| n == "flatpak")
    {
        return None;
    }
    let real = resolve_program(env, program)?;
    // Fail closed: a program we cannot resolve or read might be an AppImage.
    let canon = real.canonicalize().ok()?;
    if file_is_appimage(&canon)? {
        return None;
    }
    let in_system = env.unsys(&real);
    if owned.contains(&in_system) {
        return None;
    }
    if owned.contains(&env.unsys(&canon)) {
        return None;
    }

    let name = entry.id.strip_suffix(".desktop").unwrap_or(&entry.id);
    let mut app = App::new(SourceKind::Unmanaged, &entry.id);
    app.name = name.to_string();
    app.display_name = entry
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| name.to_string());
    app.installation = if entry.path.starts_with(&env.home) || real.starts_with(&env.home) {
        Installation::User
    } else {
        Installation::System
    };
    app.location = Some(location_of(&in_system));
    app.desktop_ids.push(entry.id.clone());
    app.exec_names.extend(exec_basename(entry));
    app.wm_classes.extend(entry.wm_class.clone());
    app.has_gui = true;
    Some(app)
}

/// `None` when the file's first bytes cannot be read.
fn file_is_appimage(path: &Path) -> Option<bool> {
    use std::io::Read;
    let mut head = Vec::with_capacity(16);
    std::fs::File::open(path)
        .and_then(|f| f.take(16).read_to_end(&mut head))
        .ok()?;
    Some(is_appimage(&head))
}

/// `/opt/<first component>` for programs under `/opt/`, else the program itself.
fn location_of(program: &Path) -> PathBuf {
    match program.strip_prefix("/opt") {
        Ok(rest) => match rest.components().next() {
            Some(first) => Path::new("/opt").join(first),
            None => program.to_path_buf(),
        },
        Err(_) => program.to_path_buf(),
    }
}

/// Resolves an `Exec` program to a real path under the root, if it exists.
fn resolve_program(env: &Env, program: &str) -> Option<PathBuf> {
    let home = env.unsys(&env.home);
    let candidates: Vec<PathBuf> = if program == "~" {
        Vec::new()
    } else if let Some(rest) = program.strip_prefix("~/") {
        vec![env.sys(&home.join(rest).to_string_lossy())]
    } else if program.starts_with('/') {
        vec![env.sys(program)]
    } else if program.contains('/') {
        Vec::new()
    } else {
        TRUSTED_DIRS
            .iter()
            .map(|d| env.sys(d).join(program))
            .chain(std::iter::once(env.home.join(".local/bin").join(program)))
            .collect()
    };
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::desktop::discover_desktop_entries;
    use crate::sources::ListOptions;
    use crate::testing::{FakeRunner, FixtureTree};

    fn run(t: &FixtureTree, owned: &[&str], claimed: &[&str]) -> SourceListing {
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let runner = FakeRunner::new();
        let opts = ListOptions::default();
        let cx = SourceContext {
            env: &env,
            runner: &runner,
            desktop: &desktop,
            opts: &opts,
            config: &Config::default(),
        };
        let owned = owned.iter().map(PathBuf::from).collect();
        let claimed = claimed.iter().map(|s| s.to_string()).collect();
        list_unmanaged(&cx, &owned, &claimed)
    }

    fn entry(exec: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=Foo App\nExec={exec}\n")
    }

    #[test]
    fn unmanaged_opt_app_listed() {
        let t = FixtureTree::new();
        t.write_exec("/opt/foo/foo", b"");
        t.desktop(
            "/home/user/.local/share/applications/foo.desktop",
            &entry("/opt/foo/foo %U"),
        );
        let l = run(&t, &[], &[]);
        assert_eq!(l.apps.len(), 1);
        let a = &l.apps[0];
        assert_eq!(a.id, "unmanaged:foo.desktop");
        assert_eq!(a.name, "foo");
        assert_eq!(a.display_name, "Foo App");
        assert_eq!(a.location.as_deref(), Some(Path::new("/opt/foo")));
        assert_eq!(a.installation, Installation::User);
        assert_eq!(a.desktop_ids, ["foo.desktop"]);
        assert!(a.has_gui);
    }

    #[test]
    fn owned_program_not_unmanaged() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/foo", b"");
        t.desktop("/usr/share/applications/foo.desktop", &entry("foo"));
        assert!(run(&t, &["/usr/bin/foo"], &[]).apps.is_empty());
        let listed = run(&t, &[], &[]);
        assert_eq!(listed.apps.len(), 1);
        assert_eq!(listed.apps[0].installation, Installation::System);
        assert_eq!(
            listed.apps[0].location.as_deref(),
            Some(Path::new("/usr/bin/foo"))
        );
    }

    #[test]
    fn symlinked_program_owned_via_target() {
        let t = FixtureTree::new();
        t.write_exec("/usr/lib/foo/foo", b"");
        t.symlink("/usr/bin/foo", "../lib/foo/foo");
        t.desktop(
            "/usr/share/applications/foo.desktop",
            &entry("/usr/bin/foo"),
        );
        assert!(run(&t, &["/usr/lib/foo/foo"], &[]).apps.is_empty());
    }

    #[test]
    fn claimed_and_flatpak_entries_excluded() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/foo", b"");
        t.write_exec("/usr/bin/flatpak", b"");
        t.desktop("/usr/share/applications/foo.desktop", &entry("foo"));
        t.desktop(
            "/usr/share/applications/fp.desktop",
            &entry("flatpak run org.x.Y"),
        );
        t.desktop(
            "/usr/share/applications/hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=H\nExec=foo\nNoDisplay=true\n",
        );
        t.desktop(
            "/usr/share/applications/missing.desktop",
            &entry("/opt/nothing/here"),
        );
        t.desktop(
            "/usr/share/applications/link.desktop",
            "[Desktop Entry]\nType=Link\nName=L\nURL=x\n",
        );
        assert!(run(&t, &[], &["foo.desktop"]).apps.is_empty());
    }

    #[test]
    fn home_tilde_program_expands() {
        let t = FixtureTree::new();
        t.write_exec("/home/user/tools/bar", b"");
        t.desktop("/usr/share/applications/bar.desktop", &entry("~/tools/bar"));
        let l = run(&t, &[], &[]);
        assert_eq!(l.apps.len(), 1);
        assert_eq!(l.apps[0].installation, Installation::User);
        assert_eq!(
            l.apps[0].location.as_deref(),
            Some(Path::new("/home/user/tools/bar"))
        );
    }

    #[test]
    fn flatpak_exports_excluded() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/flatpak", b"");
        t.desktop(
            "/usr/share/applications/a.desktop",
            &entry("/usr/bin/flatpak run --branch=stable org.x.Y"),
        );
        t.desktop(
            "/usr/share/applications/b.desktop",
            "[Desktop Entry]\nType=Application\nName=B\nExec=/opt/b/b\nX-Flatpak=org.x.Y\n",
        );
        t.write_exec("/opt/b/b", b"");
        assert!(run(&t, &[], &[]).apps.is_empty());
    }

    #[test]
    fn appimage_program_not_unmanaged() {
        let t = FixtureTree::new();
        t.appimage("/home/user/Applications/Foo-1.0-x86_64.AppImage");
        t.symlink(
            "/home/user/.local/share/foo/appimage",
            "../../../Applications/Foo-1.0-x86_64.AppImage",
        );
        t.desktop(
            "/home/user/.local/share/applications/foo.desktop",
            &entry("~/.local/share/foo/appimage"),
        );
        assert!(run(&t, &[], &[]).apps.is_empty());
    }

    #[test]
    fn unreadable_program_not_unmanaged() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return; // root can read everything
        }
        let t = FixtureTree::new();
        let p = t.write_exec("/opt/foo/foo", b"");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        t.desktop(
            "/home/user/.local/share/applications/foo.desktop",
            &entry("/opt/foo/foo"),
        );
        assert!(run(&t, &[], &[]).apps.is_empty());
    }
}
