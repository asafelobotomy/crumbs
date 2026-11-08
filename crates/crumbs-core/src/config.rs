//! User configuration: `{config_home}/crumbs/config.toml`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::bloat::Category;
use crate::env::Env;
use crate::error::CoreError;

/// Settings from `config.toml`. All paths and patterns are in-system and
/// absolute (a leading `~` is expanded to the in-system home on load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Paths that are never touched.
    pub protect: Vec<PathBuf>,
    /// Glob patterns (see [`glob_match`]) for paths to skip.
    pub exclude: Vec<String>,
    /// Extra directories scanned for AppImages.
    pub appimage_dirs: Vec<PathBuf>,
    /// Minimum age of `/tmp` entries to list (days).
    pub temp_age_days: u64,
    /// Minimum age of log files to list (days).
    pub log_age_days: u64,
    /// Package versions `pkg-cache` keeps (`paccache -k`). Default 2.
    pub pkg_cache_keep: u32,
    /// Journal size above which `journal` offers a vacuum, as a
    /// `journalctl` size string (`^[0-9]+[KMGT]?$`). Default `500M`.
    pub journal_max: String,
    /// Categories skipped when none are named. Defaults to `[DevCache]`.
    pub categories_off: Vec<Category>,
    /// One message per entry dropped on load (never read from the file).
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            protect: Vec::new(),
            exclude: Vec::new(),
            appimage_dirs: Vec::new(),
            temp_age_days: 7,
            log_age_days: 14,
            pkg_cache_keep: 2,
            journal_max: "500M".into(),
            categories_off: vec![Category::DevCache],
            warnings: Vec::new(),
        }
    }
}

/// Raw file form; unknown keys are ignored so newer configs load on older
/// binaries.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    protect: Vec<String>,
    exclude: Vec<String>,
    appimage_dirs: Vec<String>,
    // Signed, so a negative value gets the same warning as zero.
    temp_age_days: Option<i64>,
    log_age_days: Option<i64>,
    pkg_cache_keep: Option<i64>,
    journal_max: Option<String>,
    categories_off: Option<Vec<String>>,
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

/// Resolves `.` and `..` segments and collapses repeated and trailing
/// slashes. A `..` at `/` stays at `/`, as the kernel resolves it.
fn normalise_lexical(abs: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in abs.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    format!("/{}", stack.join("/"))
}

/// Loads `{config_home}/crumbs/config.toml`. A missing file yields the
/// default config. Entries that are not absolute after `~` expansion are
/// dropped, each with a message in `warnings`.
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
    let mut warnings = Vec::new();
    let mut strings = |key: &str, v: &[String]| -> Vec<String> {
        v.iter()
            .filter_map(|s| {
                let Some(expanded) = expand(env, s) else {
                    warnings.push(format!(
                        "config.toml: ignoring {key} entry '{s}' (must start with / or ~/)"
                    ));
                    return None;
                };
                Some(normalise_lexical(&expanded))
            })
            .collect()
    };
    let protect = strings("protect", &raw.protect);
    let exclude = strings("exclude", &raw.exclude);
    let appimage_dirs = strings("appimage_dirs", &raw.appimage_dirs);
    let categories_off = match &raw.categories_off {
        None => vec![Category::DevCache],
        Some(names) => names
            .iter()
            .filter_map(|n| match n.parse::<Category>() {
                Ok(c) => Some(c),
                Err(_) => {
                    warnings.push(format!(
                        "config.toml: unknown category '{n}' in categories_off"
                    ));
                    None
                }
            })
            .collect(),
    };
    let mut at_least_one = |key: &str, v: Option<i64>, default: u64| match v {
        None => default,
        Some(n) => u64::try_from(n)
            .ok()
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| {
                warnings.push(format!("config.toml: {key} must be at least 1; using 1"));
                1
            }),
    };
    let temp_age_days = at_least_one("temp_age_days", raw.temp_age_days, 7);
    let log_age_days = at_least_one("log_age_days", raw.log_age_days, 14);
    let pkg_cache_keep = match raw.pkg_cache_keep {
        None => 2,
        Some(n) => u32::try_from(n).unwrap_or_else(|_| {
            warnings.push("config.toml: pkg_cache_keep must be at least 0; using 2".into());
            2
        }),
    };
    let journal_max = match raw.journal_max {
        None => "500M".to_string(),
        Some(s) if journal_size_bytes(&s).is_some() => s,
        Some(s) => {
            warnings.push(format!(
                "config.toml: ignoring journal_max '{s}' (use digits and an optional K, M, G or T); using 500M"
            ));
            "500M".to_string()
        }
    };
    let paths = |v: Vec<String>| v.into_iter().map(PathBuf::from).collect();
    Ok(Config {
        protect: paths(protect),
        exclude,
        appimage_dirs: paths(appimage_dirs),
        temp_age_days,
        log_age_days,
        pkg_cache_keep,
        journal_max,
        categories_off,
        warnings,
    })
}

/// Bytes of a `journalctl` size string: `^[0-9]+[KMGT]?$`, 1024-based.
pub fn journal_size_bytes(s: &str) -> Option<u64> {
    let (digits, unit) = match s.char_indices().last()? {
        (i, c @ ('K' | 'M' | 'G' | 'T')) => (&s[..i], c),
        _ => (s, 'B'),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let power = match unit {
        'K' => 1,
        'M' => 2,
        'G' => 3,
        'T' => 4,
        _ => 0,
    };
    digits.parse::<u64>().ok()?.checked_mul(1024u64.pow(power))
}

/// Matches an in-system `path` against a glob `pattern`.
///
/// Both are compared as lists of `/`-separated segments. `*` matches any
/// characters within one segment; a segment that is `**` matches zero or
/// more whole segments. A segment that mixes `**` with other text is
/// treated as a bare `**` (it can only shield more, never less). A pattern
/// also matches anything beneath a path it matches (so `/a/b` matches
/// `/a/b/c/d`). Runs in O(pattern segments x path segments).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat = segments(pattern);
    let path = segments(path);
    let mut cur = start_states(&pat);
    if cur[pat.len()] {
        return true;
    }
    for seg in path {
        cur = step(&pat, &cur, seg);
        if cur[pat.len()] {
            return true;
        }
    }
    false
}

/// True if some path strictly beneath `dir` could match `pattern`: `dir`'s
/// segments match a prefix of the pattern, with at least one pattern
/// segment left to consume. A path equal to or beneath a fully matched
/// pattern is [`glob_match`]'s business, not this function's.
pub fn could_match_below(pattern: &str, dir: &str) -> bool {
    let pat = segments(pattern);
    let mut cur = start_states(&pat);
    for seg in segments(dir) {
        cur = step(&pat, &cur, seg);
    }
    cur[..pat.len()].iter().any(|&b| b)
}

fn segments(s: &str) -> Vec<&str> {
    s.split('/').filter(|s| !s.is_empty()).collect()
}

fn is_globstar(seg: &str) -> bool {
    seg.contains("**")
}

/// Adds the states reachable by letting `**` segments match nothing.
fn close(pat: &[&str], states: &mut [bool]) {
    for j in 0..pat.len() {
        if states[j] && is_globstar(pat[j]) {
            states[j + 1] = true;
        }
    }
}

fn start_states(pat: &[&str]) -> Vec<bool> {
    let mut s = vec![false; pat.len() + 1];
    s[0] = true;
    close(pat, &mut s);
    s
}

/// Advances the set of pattern positions over one path segment.
fn step(pat: &[&str], cur: &[bool], seg: &str) -> Vec<bool> {
    let mut next = vec![false; pat.len() + 1];
    for j in 0..pat.len() {
        if !cur[j] {
            continue;
        }
        if is_globstar(pat[j]) {
            next[j] = true;
        } else if segment_match(pat[j].as_bytes(), seg.as_bytes()) {
            next[j + 1] = true;
        }
    }
    close(pat, &mut next);
    next
}

/// Matches one segment: `*` is any run of characters, the rest is literal.
fn segment_match(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if p.get(pi) == Some(&b'*') {
            star = Some((pi, si));
            pi += 1;
        } else if p.get(pi) == Some(&s[si]) {
            pi += 1;
            si += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
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
    fn invalid_entries_warn() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"protect = [\"foo/bar\", \"/ok\"]\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!(c.protect, [PathBuf::from("/ok")]);
        assert_eq!(c.warnings.len(), 1);
        assert!(c.warnings[0].contains("foo/bar"), "{:?}", c.warnings);
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
            ("/home/**/x", "/home/x", true), // `**` may match zero segments
            ("/a/b", "/a/b", true),
            ("/a/b", "/a/b/c/d", true),
            ("/a/b", "/a/bc", false),
            ("/a/b", "/a", false),
            ("/a/b/", "/a/b", true),
            ("/a/b/", "/a/b/c", true),
            ("/a/b.c", "/a/bxc", false),
        ];
        for (pat, path, want) in cases {
            assert_eq!(glob_match(pat, path), want, "{pat} vs {path}");
        }
    }

    #[test]
    fn glob_double_star_matches_dir_itself() {
        assert!(glob_match("/a/.cache/**", "/a/.cache"));
        assert!(glob_match("/a/**/z", "/a/z"));
        assert!(glob_match("/a/**/z", "/a/b/c/z"));
        assert!(!glob_match("/a/**/z", "/a/b/c"));
    }

    #[test]
    fn glob_many_double_stars_is_fast() {
        let path = format!("/{}", vec!["seg"; 40].join("/"));
        let start = std::time::Instant::now();
        assert!(!glob_match("/**/**/**/**/**/**/x", &path));
        assert!(glob_match("/**/**/**/**/**/**/seg", &path));
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn could_match_below_table() {
        let star = "/h/.config/*/keep";
        let cases = [
            (star, "/h/.config/Foobar", true),
            (star, "/h/.config", true),
            (star, "/h/.cache/x", false),
            ("/h/a/**/z", "/h/a/b/c", true),
            ("/h/a/b", "/h/a", true),
            ("/h/a/b", "/h/a/b", false),
            ("/h/a/b", "/h/x", false),
            ("/h/a/**", "/h/a", true),
        ];
        for (pat, dir, want) in cases {
            assert_eq!(could_match_below(pat, dir), want, "{pat} below {dir}");
        }
    }

    #[test]
    fn config_normalises_dotdot() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"protect = [\"/a/b/../c\", \"/a/./d\", \"/../x\"]\nexclude = [\"~/.cache/../.config/*/keep\", \"/a/../../y\"]\nappimage_dirs = [\"/opt/./ai/\"]\n",
        );
        let c = load_config(&t.env()).unwrap();
        // `..` at `/` stays at `/`, as the kernel resolves it: the entry is
        // kept, never dropped.
        assert_eq!(
            c.protect,
            [
                PathBuf::from("/a/c"),
                PathBuf::from("/a/d"),
                PathBuf::from("/x")
            ]
        );
        assert_eq!(c.exclude, ["/home/user/.config/*/keep", "/y"]);
        assert_eq!(c.appimage_dirs, [PathBuf::from("/opt/ai")]);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn config_age_days_at_least_one() {
        let t = FixtureTree::new();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"temp_age_days = 0\nlog_age_days = -3\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.temp_age_days, c.log_age_days), (1, 1));
        assert_eq!(
            c.warnings,
            [
                "config.toml: temp_age_days must be at least 1; using 1",
                "config.toml: log_age_days must be at least 1; using 1"
            ]
        );
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"temp_age_days = 1\nlog_age_days = 2\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.temp_age_days, c.log_age_days), (1, 2));
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn config_bloat_keys_and_unknown_category_warns() {
        let t = FixtureTree::new();
        let d = load_config(&t.env()).unwrap();
        assert_eq!((d.temp_age_days, d.log_age_days), (7, 14));
        assert_eq!(d.categories_off, [Category::DevCache]);

        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"temp_age_days = 3\nlog_age_days = 30\ncategories_off = [\"logs\", \"bogus\", \"temp\"]\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.temp_age_days, c.log_age_days), (3, 30));
        assert_eq!(c.categories_off, [Category::Logs, Category::Temp]);
        assert_eq!(
            c.warnings,
            ["config.toml: unknown category 'bogus' in categories_off"]
        );

        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"categories_off = []\n",
        );
        let c = load_config(&t.env()).unwrap();
        assert!(c.categories_off.is_empty());
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn system_keys_defaults_and_invalid_values_warn() {
        let t = FixtureTree::new();
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (2, "500M"));

        let write = |body: &str| t.write("/home/user/.config/crumbs/config.toml", body.as_bytes());
        write("pkg_cache_keep = 0\njournal_max = \"2G\"\n");
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (0, "2G"));
        assert!(c.warnings.is_empty());

        write("pkg_cache_keep = -1\njournal_max = \"1.5G\"\n");
        let c = load_config(&t.env()).unwrap();
        assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (2, "500M"));
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        assert!(c.warnings[0].contains("pkg_cache_keep"));
        assert!(c.warnings[1].contains("journal_max"));
    }
}
