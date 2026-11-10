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
        Action::Delete { .. } => matches!(pa.scope, Scope::System | Scope::SystemTree),
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => false,
    }
}

/// The exact command the privileged batch runs for `pa`, as one line:
/// the program at its in-system trusted path (as given when not
/// installed), every word through [`sh_quote`], sanitised for the
/// terminal. `None` for an action that is not privileged.
pub fn root_command(env: &Env, pa: &PlannedAction) -> Option<String> {
    if !is_privileged(pa) {
        return None;
    }
    let (program, args): (&str, Vec<&OsStr>) = match &pa.action {
        Action::PrivilegedCommand { program, args }
        | Action::PackageRemove { program, args, .. } => {
            (program, args.iter().map(OsStr::new).collect())
        }
        // The batch wraps this in a mount-namespace guard; run by hand,
        // it is the user's own namespace.
        Action::Delete { path } if pa.scope == Scope::SystemTree => (
            "rm",
            vec![
                OsStr::new("-rf"),
                OsStr::new("--one-file-system"),
                OsStr::new("--"),
                path.as_os_str(),
            ],
        ),
        Action::Delete { path } => (
            "rm",
            vec![OsStr::new("-f"), OsStr::new("--"), path.as_os_str()],
        ),
        _ => return None,
    };
    let program =
        resolve_trusted(env, program).map_or_else(|| PathBuf::from(program), |p| env.unsys(&p));
    let words: Vec<String> = std::iter::once(program.as_os_str())
        .chain(args)
        .map(|w| {
            let text = String::from_utf8_lossy(w.as_encoded_bytes());
            if text.chars().any(|c| c != '\t' && c.is_control()) {
                ansi_c_quote(&text)
            } else {
                String::from_utf8_lossy(&sh_quote(w)).into_owned()
            }
        })
        .collect();
    Some(sanitise(&words.join(" ")))
}

/// `$'...'` quoting (bash, zsh, ksh) for a word with control characters,
/// such as a script: one readable line that the shell turns back into the
/// exact word, where `'...'` would carry raw newlines.
fn ansi_c_quote(word: &str) -> String {
    let mut out = String::from("$'");
    for c in word.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `sudo <root command>` for each privileged action, in order.
pub fn manual_commands<'a>(
    env: &Env,
    actions: impl IntoIterator<Item = &'a PlannedAction>,
) -> Vec<String> {
    actions
        .into_iter()
        .filter_map(|pa| root_command(env, pa))
        .map(|line| format!("sudo {line}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::safety::Fingerprint;
    use crumbs_core::testing::FixtureTree;

    #[test]
    fn a_script_argument_is_written_so_bash_reads_it_back() {
        // A multi-line argument (the snapshots step) is shown as $'...':
        // one line, readable, and pasted into bash or zsh it is the script.
        let t = FixtureTree::new();
        let script = "a='x'\nprintf '%s\\n' \"$a\"\t# tab\n";
        let pa = planned(
            Action::PrivilegedCommand {
                program: "sh".into(),
                args: vec![
                    "-c".into(),
                    script.into(),
                    "crumbs-snapshots".into(),
                    "3".into(),
                ],
            },
            Scope::System,
        );
        let line = root_command(&t.env(), &pa).unwrap();
        assert!(!line.contains('\u{FFFD}'), "{line}");
        assert!(line.starts_with("'sh' '-c' $'a=\\'x\\'\\nprintf"), "{line}");
        assert!(line.ends_with(" 'crumbs-snapshots' '3'"), "{line}");
        let Ok(bash) = std::process::Command::new("bash").arg("--version").output() else {
            eprintln!("skipped round trip: no bash");
            return;
        };
        assert!(bash.status.success());
        let quoted = line
            .strip_prefix("'sh' '-c' ")
            .and_then(|l| l.strip_suffix(" 'crumbs-snapshots' '3'"))
            .unwrap();
        let back = std::process::Command::new("bash")
            .args(["-c", &format!("printf %s {quoted}")])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(back.stdout).unwrap(), script);
    }

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
            depends_on_plan: None,
            size_known: true,
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
                Action::Delete {
                    path: "/opt/it's".into(),
                },
                Scope::SystemTree,
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
                // The user runs it themselves: no namespace guard.
                r"sudo '/usr/bin/rm' '-rf' '--one-file-system' '--' '/opt/it'\''s'",
                // Not installed: shown as given.
                "sudo 'pacman' '-Rn' 'a'",
            ]
        );
    }
}
