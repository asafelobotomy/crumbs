//! Path-based removal of an unmanaged app's own files.

use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::env::{Env, display_path};
use crate::inventory::Inventory;
use crate::leftovers::candidates::NameKey;
use crate::leftovers::system::app_name_sets;
use crate::model::App;
use crate::ownership::tmpfiles_declared;
use crate::safety::{self, Fingerprint, Scope};
use crate::size::{DiskUsage, disk_usage};

use super::RemovalError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmanagedTarget {
    pub path: PathBuf,
    pub scope: Scope,
    pub fingerprint: Fingerprint,
    pub size: DiskUsage,
    pub left_link: Option<PathBuf>,
    /// A symlink to the program in a non-protected home location, with its
    /// fingerprint: removed together with the target.
    pub home_link: Option<(PathBuf, Fingerprint)>,
}

/// Plans the removal of an unmanaged app's own files: the `/opt/<dir>` or
/// home directory its canonical program sits in, or the single program file
/// in `~/.local/bin` or `~/bin`. Only when that target is unambiguous:
/// unowned, used by no other detected app, and named after the app.
/// Everything checked here is checked again by the executor.
pub fn plan_unmanaged(
    env: &Env,
    cfg: &Config,
    inv: &Inventory,
    app: &App,
) -> Result<UnmanagedTarget, RemovalError> {
    let refuse = |reason: String| {
        RemovalError::Unsupported(format!(
            "removing this unmanaged app's files isn't possible: {reason}; use --leftovers-only"
        ))
    };
    let protected = || refuse("its program is in a protected or shared location".into());
    let Some(program) = app.program.as_deref() else {
        return Err(protected());
    };
    let home = env.unsys(&env.home);
    let (path, scope) = if let Ok(rest) = program.strip_prefix("/opt") {
        let Some(first) = rest.components().next() else {
            return Err(protected());
        };
        (Path::new("/opt").join(first), Scope::SystemTree)
    } else if let Some((base, rest)) = ["opt", "Applications"]
        .iter()
        .find_map(|d| program.strip_prefix(home.join(d)).ok().map(|r| (d, r)))
    {
        let mut comps = rest.components();
        match (comps.next(), comps.next()) {
            (Some(first), Some(_)) => (home.join(base).join(first), Scope::Home),
            _ => return Err(protected()),
        }
    } else if ["bin", ".local/bin"]
        .iter()
        .any(|d| program.parent() == Some(home.join(d).as_path()))
    {
        (program.to_path_buf(), Scope::Home)
    } else {
        return Err(protected());
    };
    let shown = display_path(env, &path);

    // (a) Unowned. Without a complete package database "unowned" is a guess.
    if scope == Scope::SystemTree {
        if inv.owned.is_empty() || !inv.owned.is_complete() {
            return Err(refuse(
                "package ownership is unknown (no pacman database)".into(),
            ));
        }
        let declared = tmpfiles_declared(env)
            .map_err(|_| refuse("package ownership is unknown (tmpfiles.d unreadable)".into()))?;
        let declared_below = declared
            .range::<Path, _>((Bound::Included(path.as_path()), Bound::Unbounded))
            .next()
            .is_some_and(|first| first.starts_with(&path));
        if inv.owned.owns_at_or_below(&path) || declared_below {
            return Err(refuse(format!("{shown} belongs to a package")));
        }
    } else if inv.owned.owns_at_or_below(&path) {
        return Err(refuse(format!("{shown} belongs to a package")));
    }

    // (b) No other detected app lives there. One pass over the inventory.
    let mut others: Vec<&str> = inv
        .apps
        .iter()
        .filter(|o| o.id != app.id)
        .filter(|o| {
            [o.program.as_deref(), o.location.as_deref()]
                .into_iter()
                .flatten()
                .any(|p| p.starts_with(&path))
        })
        .map(|o| o.display_name.as_str())
        .collect();
    others.sort_unstable();
    others.dedup();
    if !others.is_empty() {
        return Err(refuse(format!(
            "{shown} is shared with {}",
            others.join(", ")
        )));
    }

    // (c) The directory (or file) is named after the app.
    let leaf = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !matches_app(app, leaf) {
        return Err(refuse(format!("{shown} does not match the app's name")));
    }

    let verified =
        safety::verify_now(env, cfg, scope, &path).map_err(|e| refuse(format!("{e}")))?;
    let fingerprint = safety::fingerprint(env, &path).unwrap_or(verified);
    let mut left_link = None;
    let mut home_link = None;
    if let Some(link) = app.program_link.clone().filter(|l| !l.starts_with(&path)) {
        let in_home = link.starts_with(&home);
        match safety::verify_now(env, cfg, Scope::Home, &link) {
            Ok(fp) if in_home => home_link = Some((link, fp)),
            _ => left_link = Some(link),
        }
    }
    Ok(UnmanagedTarget {
        size: disk_usage(env, &path),
        path,
        scope,
        fingerprint,
        left_link,
        home_link,
    })
}

/// Whether `leaf` is one of the app's names: exact (ignoring case) or by
/// normalised key.
fn matches_app(app: &App, leaf: &str) -> bool {
    let (names, keys) = app_name_sets(app);
    if names.iter().any(|n| n.eq_ignore_ascii_case(leaf)) {
        return true;
    }
    let entry = NameKey::new(leaf);
    keys.iter()
        .any(|k| k.norm == entry.norm || k.norm == entry.unversioned)
}
