//! Removing Debian/Ubuntu packages with apt: simulated as the user with
//! `apt-get -s purge`, then the exact simulated list is purged as root.

use std::fs;

use crate::command::{CmdOutput, CommandRunner, resolve_trusted};
use crate::env::Env;
use crate::model::{App, SourceKind};
use crate::plan::Action;
use crate::sources::dpkg::{STATUS, parse_status};

use super::{RemovalError, RemovalPlan, invalid};

pub const MSG_APT_BUSY: &str = "apt is busy (another package manager is running)";

/// A Debian package name, optionally arch-qualified (`libfoo:i386`).
fn valid_apt_name(name: &str) -> bool {
    let (pkg, arch) = name.split_once(':').unwrap_or((name, "x"));
    let ok_part = |s: &str, first_alnum: bool| {
        !s.is_empty()
            && (!first_alnum || s.as_bytes()[0].is_ascii_alphanumeric())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'_' | b'-'))
    };
    ok_part(pkg, true) && ok_part(arch, true)
}

/// apt's own error lines (`E: …`) from both streams.
fn error_lines(out: &CmdOutput) -> Vec<String> {
    out.stderr
        .lines()
        .chain(out.stdout.lines())
        .map(str::trim)
        .filter(|l| l.starts_with("E:"))
        .map(String::from)
        .collect()
}

/// `(name, version)` of every `Purg`/`Remv` line of an apt simulation, or a
/// refusal when the simulation would also install or configure something,
/// or would remove an essential package.
pub(crate) fn parse_simulation(out: &CmdOutput) -> Result<Vec<(String, String)>, RemovalError> {
    let text = format!("{}\n{}", out.stdout, out.stderr);
    if text.contains("Could not get lock")
        || text.contains("Unable to acquire the dpkg frontend lock")
    {
        return Err(RemovalError::Busy(MSG_APT_BUSY.into()));
    }
    if out.status != 0 {
        let lines = error_lines(out);
        return Err(RemovalError::Refused(if lines.is_empty() {
            format!("apt-get refused the removal (status {})", out.status)
        } else {
            lines.join("\n")
        }));
    }
    if text.contains("essential packages will be removed") {
        return Err(RemovalError::Refused(
            "apt would remove an essential package; refusing".into(),
        ));
    }
    let mut removed = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(op) = parts.next() else { continue };
        match op {
            "Inst" | "Conf" => {
                return Err(RemovalError::Refused(format!(
                    "apt would also install or configure packages ({line}); refusing"
                )));
            }
            "Purg" | "Remv" => {
                let name = parts.next().unwrap_or("");
                if !valid_apt_name(name) {
                    return Err(invalid("package name", name));
                }
                let version = parts
                    .next()
                    .and_then(|v| v.strip_prefix('['))
                    .and_then(|v| v.strip_suffix(']'))
                    .unwrap_or("")
                    .to_string();
                if !removed.iter().any(|(n, _): &(String, String)| n == name) {
                    removed.push((name.to_string(), version));
                }
            }
            _ => {}
        }
    }
    Ok(removed)
}

/// The purge action for an exact list: never `autoremove`, never
/// `--auto-remove`, so apt cannot remove more than the preview listed.
pub(crate) fn purge_action(names: Vec<String>, versions: Vec<String>) -> Action {
    let mut args = vec![
        "purge".to_string(),
        "-y".to_string(),
        "--no-install-recommends".to_string(),
    ];
    args.extend(names.iter().cloned());
    Action::PackageRemove {
        source: SourceKind::Dpkg,
        names,
        versions,
        program: "apt-get".into(),
        args,
    }
}

pub(super) fn plan(
    env: &Env,
    runner: &dyn CommandRunner,
    app: &App,
) -> Result<RemovalPlan, RemovalError> {
    if !valid_apt_name(&app.name) {
        return Err(invalid("package name", &app.name));
    }
    let program = resolve_trusted(env, "apt-get")
        .ok_or_else(|| RemovalError::Unsupported("apt-get is not installed".into()))?;
    let out = runner
        .run(&program, &["-s", "purge", &app.name])
        .map_err(|e| RemovalError::Refused(e.to_string()))?;
    let simulated = parse_simulation(&out)?;
    let Some(own) = simulated.iter().position(|(n, _)| *n == app.name) else {
        return Err(RemovalError::Refused(format!(
            "apt would not remove {}",
            app.name
        )));
    };
    let mut names = vec![simulated[own].0.clone()];
    let mut versions = vec![simulated[own].1.clone()];
    let mut also_removed = Vec::new();
    for (k, (n, v)) in simulated.iter().enumerate() {
        if k != own {
            names.push(n.clone());
            versions.push(v.clone());
            also_removed.push((n.clone(), v.clone()));
        }
    }
    Ok(RemovalPlan {
        actions: vec![purge_action(names, versions)],
        also_removed,
        notes: Vec::new(),
    })
}

/// Re-reads the dpkg status file: the entries of `names` still present in
/// any state but not-installed. A purge leaves no config files, so the `rc`
/// state (`deinstall ok config-files`) still counts. An unreadable status
/// file is an error.
pub fn dpkg_still_installed(env: &Env, names: &[String]) -> Result<Vec<String>, String> {
    let path = env.sys(STATUS);
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {STATUS}: {e}"))?;
    let present: Vec<String> = parse_status(&String::from_utf8_lossy(&bytes))
        .into_iter()
        .filter(|s| !s.status.ends_with("not-installed"))
        .map(|s| s.name)
        .collect();
    Ok(names
        .iter()
        .filter(|n| {
            let bare = n.split(':').next().unwrap_or(n);
            present.iter().any(|p| p == bare)
        })
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CmdOutput, SystemRunner};
    use crate::model::{App, SourceKind};
    use crate::plan::Action;
    use crate::testing::{FakeRunner, FixtureTree};

    const PURGE_SL: &str = include_str!("../../tests/fixtures/apt/purge-sl.txt");
    const PURGE_PERL_BASE: &str = include_str!("../../tests/fixtures/apt/purge-perl-base.txt");
    const PURGE_NOSUCH: &str = include_str!("../../tests/fixtures/apt/purge-nosuch.txt");

    fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            status,
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    fn debian() -> FixtureTree {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/apt-get", b"#!/bin/sh\n");
        t.write("/var/lib/dpkg/status", b"");
        t
    }

    fn plan_with(name: &str, o: CmdOutput) -> Result<RemovalPlan, RemovalError> {
        let t = debian();
        let r = FakeRunner::new().on("apt-get", &["-s", "purge", name], o);
        plan(&t.env(), &r, &App::new(SourceKind::Dpkg, name))
    }

    /// The perl-base capture without its essential-package warning: a plain
    /// removal that takes dependents with it.
    fn without_essential_warning(text: &str) -> String {
        let mut keep = Vec::new();
        let mut skip = false;
        for l in text.lines() {
            if l.starts_with("WARNING: The following essential") {
                skip = true;
                continue;
            }
            if skip && (l.starts_with(' ') || l.starts_with("This should NOT")) {
                continue;
            }
            skip = false;
            keep.push(l);
        }
        keep.join("\n")
    }

    #[test]
    fn apt_simulation_lists_purged_and_dependents() {
        let text = without_essential_warning(PURGE_PERL_BASE);
        let p = plan_with("perl-base", out(0, &text, "")).unwrap();
        let Action::PackageRemove {
            names, versions, ..
        } = &p.actions[0]
        else {
            panic!("{:?}", p.actions);
        };
        assert_eq!(names[0], "perl-base");
        assert_eq!(versions[0], "5.40.1-6+deb13u1");
        assert!(names.contains(&"cowsay".to_string()));
        assert_eq!(names.len(), 6);
        assert_eq!(p.also_removed.len(), 5);
        assert!(
            p.also_removed
                .contains(&("cowsay".into(), "3.03+dfsg2-8".into()))
        );
    }

    #[test]
    fn apt_exact_list_never_autoremoves() {
        let p = plan_with("sl", out(0, PURGE_SL, "")).unwrap();
        assert_eq!(
            p.actions,
            [Action::PackageRemove {
                source: SourceKind::Dpkg,
                names: vec!["sl".into()],
                versions: vec!["5.02-1+b1".into()],
                program: "apt-get".into(),
                args: vec![
                    "purge".into(),
                    "-y".into(),
                    "--no-install-recommends".into(),
                    "sl".into(),
                ],
            }]
        );
        assert!(p.also_removed.is_empty());
    }

    #[test]
    fn apt_essential_removal_refuses() {
        let e = plan_with("perl-base", out(0, PURGE_PERL_BASE, "")).unwrap_err();
        let RemovalError::Refused(text) = e else {
            panic!("{e:?}");
        };
        assert!(text.contains("essential"), "{text}");
    }

    #[test]
    fn apt_simulation_with_install_refuses() {
        let text = format!("{PURGE_SL}\nInst libfoo [1.0] (1.0 Debian:13)\n");
        assert!(matches!(
            plan_with("sl", out(0, &text, "")),
            Err(RemovalError::Refused(_))
        ));
    }

    #[test]
    fn apt_error_lines_become_refusal() {
        let e = plan_with("nosuchpkg", out(100, "", PURGE_NOSUCH)).unwrap_err();
        assert_eq!(
            e,
            RemovalError::Refused("E: Unable to locate package nosuchpkg".into())
        );
    }

    #[test]
    fn apt_lock_message_is_busy() {
        let stderr = "E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 42 (apt-get)\nE: Unable to acquire the dpkg frontend lock (/var/lib/dpkg/lock-frontend), is another process using it?\n";
        assert_eq!(
            plan_with("sl", out(100, "", stderr)),
            Err(RemovalError::Busy(MSG_APT_BUSY.into()))
        );
    }

    #[test]
    fn apt_simulation_is_run_with_c_locale() {
        let t = FixtureTree::new();
        t.write("/var/lib/dpkg/status", b"");
        t.write_exec(
            "/usr/bin/apt-get",
            b"#!/bin/sh\nif [ \"$LC_ALL\" = C ]; then echo 'Purg sl [1.0]'; else echo 'Entf sl [1.0]'; fi\n",
        );
        let p = plan(
            &t.env(),
            &SystemRunner::new(),
            &App::new(SourceKind::Dpkg, "sl"),
        )
        .unwrap();
        let Action::PackageRemove { versions, .. } = &p.actions[0] else {
            panic!();
        };
        assert_eq!(versions, &["1.0"]);
    }

    #[test]
    fn apt_post_check_rc_state_counts_as_still_installed() {
        let t = FixtureTree::new();
        t.write(
            "/var/lib/dpkg/status",
            b"Package: gone\nStatus: not-installed\n\nPackage: rc\nStatus: deinstall ok config-files\n\nPackage: kept\nStatus: install ok installed\n\n",
        );
        let names: Vec<String> = ["gone", "rc", "kept", "absent"].map(String::from).into();
        assert_eq!(
            dpkg_still_installed(&t.env(), &names).unwrap(),
            ["rc", "kept"]
        );
    }

    #[test]
    fn apt_name_validated() {
        assert!(matches!(
            plan_with("-o", out(0, "", "")),
            Err(RemovalError::InvalidName(_))
        ));
    }
}
