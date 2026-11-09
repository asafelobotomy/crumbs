//! Removing Fedora packages with dnf (dnf5, and dnf4's table format):
//! simulated as the user with `dnf remove --assumeno`, then the exact
//! simulated list is removed as root with dependency clean-up turned off.

use crate::command::{CmdOutput, CommandRunner, resolve_trusted};
use crate::env::Env;
use crate::model::{App, SourceKind};
use crate::plan::Action;

use super::{RemovalError, RemovalPlan, invalid, valid_name};

pub const MSG_UNREADABLE: &str = "could not read dnf's plan";

/// The table sections that list packages to remove, in dnf4 and dnf5.
const SECTIONS: [&str; 3] = [
    "Removing:",
    "Removing dependent packages:",
    "Removing unused dependencies:",
];

/// `epoch:version-release` with a zero epoch left out.
fn version(v: &str) -> String {
    v.strip_prefix("0:").unwrap_or(v).to_string()
}

/// `(name, version)` of every package in the removal sections of a dnf
/// transaction table. dnf exits non-zero after `--assumeno` by design, so
/// the status is not used; the text decides. An unknown layout refuses.
pub(crate) fn parse_transaction(out: &CmdOutput) -> Result<Vec<(String, String)>, RemovalError> {
    let text = format!("{}\n{}", out.stdout, out.stderr);
    if let Some(at) = text.find("Failed to resolve the transaction") {
        let problems: Vec<&str> = text[at..]
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        return Err(RemovalError::Refused(if problems.is_empty() {
            "dnf could not resolve the removal".into()
        } else {
            problems.join("\n")
        }));
    }
    if let Some(line) = text.lines().find(|l| {
        l.starts_with("No packages to remove for argument")
            || l.starts_with("No match for argument")
    }) {
        return Err(RemovalError::Refused(line.trim().to_string()));
    }
    let mut removed: Vec<(String, String)> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if SECTIONS.contains(&trimmed) {
            in_section = true;
            seen_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() || !line.starts_with(' ') {
            in_section = false;
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 5 {
            return Err(RemovalError::Refused(MSG_UNREADABLE.into()));
        }
        let name = cols[0];
        if !valid_name(name) {
            return Err(invalid("package name", name));
        }
        if !removed.iter().any(|(n, _)| n == name) {
            removed.push((name.to_string(), version(cols[2])));
        }
    }
    if !seen_section || removed.is_empty() {
        return Err(RemovalError::Refused(MSG_UNREADABLE.into()));
    }
    Ok(removed)
}

/// The remove action for an exact list. `clean_requirements_on_remove` is
/// off, so dnf can never remove more than the preview listed.
pub(crate) fn remove_action(names: Vec<String>, versions: Vec<String>) -> Action {
    let mut args = vec![
        "remove".to_string(),
        "-y".to_string(),
        "--setopt=clean_requirements_on_remove=False".to_string(),
    ];
    args.extend(names.iter().cloned());
    Action::PackageRemove {
        source: SourceKind::Rpm,
        names,
        versions,
        program: "dnf".into(),
        args,
    }
}

pub(super) fn plan(
    env: &Env,
    runner: &dyn CommandRunner,
    app: &App,
) -> Result<RemovalPlan, RemovalError> {
    if !valid_name(&app.name) {
        return Err(invalid("package name", &app.name));
    }
    let program = resolve_trusted(env, "dnf")
        .ok_or_else(|| RemovalError::Unsupported("dnf is not installed".into()))?;
    let out = runner
        .run(&program, &["remove", "--assumeno", &app.name])
        .map_err(|e| RemovalError::Refused(e.to_string()))?;
    let simulated = parse_transaction(&out)?;
    let Some(own) = simulated.iter().position(|(n, _)| *n == app.name) else {
        return Err(RemovalError::Refused(format!(
            "dnf would not remove {}",
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
        actions: vec![remove_action(names, versions)],
        also_removed,
        notes: Vec::new(),
    })
}

/// The entries of `names` that `rpm -q` still finds. A missing `rpm` is an
/// error.
pub fn rpm_still_installed(
    env: &Env,
    runner: &dyn CommandRunner,
    names: &[String],
) -> Result<Vec<String>, String> {
    let rpm = resolve_trusted(env, "rpm").ok_or("rpm is not installed")?;
    let mut still = Vec::new();
    for n in names {
        let out = runner
            .run(&rpm, &["-q", "--qf", "%{NAME}\n", n.as_str()])
            .map_err(|e| e.to_string())?;
        if out.status == 0 {
            still.push(n.clone());
        }
    }
    Ok(still)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::model::{App, SourceKind};
    use crate::plan::Action;
    use crate::testing::{FakeRunner, FixtureTree};

    const SL: &str = include_str!("../../tests/fixtures/dnf/remove-sl.txt");
    const PERL_LIBS: &str = include_str!("../../tests/fixtures/dnf/remove-perl-libs.txt");
    const PROTECTED: &str = include_str!("../../tests/fixtures/dnf/remove-dnf5.txt");
    const NOSUCH: &str = include_str!("../../tests/fixtures/dnf/remove-nosuch.txt");
    const DNF4_SL: &str = include_str!("../../tests/fixtures/dnf/dnf4-remove-sl.txt");

    fn out(status: i32, stdout: &str) -> CmdOutput {
        CmdOutput {
            status,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn plan_with(name: &str, o: CmdOutput) -> Result<RemovalPlan, RemovalError> {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/dnf", b"#!/bin/sh\n");
        let r = FakeRunner::new().on("dnf", &["remove", "--assumeno", name], o);
        plan(&t.env(), &r, &App::new(SourceKind::Rpm, name))
    }

    fn names(p: &RemovalPlan) -> (Vec<String>, Vec<String>, Vec<String>) {
        let Action::PackageRemove {
            names,
            versions,
            args,
            ..
        } = &p.actions[0]
        else {
            panic!("{:?}", p.actions);
        };
        (names.clone(), versions.clone(), args.clone())
    }

    #[test]
    fn dnf5_table_parsed() {
        let p = plan_with("perl-libs", out(1, PERL_LIBS)).unwrap();
        let (n, v, _) = names(&p);
        assert_eq!(n[0], "perl-libs");
        assert_eq!(v[0], "4:5.42.3-525.fc44");
        assert!(n.contains(&"cowsay".to_string()));
        assert!(n.contains(&"groff-base".to_string()));
        assert!(
            p.also_removed
                .contains(&("perl-Carp".into(), "1.54-521.fc44".into()))
        );
        assert_eq!(n.len(), 1 + 6 + 29);
    }

    #[test]
    fn dnf4_table_parsed() {
        let p = plan_with("sl", out(1, DNF4_SL)).unwrap();
        let (n, v, _) = names(&p);
        assert_eq!(n, ["sl", "sl-extra", "ncurses"]);
        assert_eq!(v, ["5.02-25.fc40", "1:1.0-1.fc40", "6.4-12.20240127.fc40"]);
    }

    #[test]
    fn dnf_exact_list_disables_clean_requirements() {
        let p = plan_with("sl", out(1, SL)).unwrap();
        let (n, v, args) = names(&p);
        assert_eq!(n, ["sl"]);
        assert_eq!(v, ["5.02-25.fc44"]);
        assert_eq!(
            args,
            [
                "remove",
                "-y",
                "--setopt=clean_requirements_on_remove=False",
                "sl"
            ]
        );
    }

    #[test]
    fn dnf_unknown_table_refuses() {
        let e = plan_with("sl", out(1, "Something new:\n sl x86_64 1-1 repo 1 KiB\n")).unwrap_err();
        assert_eq!(e, RemovalError::Refused(MSG_UNREADABLE.into()));
    }

    #[test]
    fn dnf_protected_package_refuses() {
        let e = plan_with("dnf5", out(1, PROTECTED)).unwrap_err();
        let RemovalError::Refused(text) = e else {
            panic!("{e:?}")
        };
        assert!(text.contains("protected packages: dnf5"), "{text}");
    }

    #[test]
    fn dnf_not_installed_refuses() {
        let e = plan_with("nosuchpkg", out(0, NOSUCH)).unwrap_err();
        assert!(matches!(e, RemovalError::Refused(t) if t.contains("nosuchpkg")));
    }

    #[test]
    fn dnf_nonzero_assumeno_is_expected() {
        assert!(plan_with("sl", out(1, SL)).is_ok());
    }

    #[test]
    fn rpm_post_check() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/rpm", b"#!/bin/sh\n");
        let r = FakeRunner::new()
            .on(
                "rpm",
                &["-q", "--qf", "%{NAME}\n", "gone"],
                out(1, "package gone is not installed\n"),
            )
            .on(
                "rpm",
                &["-q", "--qf", "%{NAME}\n", "kept"],
                out(0, "kept\n"),
            );
        let names = vec!["gone".to_string(), "kept".to_string()];
        assert_eq!(rpm_still_installed(&t.env(), &r, &names).unwrap(), ["kept"]);
    }
}
