//! What an uninstall will do, decided before anything is touched.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::env::Env;
use crate::error::CoreError;
use crate::leftovers::{Finding, FindingKind};
use crate::model::{App, SourceKind};
use crate::safety::{self, Fingerprint, Scope};
use crate::size::disk_usage;

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
}

impl Action {
    /// The in-system path the action works on.
    pub fn path(&self) -> &Path {
        match self {
            Action::Trash { path } | Action::Delete { path } | Action::EditFile { path, .. } => {
                path
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedAction {
    pub action: Action,
    pub size: u64,
    /// Taken at scan time; the executor re-verifies against it.
    #[serde(skip)]
    pub fingerprint: Fingerprint,
    /// Where the path may lie; the executor verifies it in this scope.
    /// Left out of the JSON when it is `Home`, as in M2.
    #[serde(default, skip_serializing_if = "Scope::is_home")]
    pub scope: Scope,
    /// Index of an earlier action in the same plan that must have succeeded
    /// first; the executor skips this action otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<usize>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub app_id: Option<String>,
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
    let mut actions = Vec::new();
    let mut notes = Vec::new();
    let mut skip_path: Option<PathBuf> = None;
    if let Some(app) = app {
        if app.source == SourceKind::AppImage && !opts.leftovers_only {
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
        if skip_path.as_ref() == Some(&f.path) {
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
            reason: f.reason.clone(),
        });
    }
    if skip_path.is_some() {
        // A full AppImage uninstall: nothing else goes unless the file did.
        for a in actions.iter_mut().skip(1) {
            a.depends_on = Some(0);
        }
    }
    let total_size = actions.iter().map(|a| a.size).sum();
    Ok(Plan {
        app_id: app.map(|a| a.id.clone()),
        actions,
        total_size,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leftovers::Confidence;
    use crate::testing::FixtureTree;

    /// A finding for an existing in-system path, fingerprinted now.
    pub(crate) fn finding(env: &Env, path: &str, kind: FindingKind) -> Finding {
        let p = PathBuf::from(path);
        Finding {
            size: disk_usage(env, &p),
            fingerprint: safety::fingerprint(env, &p).unwrap(),
            path: p,
            kind,
            confidence: Confidence::High,
            shared: false,
            reason: format!("reason for {path}"),
            preselected: true,
            mime_lines: Vec::new(),
        }
    }

    fn kinds(plan: &Plan) -> Vec<&'static str> {
        plan.actions
            .iter()
            .map(|a| match a.action {
                Action::Trash { .. } => "trash",
                Action::Delete { .. } => "delete",
                Action::EditFile { .. } => "edit",
            })
            .collect()
    }

    #[test]
    fn plan_trash_default_delete_permanent() {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/a", b"aaaa");
        t.write("home/user/.cache/foo/b", b"bbbb");
        let env = t.env();
        let fs = [
            finding(&env, "/home/user/.config/foo", FindingKind::Config),
            finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
        ];
        let app = App::new(SourceKind::Pacman, "foo");
        let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
        assert_eq!(kinds(&plan), ["trash", "trash"]);
        assert_eq!(plan.app_id.as_deref(), Some("pacman:foo"));
        assert_eq!(plan.notes, [NOTE_APP_STAYS]);
        assert_eq!(plan.total_size, fs[0].size.bytes + fs[1].size.bytes);
        assert!(plan.total_size > 0);
        assert_eq!(
            plan.actions[0].action.path(),
            Path::new("/home/user/.config/foo")
        );
        assert_eq!(plan.actions[0].fingerprint, fs[0].fingerprint);
        assert_eq!(plan.actions[0].reason, fs[0].reason);

        let opts = PlanOptions {
            permanent: true,
            keep_data: false,
            ..PlanOptions::default()
        };
        let plan = build_plan(&env, None, &fs, &opts).unwrap();
        assert_eq!(kinds(&plan), ["delete", "delete"]);
        assert_eq!(plan.app_id, None);
        assert!(plan.notes.is_empty());

        let json = serde_json::to_value(&plan.actions[0]).unwrap();
        assert_eq!(json["action"]["type"], "delete");
        assert_eq!(json["action"]["path"], "/home/user/.config/foo");
        assert!(json.get("fingerprint").is_none());
    }

    #[test]
    fn plan_keep_data_only_cache() {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/a", b"a");
        t.write("home/user/.cache/foo/b", b"b");
        t.write("home/user/.local/share/foo/c", b"c");
        t.write("home/user/.config/mimeapps.list", b"x=foo.desktop\n");
        let env = t.env();
        let mut mime = finding(
            &env,
            "/home/user/.config/mimeapps.list",
            FindingKind::MimeAssoc,
        );
        mime.mime_lines = vec!["x=foo.desktop".into()];
        let fs = [
            finding(&env, "/home/user/.config/foo", FindingKind::Config),
            finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
            finding(&env, "/home/user/.local/share/foo", FindingKind::Data),
            mime,
        ];
        let opts = PlanOptions {
            permanent: false,
            keep_data: true,
            ..PlanOptions::default()
        };
        let plan = build_plan(&env, None, &fs, &opts).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(
            plan.actions[0].action,
            Action::Trash {
                path: "/home/user/.cache/foo".into()
            }
        );
        assert_eq!(plan.total_size, fs[1].size.bytes);
    }

    #[test]
    fn plan_leftovers_only_skips_appimage_file() {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/a", b"a");
        let env = t.env();
        // No location: a full plan fails, a leftovers-only plan never looks.
        let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
        app.location = None;
        let fs = [finding(&env, "/home/user/.config/foo", FindingKind::Config)];
        let opts = PlanOptions {
            leftovers_only: true,
            ..PlanOptions::default()
        };
        let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
        assert_eq!(kinds(&plan), ["trash"]);
        assert_eq!(
            plan.actions[0].action.path(),
            Path::new("/home/user/.config/foo")
        );
        assert_eq!(plan.notes, [NOTE_APP_STAYS]);
        assert_eq!(plan.app_id.as_deref(), Some("appimage:Foo.AppImage"));
    }

    #[test]
    fn plan_mime_is_edit() {
        let t = FixtureTree::new();
        t.write(
            "home/user/.config/mimeapps.list",
            b"a=foo.desktop\nb=bar.desktop\n",
        );
        let env = t.env();
        let mut mime = finding(
            &env,
            "/home/user/.config/mimeapps.list",
            FindingKind::MimeAssoc,
        );
        mime.mime_lines = vec!["a=foo.desktop".into()];
        for permanent in [false, true] {
            let opts = PlanOptions {
                permanent,
                keep_data: false,
                leftovers_only: false,
            };
            let plan = build_plan(&env, None, std::slice::from_ref(&mime), &opts).unwrap();
            assert_eq!(
                plan.actions[0].action,
                Action::EditFile {
                    path: "/home/user/.config/mimeapps.list".into(),
                    remove_lines: vec!["a=foo.desktop".into()],
                }
            );
            assert_eq!(plan.actions[0].size, 0);
            assert_eq!(plan.total_size, 0);
        }
        let plan = build_plan(&env, None, &[mime], &PlanOptions::default()).unwrap();
        let json = serde_json::to_value(&plan.actions[0].action).unwrap();
        assert_eq!(json["type"], "edit-file");
        assert_eq!(json["remove_lines"][0], "a=foo.desktop");
    }

    #[test]
    fn plan_appimage_includes_file_first() {
        let t = FixtureTree::new();
        t.appimage("home/user/Applications/Foo.AppImage");
        t.write("home/user/.config/foo/a", b"a");
        let env = t.env();
        let loc = PathBuf::from("/home/user/Applications/Foo.AppImage");
        let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
        app.id = "appimage:~/Applications/Foo.AppImage".into();
        app.location = Some(loc.clone());
        let fs = [finding(&env, "/home/user/.config/foo", FindingKind::Config)];
        let opts = PlanOptions {
            permanent: false,
            keep_data: true,
            ..PlanOptions::default()
        };
        let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
        assert_eq!(plan.actions.len(), 1, "keep_data drops the config");
        assert_eq!(plan.actions[0].action, Action::Trash { path: loc.clone() });
        assert_eq!(plan.actions[0].reason, REASON_APPIMAGE);
        assert_eq!(
            plan.actions[0].fingerprint,
            safety::fingerprint(&env, &loc).unwrap()
        );
        assert!(plan.actions[0].size > 0);
        assert!(plan.notes.is_empty());

        let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
        assert_eq!(kinds(&plan), ["trash", "trash"]);
        assert_eq!(plan.actions[0].action.path(), loc);

        app.location = None;
        assert!(build_plan(&env, Some(&app), &fs, &PlanOptions::default()).is_err());
    }

    #[test]
    fn appimage_leftovers_depend_on_the_file() {
        let t = FixtureTree::new();
        t.appimage("home/user/Applications/Foo.AppImage");
        t.write("home/user/.config/foo/a", b"a");
        t.write("home/user/.cache/foo/b", b"b");
        let env = t.env();
        let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
        app.location = Some("/home/user/Applications/Foo.AppImage".into());
        let fs = [
            finding(&env, "/home/user/.config/foo", FindingKind::Config),
            finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
        ];
        let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
        let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
        assert_eq!(deps, [None, Some(0), Some(0)]);
        let json = serde_json::to_value(&plan.actions[0]).unwrap();
        assert!(json.get("depends_on").is_none());
        let json = serde_json::to_value(&plan.actions[1]).unwrap();
        assert_eq!(json["depends_on"], 0);

        // Leftovers only: the app stays, so nothing depends on anything.
        let opts = PlanOptions {
            leftovers_only: true,
            ..PlanOptions::default()
        };
        let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
        assert!(plan.actions.iter().all(|a| a.depends_on.is_none()));
    }
}
