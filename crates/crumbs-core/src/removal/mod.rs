//! Decides how an app itself is removed, before anything is touched. The
//! checks here run as the user and change nothing.

mod apt;
mod flatpak;
mod pacman;
mod snap;
mod unmanaged;

use crate::command::CommandRunner;
use crate::env::Env;
use crate::model::{App, SourceKind};
use crate::plan::Action;

pub use flatpak::flatpak_still_installed;
pub use pacman::pacman_still_installed;
pub use unmanaged::{UnmanagedTarget, plan_unmanaged};

/// The actions that remove the app, plus what the user should be told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalPlan {
    pub actions: Vec<Action>,
    /// `(name, version)` of every package removed besides the app's own.
    pub also_removed: Vec<(String, String)>,
    pub notes: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemovalError {
    /// Another package-manager run holds the database lock.
    #[error("{0}")]
    Busy(String),
    /// The package manager's simulation refused; the text is its error.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    InvalidName(String),
}

pub const MSG_BUSY: &str = "pacman is busy (database lock present)";
pub const MSG_UNMANAGED: &str = "removing unmanaged apps isn't supported yet; use --leftovers-only";

/// Whether `name` is safe to hand to a package manager as one argument:
/// `^[A-Za-z0-9@._+-]+$`, and not an option (no leading `-`).
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'+' | b'-'))
}

/// What pacman said when a `-Rs` simulation failed: the summary on stderr
/// plus the `:: ... breaks dependency ...` reason lines from stdout.
pub(crate) fn refusal_text(out: &crate::command::CmdOutput) -> String {
    let mut lines: Vec<&str> = Some(out.stderr.trim())
        .filter(|l| !l.is_empty())
        .into_iter()
        .collect();
    lines.extend(
        out.stdout
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("::")),
    );
    if lines.is_empty() {
        format!("pacman refused the removal (status {})", out.status)
    } else {
        lines.join("\n")
    }
}

pub(crate) fn invalid(what: &str, name: &str) -> RemovalError {
    RemovalError::InvalidName(format!(
        "refusing {what} {name:?}: not a plain package name"
    ))
}

/// Plans the removal of the app itself. An AppImage is handled by the
/// M2 file plan, so it yields no actions here.
pub fn plan_removal(
    env: &Env,
    runner: &dyn CommandRunner,
    app: &App,
) -> Result<RemovalPlan, RemovalError> {
    match app.source {
        SourceKind::Pacman => pacman::plan(env, runner, app),
        SourceKind::Rpm => Err(RemovalError::Unsupported(
            "removing rpm packages isn't supported yet; use --leftovers-only".into(),
        )),
        SourceKind::Dpkg => apt::plan(env, runner, app),
        SourceKind::Flatpak => flatpak::plan(app),
        SourceKind::Snap => snap::plan(app),
        SourceKind::AppImage => Ok(RemovalPlan::default()),
        SourceKind::Unmanaged => Err(RemovalError::Unsupported(MSG_UNMANAGED.into())),
    }
}

/// Which of `names` are still installed for `source`, in `names` order.
/// Pacman re-reads the local database; Flatpak asks `flatpak list` (used
/// for system and custom installations). Other sources have none. Anything
/// that stops the answer from being trusted (unreadable database, missing
/// or failing program) is an `Err`, never an empty list.
pub fn still_installed(
    env: &Env,
    runner: &dyn CommandRunner,
    source: SourceKind,
    names: &[String],
) -> Result<Vec<String>, String> {
    match source {
        SourceKind::Pacman => pacman_still_installed(env, names),
        SourceKind::Dpkg => apt::dpkg_still_installed(env, names),
        SourceKind::Rpm => Err("checking rpm packages isn't supported yet".into()),
        SourceKind::Flatpak => flatpak_still_installed(env, runner, names),
        SourceKind::Snap => snap::snap_still_installed(env, runner, names),
        SourceKind::AppImage | SourceKind::Unmanaged => Ok(Vec::new()),
    }
}

pub(crate) fn lock_path(env: &Env) -> std::path::PathBuf {
    env.sys("/var/lib/pacman/db.lck")
}

#[cfg(test)]
mod tests;
