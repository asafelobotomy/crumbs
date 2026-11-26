use super::*;
use crate::command::CmdOutput;
use crate::model::Installation;
use crate::testing::{FakeRunner, FixtureTree};

fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
    }
}

fn sim(runner: FakeRunner, pkg: &str, o: CmdOutput) -> FakeRunner {
    runner.on(
        "pacman",
        &["-Rs", "--print", "--print-format", "%n %v", pkg],
        o,
    )
}

fn pacman_app(name: &str) -> App {
    let mut app = App::new(SourceKind::Pacman, name);
    app.version = Some("0.24.5-1".into());
    app
}

fn with_pacman() -> FixtureTree {
    let t = FixtureTree::new();
    t.write_exec("/usr/bin/pacman", b"");
    t.mkdir("/var/lib/pacman/local");
    t
}

fn flatpak_app(installation: Installation) -> App {
    let mut app = App::new(SourceKind::Flatpak, "org.x.Y");
    app.installation = installation;
    app
}

mod pacman_dangling;
mod sys;
