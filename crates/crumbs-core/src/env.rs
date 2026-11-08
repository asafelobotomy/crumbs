use std::fs;
use std::path::{Path, PathBuf};

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
        })
    }

    pub fn fake(root: &Path) -> Result<Env, CoreError> {
        let unsafe_root = || CoreError::UnsafeRoot(root.to_path_buf());
        let canon = root.canonicalize().map_err(|_| unsafe_root())?;
        let tmp = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| unsafe_root())?;
        if canon == tmp || !canon.starts_with(&tmp) {
            return Err(unsafe_root());
        }
        let home = canon.join("home/user");
        let distro = load_distro(&canon);
        Ok(Env {
            config_home: home.join(".config"),
            cache_home: home.join(".cache"),
            data_home: home.join(".local/share"),
            state_home: home.join(".local/state"),
            data_dirs: vec![canon.join("usr/local/share"), canon.join("usr/share")],
            uid: rustix::process::getuid().as_raw(),
            home,
            root: canon,
            distro,
        })
    }

    /// Maps an in-system absolute path onto the root.
    pub fn sys(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }

    /// Inverse of `sys`: the absolute in-system path, for display and JSON.
    pub fn unsys(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.root) {
            Ok(rest) => Path::new("/").join(rest),
            Err(_) => p.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
