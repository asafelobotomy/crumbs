//! The commands a person can run by hand when crumbs could not get
//! administrator rights: one line per privileged action, quoted exactly as
//! the privileged batch quotes them.

use std::ffi::OsStr;
use std::path::PathBuf;

use crumbs_core::Env;
use crumbs_core::command::resolve_trusted;
use crumbs_core::plan::{Action, PlannedAction};
use crumbs_core::privilege::batch::sh_quote;
use crumbs_core::safety::Scope;

use super::render::sanitise;

/// Whether the action runs in the privileged batch (as the executor
/// decides it).
pub fn is_privileged(pa: &PlannedAction) -> bool {
    match pa.action {
        Action::PrivilegedCommand { .. } | Action::PackageRemove { .. } => true,
        Action::Delete { .. } => pa.scope == Scope::System,
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => false,
    }
}

/// `sudo <program> <args>` for each privileged action, in order. Programs
/// are shown at their in-system trusted path (as given when not
/// installed); every word goes through [`sh_quote`]. Lines are sanitised
/// for the terminal.
pub fn manual_commands<'a>(
    env: &Env,
    actions: impl IntoIterator<Item = &'a PlannedAction>,
) -> Vec<String> {
    actions
        .into_iter()
        .filter(|pa| is_privileged(pa))
        .filter_map(|pa| {
            let (program, args): (&str, Vec<&OsStr>) = match &pa.action {
                Action::PrivilegedCommand { program, args }
                | Action::PackageRemove { program, args, .. } => {
                    (program, args.iter().map(OsStr::new).collect())
                }
                Action::Delete { path } => (
                    "rm",
                    vec![OsStr::new("-f"), OsStr::new("--"), path.as_os_str()],
                ),
                _ => return None,
            };
            let program = resolve_trusted(env, program)
                .map_or_else(|| PathBuf::from(program), |p| env.unsys(&p));
            let mut line = String::from("sudo");
            for word in std::iter::once(program.as_os_str()).chain(args) {
                line.push(' ');
                line.push_str(&String::from_utf8_lossy(&sh_quote(word)));
            }
            Some(sanitise(&line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::safety::Fingerprint;
    use crumbs_core::testing::FixtureTree;

    fn planned(action: Action, scope: Scope) -> PlannedAction {
        PlannedAction {
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
            depends_on: None,
            reason: String::new(),
        }
    }

    #[test]
    fn quotes_like_the_batch_and_skips_user_actions() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/flatpak", b"");
        t.write_exec("/usr/bin/rm", b"");
        let actions = [
            planned(
                Action::PrivilegedCommand {
                    program: "flatpak".into(),
                    args: vec!["uninstall".into(), "it's".into()],
                },
                Scope::Home,
            ),
            planned(
                Action::Trash {
                    path: "/home/user/x".into(),
                },
                Scope::Home,
            ),
            planned(
                Action::UserCommand {
                    program: "flatpak".into(),
                    args: Vec::new(),
                },
                Scope::Home,
            ),
            planned(
                Action::Delete {
                    path: "/var/lib/systemd/coredump/core.1".into(),
                },
                Scope::System,
            ),
            planned(
                Action::PackageRemove {
                    source: crumbs_core::SourceKind::Pacman,
                    names: vec!["a".into()],
                    versions: vec!["1".into()],
                    program: "pacman".into(),
                    args: vec!["-Rn".into(), "a".into()],
                },
                Scope::Home,
            ),
        ];
        assert_eq!(
            manual_commands(&t.env(), &actions),
            [
                r"sudo '/usr/bin/flatpak' 'uninstall' 'it'\''s'",
                "sudo '/usr/bin/rm' '-f' '--' '/var/lib/systemd/coredump/core.1'",
                // Not installed: shown as given.
                "sudo 'pacman' '-Rn' 'a'",
            ]
        );
    }
}
