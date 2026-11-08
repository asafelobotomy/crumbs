use std::io::Write;
use std::process::ExitCode;

use crumbs_core::command::SystemRunner;
use crumbs_core::doctor::{DoctorReport, doctor};
use crumbs_core::privilege::EscalationKind;
use crumbs_core::{Env, SourceError, SourceKind};

use crate::output::print_json;

pub fn run(env: &Env, json: bool) -> anyhow::Result<ExitCode> {
    let report = doctor(env, &SystemRunner);
    let code = if crate::partial_failure_parts(&report.sources, &report.errors) {
        3
    } else {
        0
    };
    if json {
        print_json("doctor", &report, &report.errors)?;
    } else {
        print_human(&report)?;
        let mut err = anstream::stderr().lock();
        for e in &report.errors {
            writeln!(err, "warning: {}: {}", e.source.as_str(), e.message)?;
        }
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
            writeln!(out, "\nEscalation: {kind} ({})", e.program.display())?;
        }
        None => writeln!(out, "\nEscalation: none (privileged cleanup unavailable)")?,
    }
    let trash = &r.trash;
    let state = match (trash.exists, trash.writable) {
        (true, true) => "writable",
        (true, false) => "not writable",
        (false, _) => "missing",
    };
    writeln!(out, "Trash: {} ({state})", trash.path.display())?;
    Ok(())
}
