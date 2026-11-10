//! What an uninstall will do, decided before anything is touched.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::bloat::{BloatItem, ItemAction};
use crate::env::Env;
use crate::error::CoreError;
use crate::leftovers::{Finding, FindingKind};
use crate::model::{App, SourceKind};
use crate::removal::UnmanagedTarget;
use crate::safety::{self, Fingerprint, Scope};
use crate::size::disk_usage;

mod clean;
pub use clean::*;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Trash {
        path: PathBuf,
    },
    Delete {
        path: PathBuf,
    },
    /// Removes the lines exactly equal to one of `remove_lines`.
    EditFile {
        path: PathBuf,
        remove_lines: Vec<String>,
    },
    /// Runs a trusted program (resolved at execution time, never via
    /// `$PATH`) to remove something it manages itself.
    UserCommand {
        program: String,
        args: Vec<String>,
    },
    /// Runs a trusted program as root, in the run's single privileged batch.
    PrivilegedCommand {
        program: String,
        args: Vec<String>,
    },
    /// Removes packages with the exact command `program args` (run as root
    /// in the batch); `names` and `versions` are recorded in the history.
    PackageRemove {
        source: SourceKind,
        names: Vec<String>,
        versions: Vec<String>,
        program: String,
        args: Vec<String>,
    },
}

impl Action {
    /// The in-system path the action works on; `None` for a command or a
    /// package removal.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Action::Trash { path } | Action::Delete { path } | Action::EditFile { path, .. } => {
                Some(path)
            }
            Action::UserCommand { .. }
            | Action::PrivilegedCommand { .. }
            | Action::PackageRemove { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAction {
    pub action: Action,
    pub size: u64,
    /// Taken at scan time; the executor re-verifies against it. Not part
    /// of the JSON.
    pub fingerprint: Fingerprint,
    /// Where the path may lie; the executor verifies it in this scope.
    /// Left out of the JSON when it is `Home` (as in M2) and for an action
    /// without a path, where it means nothing.
    pub scope: Scope,
    /// Index of an earlier action in the same plan that must have succeeded
    /// first; the executor skips this action otherwise. Left out of the
    /// JSON when `None`.
    pub depends_on: Option<usize>,
    /// `(plan index, action index)` of an action in an earlier plan of the
    /// same run that covered this action's dependency (see the CLI's
    /// `drop_overlaps`); [`execute_many`](crate::executor::execute_many)
    /// skips this action unless that action succeeded. Not part of the JSON.
    pub depends_on_plan: Option<(usize, usize)>,
    /// False when part of the size could not be read and nothing was
    /// counted (a tree with `size.errors > 0` and 0 bytes): the preview
    /// shows `-` and its total `+ unknown`. Not part of the JSON.
    pub size_known: bool,
    pub reason: String,
}

impl Serialize for PlannedAction {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let with_scope = self.action.path().is_some() && !self.scope.is_home();
        let fields = 3 + usize::from(with_scope) + usize::from(self.depends_on.is_some());
        let mut st = s.serialize_struct("PlannedAction", fields)?;
        st.serialize_field("action", &self.action)?;
        st.serialize_field("size", &self.size)?;
        if with_scope {
            st.serialize_field("scope", &self.scope)?;
        }
        if let Some(d) = self.depends_on {
            st.serialize_field("depends_on", &d)?;
        }
        st.serialize_field("reason", &self.reason)?;
        st.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub app_id: Option<String>,
    /// Which command made the plan; recorded as each history entry's
    /// `kind`. Not part of the JSON plan.
    #[serde(skip)]
    pub kind: Option<String>,
    pub actions: Vec<PlannedAction>,
    pub total_size: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanOptions {
    /// `Delete` instead of `Trash`.
    pub permanent: bool,
    /// Keep everything except caches.
    pub keep_data: bool,
    /// The app itself stays: an AppImage file is neither planned nor
    /// looked at.
    pub leftovers_only: bool,
}

pub const NOTE_APP_STAYS: &str = "the app itself stays installed (leftovers only)";
pub const REASON_APPIMAGE: &str = "AppImage file";
pub const REASON_UNMANAGED_LINK: &str = "link to the app's program";
pub const REASON_UNMANAGED: &str = "the app's own files";

/// Whether a size was read: false when part of it could not be read and
/// nothing was counted.
pub fn size_known(size: &crate::size::DiskUsage) -> bool {
    !(size.errors > 0 && size.bytes == 0)
}

/// A root delete of a picked `/etc` or `/opt` leftover: planned with no
/// dependency, whatever else the plan removes (see [`build_plan_unmanaged`]).
pub fn is_system_leftover(pa: &PlannedAction) -> bool {
    pa.scope == Scope::SystemTree && matches!(pa.action, Action::Delete { .. })
}

fn remove(path: PathBuf, opts: &PlanOptions) -> Action {
    if opts.permanent {
        Action::Delete { path }
    } else {
        Action::Trash { path }
    }
}

/// Builds the plan. For an AppImage app the file itself comes first, with
/// its fingerprint taken now; a missing `location` is an error. With
/// `leftovers_only` the app stays and the plan notes so.
pub fn build_plan(
    env: &Env,
    app: Option<&App>,
    findings: &[Finding],
    opts: &PlanOptions,
) -> Result<Plan, CoreError> {
    build_plan_unmanaged(env, app, None, findings, opts)
}

/// Like [`build_plan`], and for an unmanaged app with a `target` (from
/// `plan_unmanaged`) the target's removal comes first, every leftover
/// depending on it except system leftovers (`Scope::SystemTree`), which
/// depend on nothing. A system target is a root delete whatever
/// `--permanent` says. With `leftovers_only`, or no target, the app stays.
pub fn build_plan_unmanaged(
    env: &Env,
    app: Option<&App>,
    target: Option<&UnmanagedTarget>,
    findings: &[Finding],
    opts: &PlanOptions,
) -> Result<Plan, CoreError> {
    let mut actions = Vec::new();
    let mut notes = Vec::new();
    let mut skip_path: Option<PathBuf> = None;
    if let Some(app) = app {
        if let (SourceKind::Unmanaged, Some(t), false) = (app.source, target, opts.leftovers_only) {
            let action = if t.scope == Scope::SystemTree {
                notes.push(NOTE_SYSTEM_DELETE.into());
                Action::Delete {
                    path: t.path.clone(),
                }
            } else {
                remove(t.path.clone(), opts)
            };
            actions.push(PlannedAction {
                action,
                size: t.size.bytes,
                fingerprint: t.fingerprint,
                scope: t.scope,
                depends_on: None,
                depends_on_plan: None,
                size_known: size_known(&t.size),
                reason: REASON_UNMANAGED.into(),
            });
            if let Some((link, fingerprint)) = &t.home_link {
                actions.push(PlannedAction {
                    action: remove(link.clone(), opts),
                    size: 0,
                    fingerprint: *fingerprint,
                    scope: Scope::Home,
                    depends_on: None,
                    depends_on_plan: None,
                    size_known: true,
                    reason: REASON_UNMANAGED_LINK.into(),
                });
            }
            if let Some(link) = &t.left_link {
                notes.push(format!(
                    "{} is left in place{}",
                    crate::env::display_path(env, link),
                    if t.left_link_protected {
                        " (protected path)"
                    } else {
                        ""
                    }
                ));
            }
            skip_path = Some(t.path.clone());
        } else if app.source == SourceKind::AppImage && !opts.leftovers_only {
            let location = app.location.clone().ok_or_else(|| CoreError::Parse {
                what: format!("app {}", app.id),
                message: "AppImage has no location".into(),
            })?;
            let fingerprint = safety::fingerprint(env, &location)?;
            actions.push(PlannedAction {
                size: disk_usage(env, &location).bytes,
                action: remove(location.clone(), opts),
                fingerprint,
                scope: Scope::Home,
                depends_on: None,
                depends_on_plan: None,
                size_known: true,
                reason: REASON_APPIMAGE.into(),
            });
            skip_path = Some(location);
        } else {
            notes.push(NOTE_APP_STAYS.into());
        }
    }
    for f in findings {
        if opts.keep_data && f.kind != FindingKind::Cache {
            continue;
        }
        if skip_path.as_ref().is_some_and(|s| f.path.starts_with(s)) {
            continue;
        }
        if f.scope == Scope::SystemTree {
            // Root deletes are never trashed, whatever `--permanent` says.
            if !notes.iter().any(|n| n == NOTE_SYSTEM_DELETE) {
                notes.push(NOTE_SYSTEM_DELETE.into());
            }
            actions.push(PlannedAction {
                action: Action::Delete {
                    path: f.path.clone(),
                },
                size: f.size.bytes,
                fingerprint: f.fingerprint,
                scope: Scope::SystemTree,
                depends_on: None,
                depends_on_plan: None,
                size_known: size_known(&f.size),
                reason: f.reason.clone(),
            });
            continue;
        }
        let (action, size) = if f.kind == FindingKind::MimeAssoc {
            let edit = Action::EditFile {
                path: f.path.clone(),
                remove_lines: f.mime_lines.clone(),
            };
            // Editing frees nothing.
            (edit, 0)
        } else {
            (remove(f.path.clone(), opts), f.size.bytes)
        };
        actions.push(PlannedAction {
            action,
            size,
            fingerprint: f.fingerprint,
            scope: Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: f.reason.clone(),
        });
    }
    if skip_path.is_some() {
        // A full AppImage or unmanaged uninstall: nothing else goes unless
        // the app's own file or directory did. A system leftover is the
        // exception: it was picked by hand and no package owns it (the same
        // as `--leftovers-only`), and a privileged action cannot wait for
        // another one.
        for a in actions.iter_mut().skip(1) {
            if !is_system_leftover(a) {
                a.depends_on = Some(0);
            }
        }
    }
    let total_size = actions.iter().map(|a| a.size).sum();
    Ok(Plan {
        app_id: app.map(|a| a.id.clone()),
        kind: Some(
            if app.is_some() {
                crate::history::KIND_UNINSTALL
            } else {
                crate::history::KIND_ORPHANS
            }
            .into(),
        ),
        actions,
        total_size,
        notes,
    })
}

#[cfg(test)]
mod tests;
