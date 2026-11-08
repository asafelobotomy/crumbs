//! What an uninstall will do, decided before anything is touched.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::bloat::{BloatItem, ItemAction};
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

pub const NOTE_TRASH_FREES_LATER: &str =
    "space is freed when the trash is emptied (or use --permanent)";
pub const NOTE_PERMANENT_TEMP: &str = "permanently delete (temporary files)";
pub const NOTE_PERMANENT_TRASH: &str = "permanently delete (emptying the trash)";

/// Builds a plan for `clean` from scanned items. Trash items are trashed
/// (deleted with `--permanent`), delete items are always deleted, command
/// items become `UserCommand` and privileged items `PrivilegedCommand`. A `UserTemp` or `HomeTrash` item may only be
/// deleted, never trashed. Extra paths (a trashed entry's `.trashinfo`) are
/// deleted right after their item and depend on it.
pub fn build_clean_plan(
    _env: &Env,
    items: &[BloatItem],
    opts: &PlanOptions,
) -> Result<Plan, CoreError> {
    let mut actions: Vec<PlannedAction> = Vec::new();
    for item in items {
        let reason = item.note.clone().unwrap_or_else(|| item.label.clone());
        let bad = |message: &str| CoreError::Parse {
            what: format!("clean item {}", item.label),
            message: message.into(),
        };
        if let ItemAction::Command { program, args } = &item.action {
            actions.push(PlannedAction {
                action: Action::UserCommand {
                    program: program.clone(),
                    args: args.clone(),
                },
                size: item.size.unwrap_or(0),
                fingerprint: Fingerprint {
                    dev: 0,
                    ino: 0,
                    is_symlink: false,
                    ctime: 0,
                    ctime_nsec: 0,
                },
                scope: item.scope,
                depends_on: None,
                reason,
            });
            continue;
        }
        if let ItemAction::Privileged { program, args } = &item.action {
            // Run as root in the run's one batch; the executor resolves the
            // bare program name from the trusted directories.
            actions.push(PlannedAction {
                action: Action::PrivilegedCommand {
                    program: program.clone(),
                    args: args.clone(),
                },
                size: item.size.unwrap_or(0),
                fingerprint: Fingerprint {
                    dev: 0,
                    ino: 0,
                    is_symlink: false,
                    ctime: 0,
                    ctime_nsec: 0,
                },
                scope: item.scope,
                depends_on: None,
                reason,
            });
            continue;
        }
        let path = item.path.clone().ok_or_else(|| bad("item has no path"))?;
        let fingerprint = item
            .fingerprint
            .ok_or_else(|| bad("item has no fingerprint"))?;
        let action = match item.action {
            ItemAction::Trash if item.scope != Scope::Home => {
                return Err(bad("items outside the home can only be deleted"));
            }
            ItemAction::Trash if !opts.permanent => Action::Trash { path },
            _ => Action::Delete { path },
        };
        let main = actions.len();
        actions.push(PlannedAction {
            action,
            size: item.size.unwrap_or(0),
            fingerprint,
            scope: item.scope,
            depends_on: None,
            reason: reason.clone(),
        });
        for (extra, fp) in &item.extra_paths {
            actions.push(PlannedAction {
                action: Action::Delete {
                    path: extra.clone(),
                },
                size: 0,
                fingerprint: *fp,
                scope: item.scope,
                depends_on: Some(main),
                reason: reason.clone(),
            });
        }
    }
    let mut notes = Vec::new();
    if actions
        .iter()
        .any(|a| matches!(a.action, Action::Trash { .. }))
    {
        notes.push(NOTE_TRASH_FREES_LATER.into());
    }
    if actions.iter().any(|a| a.scope == Scope::UserTemp) {
        notes.push(NOTE_PERMANENT_TEMP.into());
    }
    if actions.iter().any(|a| a.scope == Scope::HomeTrash) {
        notes.push(NOTE_PERMANENT_TRASH.into());
    }
    let total_size = actions.iter().map(|a| a.size).sum();
    Ok(Plan {
        app_id: None,
        kind: Some(crate::history::KIND_CLEAN.into()),
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
                Action::UserCommand { .. } => "command",
                Action::PrivilegedCommand { .. } => "privileged-command",
                Action::PackageRemove { .. } => "package-remove",
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
        assert_eq!(plan.kind.as_deref(), Some("uninstall"));
        assert_eq!(plan.notes, [NOTE_APP_STAYS]);
        assert_eq!(plan.total_size, fs[0].size.bytes + fs[1].size.bytes);
        assert!(plan.total_size > 0);
        assert_eq!(
            plan.actions[0].action.path(),
            Some(Path::new("/home/user/.config/foo"))
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
        assert_eq!(plan.kind.as_deref(), Some("orphans"));
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
            Some(Path::new("/home/user/.config/foo"))
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
        assert_eq!(plan.actions[0].action.path(), Some(loc.as_path()));

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

    fn bitem(
        env: &Env,
        path: &str,
        action: crate::bloat::ItemAction,
        scope: Scope,
    ) -> crate::bloat::BloatItem {
        let p = PathBuf::from(path);
        crate::bloat::BloatItem {
            category: crate::bloat::Category::Cache,
            size: Some(disk_usage(env, &p).bytes),
            fingerprint: Some(safety::fingerprint(env, &p).unwrap()),
            path: Some(p),
            label: format!("label {path}"),
            rebuildable: true,
            preselected: true,
            note: None,
            action,
            scope,
            extra_paths: Vec::new(),
            app_id: None,
        }
    }

    fn command_item() -> crate::bloat::BloatItem {
        crate::bloat::BloatItem {
            category: crate::bloat::Category::FlatpakUnused,
            path: None,
            label: "remove unused runtimes".into(),
            size: None,
            rebuildable: false,
            preselected: true,
            note: Some("decided by flatpak".into()),
            action: crate::bloat::ItemAction::Command {
                program: "flatpak".into(),
                args: vec!["uninstall".into(), "--unused".into()],
            },
            scope: Scope::Home,
            fingerprint: None,
            extra_paths: Vec::new(),
            app_id: None,
        }
    }

    #[test]
    fn clean_plan_maps_actions() {
        use crate::bloat::ItemAction;
        let t = FixtureTree::new();
        t.write("home/user/.cache/foo/a", b"aaaa");
        t.write("home/user/.cache/bar/b", b"bbbb");
        t.write("tmp/old", b"tttt");
        let env = t.env();
        let items = [
            bitem(
                &env,
                "/home/user/.cache/foo",
                ItemAction::Trash,
                Scope::Home,
            ),
            bitem(
                &env,
                "/home/user/.cache/bar",
                ItemAction::Delete,
                Scope::Home,
            ),
            bitem(&env, "/tmp/old", ItemAction::Delete, Scope::UserTemp),
            command_item(),
        ];
        let plan = build_clean_plan(&env, &items, &PlanOptions::default()).unwrap();
        assert_eq!(kinds(&plan), ["trash", "delete", "delete", "command"]);
        assert_eq!(plan.app_id, None);
        assert_eq!(plan.kind.as_deref(), Some("clean"));
        assert_eq!(plan.actions[2].scope, Scope::UserTemp);
        assert_eq!(plan.actions[0].fingerprint, items[0].fingerprint.unwrap());
        assert_eq!(plan.actions[0].reason, "label /home/user/.cache/foo");
        assert_eq!(plan.actions[3].reason, "decided by flatpak");
        assert_eq!(
            plan.actions[3].action,
            Action::UserCommand {
                program: "flatpak".into(),
                args: vec!["uninstall".into(), "--unused".into()],
            }
        );
        assert_eq!(plan.actions[3].size, 0);
        assert_eq!(plan.notes, [NOTE_TRASH_FREES_LATER, NOTE_PERMANENT_TEMP]);
        let json = serde_json::to_value(&plan.actions[3].action).unwrap();
        assert_eq!(json["type"], "user-command");
        assert_eq!(json["program"], "flatpak");
    }

    #[test]
    fn privileged_items_become_privileged_commands() {
        use crate::bloat::ItemAction;
        let mut item = command_item();
        item.category = crate::bloat::Category::Journal;
        item.scope = Scope::System;
        item.action = ItemAction::Privileged {
            program: "journalctl".into(),
            args: vec!["--vacuum-size=500M".into()],
        };
        let t = FixtureTree::new();
        let plan = build_clean_plan(&t.env(), &[item], &PlanOptions::default()).unwrap();
        assert_eq!(kinds(&plan), ["privileged-command"]);
        assert_eq!(
            plan.actions[0].action,
            Action::PrivilegedCommand {
                program: "journalctl".into(),
                args: vec!["--vacuum-size=500M".into()],
            }
        );
        assert_eq!(plan.actions[0].scope, Scope::System);
    }

    #[test]
    fn clean_plan_trash_pairs() {
        use crate::bloat::ItemAction;
        let t = FixtureTree::new();
        t.write("home/user/.local/share/Trash/files/x", b"xxxx");
        t.write("home/user/.local/share/Trash/info/x.trashinfo", b"i");
        let env = t.env();
        let info = PathBuf::from("/home/user/.local/share/Trash/info/x.trashinfo");
        let mut item = bitem(
            &env,
            "/home/user/.local/share/Trash/files/x",
            ItemAction::Delete,
            Scope::HomeTrash,
        );
        item.extra_paths = vec![(info.clone(), safety::fingerprint(&env, &info).unwrap())];
        let plan = build_clean_plan(&env, &[item], &PlanOptions::default()).unwrap();
        assert_eq!(kinds(&plan), ["delete", "delete"]);
        assert_eq!(plan.actions[1].action, Action::Delete { path: info });
        assert_eq!(plan.actions[0].depends_on, None);
        assert_eq!(plan.actions[1].depends_on, Some(0));
        assert!(plan.actions.iter().all(|a| a.scope == Scope::HomeTrash));
        assert_eq!(plan.notes, [NOTE_PERMANENT_TRASH]);
        assert_eq!(plan.total_size, plan.actions[0].size);
    }

    #[test]
    fn permanent_turns_trash_into_delete() {
        use crate::bloat::ItemAction;
        let t = FixtureTree::new();
        t.write("home/user/.cache/foo/a", b"aaaa");
        t.write("tmp/old", b"t");
        let env = t.env();
        let opts = PlanOptions {
            permanent: true,
            ..PlanOptions::default()
        };
        let items = [bitem(
            &env,
            "/home/user/.cache/foo",
            ItemAction::Trash,
            Scope::Home,
        )];
        let plan = build_clean_plan(&env, &items, &opts).unwrap();
        assert_eq!(kinds(&plan), ["delete"]);
        assert!(plan.notes.is_empty());

        // Temp and trash items are never trashed, permanent or not.
        for scope in [Scope::UserTemp, Scope::HomeTrash] {
            let bad = [bitem(&env, "/tmp/old", ItemAction::Trash, scope)];
            for o in [PlanOptions::default(), opts] {
                assert!(
                    matches!(
                        build_clean_plan(&env, &bad, &o),
                        Err(CoreError::Parse { .. })
                    ),
                    "{scope:?}"
                );
            }
        }
    }

    #[test]
    fn action_path_none_for_commands() {
        let cmd = Action::UserCommand {
            program: "flatpak".into(),
            args: vec![],
        };
        assert_eq!(cmd.path(), None);
        let trash = Action::Trash {
            path: PathBuf::from("/home/user/x"),
        };
        assert_eq!(trash.path(), Some(Path::new("/home/user/x")));
    }

    #[test]
    fn privileged_actions_have_no_path_and_kebab_tags() {
        let cmd = Action::PrivilegedCommand {
            program: "flatpak".into(),
            args: vec!["uninstall".into()],
        };
        let pkg = Action::PackageRemove {
            source: SourceKind::Pacman,
            names: vec!["foo".into()],
            versions: vec!["1.0-1".into()],
            program: "pacman".into(),
            args: vec!["-Rns".into(), "--noconfirm".into(), "foo".into()],
        };
        assert_eq!(cmd.path(), None);
        assert_eq!(pkg.path(), None);
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["type"], "privileged-command");
        let v = serde_json::to_value(&pkg).unwrap();
        assert_eq!(v["type"], "package-remove");
        assert_eq!(v["source"], "pacman");
        assert_eq!(v["names"][0], "foo");
    }

    #[test]
    fn scope_serialised_only_for_paths_outside_home() {
        let pa = |action: Action, scope: Scope| PlannedAction {
            action,
            size: 0,
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope,
            depends_on: Some(0),
            reason: "r".into(),
        };
        let cmd = pa(
            Action::PrivilegedCommand {
                program: "flatpak".into(),
                args: Vec::new(),
            },
            Scope::System,
        );
        let v = serde_json::to_value(&cmd).unwrap();
        assert!(v.get("scope").is_none(), "{v}");
        assert_eq!(v["depends_on"], 0);
        assert_eq!(v["reason"], "r");
        let del = pa(
            Action::Delete {
                path: "/var/lib/systemd/coredump/core.1".into(),
            },
            Scope::System,
        );
        let v = serde_json::to_value(&del).unwrap();
        assert_eq!(v["scope"], serde_json::to_value(Scope::System).unwrap());
        let home = pa(trash_action(), Scope::Home);
        assert!(serde_json::to_value(&home).unwrap().get("scope").is_none());
    }

    fn trash_action() -> Action {
        Action::Trash {
            path: "/home/user/x".into(),
        }
    }
}
