use super::*;
use crate::privilege::EscalationKind;
use crate::testing::{FakePrivilegedRunner, FixtureTree};
use std::os::unix::ffi::OsStringExt;

fn esc() -> Escalation {
    Escalation {
        kind: EscalationKind::Run0,
        program: PathBuf::from("/usr/bin/run0"),
        requires_tty: false,
    }
}

fn step(program: &Path, args: &[&[u8]]) -> Step {
    Step {
        program: program.to_path_buf(),
        args: args
            .iter()
            .map(|a| OsString::from_vec(a.to_vec()))
            .collect(),
    }
}

fn run(steps: &[Step]) -> Vec<StepOutcome> {
    let script = build_script(steps).unwrap();
    let stdout = FakePrivilegedRunner::default()
        .run(&esc(), &script)
        .unwrap();
    parse_markers(&stdout, steps.len())
}

fn outcome(status: Option<i32>, output: &str) -> StepOutcome {
    StepOutcome {
        status,
        output: output.into(),
    }
}

mod escalation_terminal;
mod sh_script;
