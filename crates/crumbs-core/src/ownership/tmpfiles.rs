//! Paths that systemd-tmpfiles.d declares, matched with its glob rules.

use super::*;

/// The `tmpfiles.d` directories read, in-system.
pub const TMPFILES_DIRS: [&str; 4] = [
    "/usr/lib/tmpfiles.d",
    "/usr/local/lib/tmpfiles.d",
    "/run/tmpfiles.d",
    "/etc/tmpfiles.d",
];

/// Paths that `tmpfiles.d` declares: runtime directories a package creates
/// without listing them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmpfilesDeclared {
    /// Literal paths.
    paths: BTreeSet<PathBuf>,
    /// Paths with glob characters or `%` specifiers, as components; a
    /// specifier is already `*`.
    patterns: Vec<Vec<String>>,
}

impl TmpfilesDeclared {
    /// True when `p` (an absolute path, in practice a direct child of `/etc`
    /// or `/opt`) may hold something declared: a literal path at or below
    /// it, or a pattern at least as deep as `p` whose first components each
    /// glob-match `p`'s (so it could match `p` or something inside it). A
    /// declared ancestor of `p` does not cover it. Components are compared
    /// whole.
    pub fn covers(&self, p: &Path) -> bool {
        let literal = self
            .paths
            .range::<Path, _>((Bound::Included(p), Bound::Unbounded))
            .next()
            .is_some_and(|first| first.starts_with(p));
        if literal {
            return true;
        }
        let Some(comps) = components(p) else {
            return false;
        };
        self.patterns.iter().any(|pat| {
            pat.len() >= comps.len()
                && pat
                    .iter()
                    .zip(&comps)
                    .all(|(g, c)| glob_component(g.as_bytes(), c))
        })
    }
}

/// The normal components of an absolute path, as bytes; `None` for a
/// relative one.
pub(super) fn components(p: &Path) -> Option<Vec<&[u8]>> {
    use std::os::unix::ffi::OsStrExt;
    if !p.is_absolute() {
        return None;
    }
    Some(
        p.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(n) => Some(n.as_bytes()),
                _ => None,
            })
            .collect(),
    )
}

/// fnmatch-style match of one path component: `*`, `?` and `[...]` (with
/// `!` or `^` negation and `a-z` ranges; a `[` without a closing `]` is
/// literal). Never crosses `/`: both sides are single components.
pub(super) fn glob_component(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|k| glob_component(rest, &s[k..])),
        Some((b'?', rest)) => !s.is_empty() && glob_component(rest, &s[1..]),
        Some((b'[', rest)) => match class(rest) {
            Some((matches, after)) => {
                s.first().is_some_and(|&c| matches(c)) && glob_component(after, &s[1..])
            }
            None => s.first() == Some(&b'[') && glob_component(rest, &s[1..]),
        },
        Some((&c, rest)) => s.first() == Some(&c) && glob_component(rest, &s[1..]),
    }
}

/// A bracket class after its `[`: a matcher and the pattern after the `]`,
/// or `None` when there is no closing `]`.
#[allow(clippy::type_complexity)]
pub(super) fn class(p: &[u8]) -> Option<(Box<dyn Fn(u8) -> bool + '_>, &[u8])> {
    let (negate, body) = match p.first() {
        Some(b'!' | b'^') => (true, &p[1..]),
        _ => (false, p),
    };
    // A `]` right after the opening is a member, not the end.
    let close = 1 + body.get(1..)?.iter().position(|&b| b == b']')?;
    let set = &body[..close];
    let after = &body[close + 1..];
    let matcher = move |c: u8| {
        let mut i = 0;
        let mut hit = false;
        while i < set.len() {
            if i + 2 < set.len() && set[i + 1] == b'-' {
                hit |= set[i] <= c && c <= set[i + 2];
                i += 3;
            } else {
                hit |= set[i] == c;
                i += 1;
            }
        }
        hit != negate
    };
    Some((Box::new(matcher), after))
}

/// Unquotes a tmpfiles.d path field the way systemd does: a field starting
/// with `"` runs to the closing `"` (whitespace included), and backslash
/// escapes (`\\`, `\"`, `\'`, `\a \b \f \n \r \t \v`, `\xHH`, `\NNN` octal)
/// are decoded. Returns the field and the rest of the line, or `Err(raw)`
/// with the raw field text when it cannot be unquoted.
pub(super) fn path_field(line: &str) -> Option<Result<String, String>> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    let mut quoted = false;
    let fail = || {
        Some(Err(line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()))
    };
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                quoted = !quoted;
                i += 1;
            }
            b'\\' => {
                let Some(&e) = bytes.get(i + 1) else {
                    return fail();
                };
                let (v, used) = match e {
                    b'\\' | b'"' | b'\'' => (e, 2),
                    b'a' => (7, 2),
                    b'b' => (8, 2),
                    b'f' => (12, 2),
                    b'n' => (b'\n', 2),
                    b'r' => (b'\r', 2),
                    b't' => (b'\t', 2),
                    b'v' => (11, 2),
                    b'x' => match bytes
                        .get(i + 2..i + 4)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                    {
                        Some(v) => (v, 4),
                        None => return fail(),
                    },
                    b'0'..=b'7' => match bytes
                        .get(i + 1..i + 4)
                        .and_then(|o| std::str::from_utf8(o).ok())
                        .and_then(|o| u8::from_str_radix(o, 8).ok())
                    {
                        Some(v) => (v, 4),
                        None => return fail(),
                    },
                    _ => return fail(),
                };
                out.push(v);
                i += used;
            }
            b' ' | b'\t' if !quoted => break,
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    if quoted {
        return fail();
    }
    match String::from_utf8(out) {
        Ok(s) => Some(Ok(s)),
        Err(_) => fail(),
    }
}

/// Whether `p` has a glob character or a `%` specifier.
pub(super) fn is_pattern(p: &str) -> bool {
    p.contains(['*', '?', '[', '%'])
}

/// A pattern path as components, each `%X` specifier as `*` (`%%` is a
/// literal `%`).
pub(super) fn pattern_components(p: &str) -> Vec<String> {
    p.split('/')
        .filter(|c| !c.is_empty())
        .map(|c| {
            let mut out = String::new();
            let mut chars = c.chars();
            while let Some(ch) = chars.next() {
                if ch == '%' {
                    match chars.next() {
                        Some('%') => out.push('%'),
                        _ => out.push('*'),
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        })
        .collect()
}

/// For a field that cannot be unquoted: its literal directory prefix (up
/// to the last `/` before the first special character) plus one `*`
/// component, so it covers whatever that directory might hold.
pub(super) fn fallback_pattern(raw: &str) -> Option<Vec<String>> {
    let raw = raw.trim_start_matches('"');
    if !raw.starts_with('/') {
        return None;
    }
    let first = raw
        .find(['*', '?', '[', '%', '\\', '"'])
        .unwrap_or(raw.len());
    let dir = raw[..first].rfind('/').map_or("", |slash| &raw[..slash]);
    let mut comps = pattern_components(dir);
    comps.push("*".into());
    Some(comps)
}

/// Reads [`TMPFILES_DIRS`], every `*.conf`. The second field of each line
/// is the path, unquoted as systemd does; a leading `%E` is `/etc`. A path
/// with glob characters or `%` specifiers is a pattern
/// ([`TmpfilesDeclared::covers`]); one that cannot be unquoted covers its
/// literal directory conservatively. A path not starting with `/` (another
/// leading specifier) is skipped. A missing directory is fine; an
/// unreadable one is an error.
pub fn tmpfiles_declared(env: &Env) -> Result<TmpfilesDeclared, CoreError> {
    let io_err = |path: &Path, source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut out = TmpfilesDeclared::default();
    for dir in TMPFILES_DIRS {
        let dir = env.sys(dir);
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&dir, e)),
        };
        for entry in rd {
            let path = entry.map_err(|e| io_err(&dir, e))?.path();
            if path.extension().is_none_or(|x| x != "conf") {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
            for line in text.lines() {
                out.add_line(line);
            }
        }
    }
    Ok(out)
}

impl TmpfilesDeclared {
    /// Adds one `tmpfiles.d` line.
    pub(super) fn add_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        // Skip the type field.
        let Some(rest) = line.split_once([' ', '\t']).map(|(_, r)| r) else {
            return;
        };
        let p = match path_field(rest) {
            None => return,
            Some(Ok(p)) => p,
            Some(Err(raw)) => {
                if let Some(pat) = fallback_pattern(&raw) {
                    self.patterns.push(pat);
                }
                return;
            }
        };
        let p = match p.strip_prefix("%E") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/etc{rest}"),
            _ => p,
        };
        if !p.starts_with('/') {
            return;
        }
        if is_pattern(&p) {
            self.patterns.push(pattern_components(&p));
            return;
        }
        let trimmed = p.trim_end_matches('/');
        if !trimmed.is_empty() {
            self.paths.insert(PathBuf::from(trimmed));
        }
    }
}
