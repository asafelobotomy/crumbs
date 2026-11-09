use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::error::CoreError;

#[derive(Debug, Clone, Serialize)]
pub struct Distro {
    pub id: String,
    pub id_like: Vec<String>,
    pub name: String,
}

impl Distro {
    fn unknown() -> Self {
        Distro {
            id: "unknown".into(),
            id_like: Vec::new(),
            name: "Unknown".into(),
        }
    }

    fn parse(text: &str) -> Self {
        let mut d = Distro::unknown();
        let mut name = None;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            match k.trim() {
                "ID" => d.id = v.to_string(),
                "ID_LIKE" => d.id_like = v.split_whitespace().map(String::from).collect(),
                "NAME" => name = Some(v.to_string()),
                _ => {}
            }
        }
        if let Some(n) = name {
            d.name = n;
        }
        d
    }
}

#[derive(Debug, Clone)]
pub struct Env {
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub cache_home: PathBuf,
    pub data_home: PathBuf,
    pub state_home: PathBuf,
    pub data_dirs: Vec<PathBuf>,
    pub uid: u32,
    pub distro: Distro,
    /// Added to the wall clock by [`Env::now`]. Always zero on the real
    /// system; a fake root reads it from `CRUMBS_TEST_CLOCK_SHIFT_DAYS`, so
    /// tests can make files look old by their ctime, which cannot be set.
    pub(crate) clock_shift: Duration,
    /// Who must own in-system `/tmp` and every directory on the way to a
    /// [`Scope::System`](crate::safety::Scope::System) target before anything
    /// there is touched: root (0) on the real system. A fake root's `/tmp` is made by the
    /// test user, so [`Env::fake`] expects that user instead. The sticky
    /// bit is required either way.
    pub(crate) system_owner_uid: u32,
    /// Test override for the user's mount namespace (see
    /// [`Env::user_mount_ns`]): `Some(None)` reads as unreadable. Always
    /// `None` outside tests, where the real `/proc/self/ns/mnt` is read.
    pub(crate) mount_ns: Option<Option<OsString>>,
    /// `None` for the real system (`statvfs` answers); a fake root answers
    /// with this instead: `CRUMBS_TEST_FS_SPACE` (`<available>/<total>`
    /// bytes), or nothing.
    pub(crate) fs_space: Option<Option<(u64, u64)>>,
}

/// Days from `CRUMBS_TEST_CLOCK_SHIFT_DAYS`; only ever read for a fake root.
/// `(available, total)` bytes from `CRUMBS_TEST_FS_SPACE`; only ever read
/// for a fake root.
fn test_fs_space() -> Option<(u64, u64)> {
    let v = std::env::var("CRUMBS_TEST_FS_SPACE").ok()?;
    let (avail, total) = v.split_once('/')?;
    Some((avail.parse().ok()?, total.parse().ok()?))
}

fn test_clock_shift() -> Duration {
    std::env::var("CRUMBS_TEST_CLOCK_SHIFT_DAYS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::ZERO, |d| {
            Duration::from_secs(d.saturating_mul(86_400))
        })
}

fn abs_var(name: &str) -> Option<PathBuf> {
    let v = std::env::var_os(name)?;
    let p = PathBuf::from(v);
    p.is_absolute().then_some(p)
}

fn load_distro(root: &Path) -> Distro {
    let path = root.join("etc/os-release");
    fs::read_to_string(path)
        .map(|t| Distro::parse(&t))
        .unwrap_or_else(|_| Distro::unknown())
}

/// The owner of `home`, or of its nearest existing ancestor when it does
/// not exist yet (a home that does not exist belongs to no one else).
pub(crate) fn home_owner(home: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    home.ancestors()
        .find_map(|p| fs::symlink_metadata(p).ok())
        .map(|m| m.uid())
}

impl Env {
    pub fn from_system() -> Result<Env, CoreError> {
        let root = PathBuf::from("/");
        let home = abs_var("HOME").ok_or_else(|| CoreError::Parse {
            what: "environment".into(),
            message: "HOME is unset or not absolute".into(),
        })?;
        let config_home = abs_var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let cache_home = abs_var("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
        let data_home = abs_var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        let state_home = abs_var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state"));

        let raw = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let mut data_dirs: Vec<PathBuf> = raw
            .split(':')
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .collect();
        for extra in [
            data_home.join("flatpak/exports/share"),
            PathBuf::from("/var/lib/flatpak/exports/share"),
        ] {
            if !data_dirs.contains(&extra) {
                data_dirs.push(extra);
            }
        }

        let distro = load_distro(&root);
        Ok(Env {
            root,
            home,
            config_home,
            cache_home,
            data_home,
            state_home,
            data_dirs,
            uid: rustix::process::getuid().as_raw(),
            distro,
            clock_shift: Duration::ZERO,
            system_owner_uid: 0,
            mount_ns: None,
            fs_space: None,
        })
    }

    pub fn fake(root: &Path) -> Result<Env, CoreError> {
        let unsafe_root = || CoreError::UnsafeRoot(root.to_path_buf());
        let tmp = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| unsafe_root())?;
        let canon = check_fake_root(root, &tmp)?;
        let home = canon.join("home/user");
        let distro = load_distro(&canon);
        let uid = rustix::process::getuid().as_raw();
        Ok(Env {
            config_home: home.join(".config"),
            cache_home: home.join(".cache"),
            data_home: home.join(".local/share"),
            state_home: home.join(".local/state"),
            data_dirs: vec![canon.join("usr/local/share"), canon.join("usr/share")],
            uid,
            system_owner_uid: uid,
            home,
            root: canon,
            distro,
            clock_shift: test_clock_shift(),
            mount_ns: None,
            fs_space: Some(test_fs_space()),
        })
    }

    /// When crumbs runs as root with a home directory that root does not
    /// own (`sudo -E`, `su -m`): that home, in-system. Acting there as root
    /// would leave root-owned history, trash entries and edits in another
    /// user's home, so callers refuse.
    pub fn foreign_home_as_root(&self) -> Option<PathBuf> {
        if self.uid != 0 {
            return None;
        }
        (home_owner(&self.home) != Some(0)).then(|| self.unsys(&self.home))
    }

    /// Test hook: sets [`Env::clock_shift`].
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn set_clock_shift(&mut self, shift: Duration) {
        self.clock_shift = shift;
    }

    /// Test hook: what [`Env::fs_space`] answers.
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn set_fs_space(&mut self, space: Option<(u64, u64)>) {
        self.fs_space = Some(space);
    }

    /// `(available, total)` bytes of the filesystem holding `in_system`, as
    /// an unprivileged user may use it; `None` when unknown.
    pub fn fs_space(&self, in_system: &Path) -> Option<(u64, u64)> {
        if let Some(fixed) = self.fs_space {
            return fixed;
        }
        let st = rustix::fs::statvfs(self.sys_path(in_system)).ok()?;
        let unit = if st.f_frsize > 0 {
            st.f_frsize
        } else {
            st.f_bsize
        };
        Some((
            st.f_bavail.saturating_mul(unit),
            st.f_blocks.saturating_mul(unit),
        ))
    }

    /// Test hook: sets [`Env::system_owner_uid`].
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn set_system_owner_uid(&mut self, uid: u32) {
        self.system_owner_uid = uid;
    }

    /// Test hook: sets [`Env::mount_ns`]; `None` makes it unreadable.
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn set_mount_ns(&mut self, ns: Option<OsString>) {
        self.mount_ns = Some(ns);
    }

    /// The mount namespace this process runs in: the target of the real
    /// `/proc/self/ns/mnt` (never the fake root's), e.g. `mnt:[4026531841]`.
    /// `None` when it cannot be read.
    pub fn user_mount_ns(&self) -> Option<OsString> {
        if let Some(ns) = &self.mount_ns {
            return ns.clone();
        }
        fs::read_link("/proc/self/ns/mnt")
            .ok()
            .map(PathBuf::into_os_string)
            .filter(|ns| !ns.is_empty())
    }

    /// The current time, shifted by [`Env::clock_shift`] (zero on the real
    /// system).
    pub fn now(&self) -> SystemTime {
        SystemTime::now() + self.clock_shift
    }

    /// Maps an in-system absolute path onto the root.
    pub fn sys(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }

    /// Like `sys` but takes a `Path` and never converts through UTF-8, so
    /// non-UTF-8 names survive.
    pub fn sys_path(&self, in_system: &Path) -> PathBuf {
        match in_system.strip_prefix("/") {
            Ok(rest) => self.root.join(rest),
            Err(_) => self.root.join(in_system),
        }
    }

    /// Inverse of `sys`: the absolute in-system path, for display and JSON.
    pub fn unsys(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.root) {
            Ok(rest) => Path::new("/").join(rest),
            Err(_) => p.to_path_buf(),
        }
    }
}

/// Canonicalises `root` and requires it to be strictly inside `temp`, where
/// `temp` itself must be a real directory (not `/`, at least 2 components).
pub(crate) fn check_fake_root(root: &Path, temp: &Path) -> Result<PathBuf, CoreError> {
    let unsafe_root = || CoreError::UnsafeRoot(root.to_path_buf());
    let canon = root.canonicalize().map_err(|_| unsafe_root())?;
    if temp.components().count() < 2 || canon == temp || !canon.starts_with(temp) {
        return Err(unsafe_root());
    }
    Ok(canon)
}

/// Display form of an in-system path: `~/<rest>` under the in-system home
/// (`~` for the home itself), otherwise the absolute path.
pub fn display_path(env: &Env, in_system: &Path) -> String {
    match in_system.strip_prefix(env.unsys(&env.home)) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => in_system.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn home_owner_falls_back_to_the_nearest_existing_ancestor() {
        use std::os::unix::fs::MetadataExt;
        let t = crate::testing::FixtureTree::new();
        let mut env = t.env();
        env.home = t.env().sys("/home/nobody-yet/sub");
        let parent_uid = std::fs::metadata(t.env().sys("/")).unwrap().uid();
        assert_eq!(home_owner(&env.home), Some(parent_uid));
        env.uid = 0;
        // The nearest existing directory is the test user's (or root's, in
        // a container): foreign exactly when that owner is not root.
        assert_eq!(env.foreign_home_as_root().is_some(), parent_uid != 0);
    }
    use super::*;
    use crate::testing::FixtureTree;

    #[test]
    fn fake_rejects_non_temp_root() {
        assert!(matches!(
            Env::fake(Path::new("/")),
            Err(CoreError::UnsafeRoot(_))
        ));
    }

    #[test]
    fn fake_rejects_root_temp_dir() {
        let t = FixtureTree::new();
        assert!(matches!(
            check_fake_root(t.dir.path(), Path::new("/")),
            Err(CoreError::UnsafeRoot(_))
        ));
    }

    #[test]
    fn sys_joins_under_root() {
        let t = FixtureTree::new();
        let env = t.env();
        let p = env.sys("/var/lib/pacman/local");
        assert_eq!(p, env.root.join("var/lib/pacman/local"));
        assert_eq!(env.unsys(&p), Path::new("/var/lib/pacman/local"));
    }

    #[test]
    fn xdg_defaults_under_home() {
        let t = FixtureTree::new();
        let env = t.env();
        assert_eq!(env.cache_home, env.home.join(".cache"));
        assert_eq!(env.config_home, env.home.join(".config"));
        assert_eq!(env.data_home, env.home.join(".local/share"));
        assert_eq!(env.state_home, env.home.join(".local/state"));
    }

    #[test]
    fn distro_parsed() {
        let t = FixtureTree::new();
        t.os_release("manjaro", "arch", "Manjaro Linux");
        let env = t.env();
        assert_eq!(env.distro.id, "manjaro");
        assert_eq!(env.distro.id_like, ["arch"]);
        assert_eq!(env.distro.name, "Manjaro Linux");
    }

    #[test]
    fn distro_missing_is_unknown() {
        let t = FixtureTree::new();
        assert_eq!(t.env().distro.id, "unknown");
    }

    #[test]
    fn display_path_home_and_absolute() {
        let t = FixtureTree::new();
        let env = t.env();
        assert_eq!(display_path(&env, Path::new("/home/user")), "~");
        assert_eq!(
            display_path(&env, Path::new("/home/user/Applications/X.AppImage")),
            "~/Applications/X.AppImage"
        );
        assert_eq!(
            display_path(&env, Path::new("/opt/x/X.AppImage")),
            "/opt/x/X.AppImage"
        );
        assert_eq!(
            display_path(&env, Path::new("/home/username/X")),
            "/home/username/X"
        );
    }
}
