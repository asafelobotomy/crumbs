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
    /// Categories run when none are named, whatever `categories_off` says;
    /// the only way to run an opt-in-only category ([`Category::opt_in_only`],
    /// `build-output`) unnamed.
    pub categories_on: Vec<Category>,
    /// Folders `build-output` looks in for git repositories; empty means
    /// the usual ones below home (`~/Projects`, `~/src`, `~/code`, ...).
    pub project_dirs: Vec<PathBuf>,
    /// Days a build folder must have gone unused to be preselected.
    pub build_age_days: u64,
    /// Newest system snapshots `snapshots` keeps. Default 3.
    pub snapshots_keep: u32,
    /// One message per entry dropped on load (never read from the file).
    pub warnings: Vec<String>,
}

impl Config {
    /// Whether `scan` and `clean` run the category when none are named:
    /// listed in `categories_on`, or not opt-in-only and not in
    /// `categories_off`.
    pub fn enabled(&self, c: Category) -> bool {
        self.categories_on.contains(&c) || (!c.opt_in_only() && !self.categories_off.contains(&c))
    }
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
            categories_on: Vec::new(),
            project_dirs: Vec::new(),
            build_age_days: 7,
            snapshots_keep: 3,
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
    categories_on: Vec<String>,
    project_dirs: Vec<String>,
    build_age_days: Option<i64>,
    snapshots_keep: Option<i64>,
}

/// Expands a leading `~` against the in-system home. Returns `None` for
/// anything that is not then absolute.
/// An `exclude` pattern given outside the config (`clean --exclude`, MCP
/// `exclude`), read exactly as the config key is: `~/` expanded, `..`
/// resolved. `None` unless it starts with `/` or `~/`.
pub fn exclude_pattern(env: &Env, s: &str) -> Option<String> {
    expand(env, s).map(|e| normalise_lexical(&e))
}

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
    let project_dirs = strings("project_dirs", &raw.project_dirs);
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
    let categories_on = raw
        .categories_on
        .iter()
        .filter_map(|n| match n.parse::<Category>() {
            Ok(c) => Some(c),
            Err(_) => {
                warnings.push(format!(
                    "config.toml: unknown category '{n}' in categories_on"
                ));
                None
            }
        })
        .collect();
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
    let build_age_days = at_least_one("build_age_days", raw.build_age_days, 7);
    // Beyond u32: keep everything, never wrap round to a small count.
    let snapshots_keep =
        u32::try_from(at_least_one("snapshots_keep", raw.snapshots_keep, 3)).unwrap_or(u32::MAX);
    let pkg_cache_keep = match raw.pkg_cache_keep {
        None => 2,
        Some(n) => u32::try_from(n).unwrap_or_else(|_| {
            warnings.push("config.toml: pkg_cache_keep must be at least 0; using 2".into());
            2
        }),
    };
    let journal_max = match raw.journal_max {
        None => "500M".to_string(),
        Some(s) if journal_size_bytes(&s).is_some_and(|b| b > 0) => s,
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
        categories_on,
        project_dirs: paths(project_dirs),
        build_age_days,
        snapshots_keep,
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
mod tests;
