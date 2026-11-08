//! User configuration: `{config_home}/crumbs/config.toml`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::env::Env;
use crate::error::CoreError;

/// Settings from `config.toml`. All paths and patterns are in-system and
/// absolute (a leading `~` is expanded to the in-system home on load).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Paths that are never touched.
    pub protect: Vec<PathBuf>,
    /// Glob patterns (see [`glob_match`]) for paths to skip.
    pub exclude: Vec<String>,
    /// Extra directories scanned for AppImages.
    pub appimage_dirs: Vec<PathBuf>,
}

/// Raw file form; unknown keys are ignored so newer configs load on older
/// binaries.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    protect: Vec<String>,
    exclude: Vec<String>,
    appimage_dirs: Vec<String>,
}

/// Expands a leading `~` against the in-system home. Returns `None` for
/// anything that is not then absolute.
fn expand(env: &Env, s: &str) -> Option<String> {
    let home = env.unsys(&env.home);
    let out = if s == "~" {
        home.to_string_lossy().into_owned()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest).to_string_lossy().into_owned()
    } else {
        s.to_string()
    };
    out.starts_with('/').then_some(out)
}

/// Loads `{config_home}/crumbs/config.toml`. A missing file yields the
/// default config. Relative `protect`/`appimage_dirs`/`exclude` entries are
/// dropped.
pub fn load_config(env: &Env) -> Result<Config, CoreError> {
    let path = env.config_home.join("crumbs/config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(source) => return Err(CoreError::Io { path, source }),
    };
    let raw: RawConfig = toml::from_str(&text).map_err(|e| CoreError::Parse {
        what: "config.toml".into(),
        message: e.to_string(),
    })?;
    let paths = |v: &[String]| -> Vec<PathBuf> {
        v.iter()
            .filter_map(|s| expand(env, s))
            .map(PathBuf::from)
            .collect()
    };
    Ok(Config {
        protect: paths(&raw.protect),
        exclude: raw.exclude.iter().filter_map(|s| expand(env, s)).collect(),
        appimage_dirs: paths(&raw.appimage_dirs),
    })
}

/// Matches an in-system `path` against a glob `pattern`.
///
/// `*` matches any characters except `/`; `**` matches any characters
/// including `/`; everything else is literal. The whole path must match,
/// except that a pattern also matches anything beneath a path it matches
/// (so the literal `/a/b` matches `/a/b/c/d`, split on a `/` boundary).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let (p, s) = (pattern.as_bytes(), path.as_bytes());
    if matches_at(p, s) {
        return true;
    }
    s.iter()
        .enumerate()
        .any(|(i, &b)| b == b'/' && i > 0 && matches_at(p, &s[..i]))
}

fn matches_at(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => {
            if let Some(rest2) = rest.strip_prefix(b"*") {
                (0..=s.len()).any(|i| matches_at(rest2, &s[i..]))
            } else {
                let limit = s.iter().position(|&b| b == b'/').unwrap_or(s.len());
                (0..=limit).any(|i| matches_at(rest, &s[i..]))
            }
        }
        Some((c, rest)) => s
            .split_first()
            .is_some_and(|(d, s2)| c == d && matches_at(rest, s2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    #[test]
    fn missing_config_is_default() {
        let t = FixtureTree::new();
        let c = load_config(&t.env()).unwrap();
        assert!(c.protect.is_empty() && c.exclude.is_empty() && c.appimage_dirs.is_empty());
    }

    #[test]
    fn parses_keys_and_expands_tilde() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"protect = [\"~/Documents\", \"/srv/keep\", \"relative/dir\"]\nexclude = [\"~/.cache/**\", \"/tmp/*.log\"]\nappimage_dirs = [\"~/Tools\", \"/opt/appimages\"]\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!(
            c.protect,
            [
                PathBuf::from("/home/user/Documents"),
                PathBuf::from("/srv/keep")
            ]
        );
        assert_eq!(c.exclude, ["/home/user/.cache/**", "/tmp/*.log"]);
        assert_eq!(
            c.appimage_dirs,
            [
                PathBuf::from("/home/user/Tools"),
                PathBuf::from("/opt/appimages")
            ]
        );
    }

    #[test]
    fn unknown_keys_ignored() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"future_key = 3\nprotect = [\"/a/b\"]\n[section]\nx = 1\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!(c.protect, [PathBuf::from("/a/b")]);
    }

    #[test]
    fn bad_toml_is_parse_error() {
        let t = FixtureTree::new();
        t.write("/home/user/.config/crumbs/config.toml", b"protect = [");
        match load_config(&t.env()) {
            Err(CoreError::Parse { what, .. }) => assert_eq!(what, "config.toml"),
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn glob_match_table() {
        let cases = [
            ("/home/u/.cache/**", "/home/u/.cache/a/b/c", true),
            ("/home/u/.cache/**", "/home/u/.cache/a", true),
            ("/home/u/.cache/**", "/home/u/.config/a", false),
            ("/home/u/*.log", "/home/u/x.log", true),
            ("/home/u/*.log", "/home/u/sub/x.log", false),
            ("/home/*/x", "/home/u/x", true),
            ("/home/*/x", "/home/u/v/x", false),
            ("/home/**/x", "/home/u/v/x", true),
            ("/home/**/x", "/home/x", false),
            ("/a/b", "/a/b", true),
            ("/a/b", "/a/b/c/d", true),
            ("/a/b", "/a/bc", false),
            ("/a/b", "/a", false),
            ("/a/b.c", "/a/bxc", false),
        ];
        for (pat, path, want) in cases {
            assert_eq!(glob_match(pat, path), want, "{pat} vs {path}");
        }
    }
}
