use std::io::Write;
use std::process::ExitCode;

use crumbs_core::command::SystemRunner;
use crumbs_core::doctor::{DoctorReport, doctor};
use crumbs_core::privilege::EscalationKind;
use crumbs_core::{Env, SourceError, SourceKind};

use super::context::{load_cfg, print_notes};
use super::render::sanitise;
use crate::output::{ErrorEntry, print_json_entries};

pub fn run(env: &Env, json: bool) -> anyhow::Result<ExitCode> {
    let (cfg, notes) = load_cfg(env)?;
    let report = doctor(env, &SystemRunner::new(), &cfg);
    let code = crate::output::source_exit_code(&report.sources, &report.errors);
    if json {
        let mut errors: Vec<ErrorEntry> = report.errors.iter().map(ErrorEntry::from).collect();
        errors.extend(notes);
        print_json_entries("doctor", &report, &errors)?;
    } else {
        print_human(&report)?;
        let mut err = anstream::stderr().lock();
        for e in &report.errors {
            writeln!(err, "warning: {}: {}", e.source.as_str(), e.message)?;
        }
        drop(err);
        print_notes(&notes)?;
    }
    Ok(ExitCode::from(code))
}

fn print_human(r: &DoctorReport) -> anyhow::Result<()> {
    let mut out = anstream::stdout().lock();
    writeln!(out, "crumbs {}", r.crumbs_version)?;
    writeln!(out, "Distro: {} ({})", r.distro.name, r.distro.id)?;
    writeln!(out, "\nSources:")?;
    for s in &r.sources {
        let name = s.source.as_str();
        let failed = r.errors.iter().any(|e: &SourceError| e.source == s.source);
        if let Some(reason) = &s.skipped_reason {
            if s.source == SourceKind::Unmanaged {
                writeln!(out, "  – {name}  skipped: {reason}")?;
            } else {
                writeln!(out, "  ✗ {name}  {reason}")?;
            }
        } else if failed {
            writeln!(out, "  ✗ {name}  failed")?;
        } else {
            writeln!(out, "  ✓ {name}  {} apps", s.app_count)?;
        }
    }
    match &r.escalation {
        Some(e) => {
            let kind = match e.kind {
                EscalationKind::Run0 => "run0",
                EscalationKind::Pkexec => "pkexec",
                EscalationKind::Sudo => "sudo",
            };
            // The same rule `uninstall` uses: sudo needs a terminal.
            let usable = super::act::usable(e);
            writeln!(
                out,
                "\nEscalation: {kind} ({}{})",
                sanitise(&e.program.display().to_string()),
                if usable { "" } else { ", needs a terminal" }
            )?;
            if usable {
                writeln!(out, "privileged cleanup: available via {kind}")?;
            } else {
                writeln!(out, "privileged cleanup: unavailable")?;
            }
        }
        None => {
            writeln!(out, "\nEscalation: none")?;
            writeln!(out, "privileged cleanup: unavailable")?;
        }
    }
    let trash = &r.trash;
    let state = match (trash.exists, trash.writable) {
        (true, true) => "writable",
        (true, false) => "not writable",
        (false, _) => "missing",
    };
    writeln!(
        out,
        "Trash: {} ({state})",
        sanitise(&trash.path.display().to_string())
    )?;
    Ok(())
}
