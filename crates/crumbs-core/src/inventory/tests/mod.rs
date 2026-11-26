use super::*;
use crate::command::CmdOutput;
use crate::testing::{FakeRunner, FixtureTree};

const FLATPAK_ARGS: [&str; 3] = [
    "list",
    "--app",
    "--columns=application,name,version,installation,size",
];

fn ok(stdout: &str) -> CmdOutput {
    CmdOutput {
        status: 0,
        stdout: stdout.into(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn pacman_db(t: &FixtureTree) {
    t.mkdir("/var/lib/pacman/local");
}

fn firefox(t: &FixtureTree) {
    t.pacman_pkg(
        "firefox",
        "159.0.1-1",
        None,
        &[
            "usr/bin/firefox",
            "usr/share/applications/",
            "usr/share/applications/firefox.desktop",
        ],
    );
    t.desktop(
        "/usr/share/applications/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=/usr/bin/firefox %u\n",
    );
    t.write_exec("/usr/bin/firefox", b"");
}

fn app(source: SourceKind, name: &str, display: &str) -> App {
    let mut a = App::new(source, name);
    a.display_name = display.into();
    a
}

fn inv(apps: Vec<App>) -> Inventory {
    Inventory {
        apps,
        statuses: Vec::new(),
        errors: Vec::new(),
        owned: Default::default(),
    }
}

fn found<'a>(r: Resolved<'a>) -> &'a App {
    match r {
        Resolved::Found(a) => a,
        other => panic!("expected Found, got {other:?}"),
    }
}

mod find_desktop;
mod unmanaged_symlinked;
