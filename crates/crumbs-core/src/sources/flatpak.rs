use crate::command::resolve_trusted;
use crate::env::Env;
use crate::error::{CoreError, SourceError};
use crate::model::{App, Installation, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

const LIST_ARGS: [&str; 3] = [
    "list",
    "--app",
    "--columns=application,name,version,installation,size",
];
const SYSTEM_APP_DIR: &str = "/var/lib/flatpak/app";

pub struct FlatpakSource;

impl AppSource for FlatpakSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Flatpak
    }

    fn available(&self, env: &Env) -> bool {
        resolve_trusted(env, "flatpak").is_some()
    }

    fn is_package_db(&self) -> bool {
        false
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let mut listing = SourceListing::default();
        let error = |message: String| SourceError {
            source: SourceKind::Flatpak,
            message,
        };
        let Some(program) = resolve_trusted(cx.env, "flatpak") else {
            listing.errors.push(error("flatpak not found".into()));
            return Ok(listing);
        };
        let out = match cx.runner.run(&program, &LIST_ARGS) {
            Ok(o) => o,
            Err(e) => {
                listing.errors.push(error(e.to_string()));
                return Ok(listing);
            }
        };
        if out.status != 0 {
            listing.errors.push(error(out.stderr.trim().to_string()));
            return Ok(listing);
        }

        for line in out.stdout.lines().filter(|l| !l.trim().is_empty()) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 4 || cols[0].is_empty() {
                listing
                    .errors
                    .push(error(format!("malformed row: {line:?}")));
                continue;
            }
            let id = cols[0];
            let mut app = App::new(SourceKind::Flatpak, id);
            app.rdns_id = Some(id.to_string());
            if !cols[1].is_empty() {
                app.display_name = cols[1].to_string();
            }
            app.version = Some(cols[2]).filter(|v| !v.is_empty()).map(String::from);
            let (installation, location) = if cols[3] == "user" {
                app.id = format!("flatpak:user/{id}");
                (
                    Installation::User,
                    cx.env.unsys(&cx.env.data_home.join("flatpak/app").join(id)),
                )
            } else {
                app.id = format!("flatpak:system/{id}");
                (
                    Installation::System,
                    std::path::Path::new(SYSTEM_APP_DIR).join(id),
                )
            };
            app.installation = installation;
            app.location = Some(location);
            app.install_size = cols.get(4).and_then(|s| parse_si_size(s));
            app.has_gui = true;
            let desktop_id = format!("{id}.desktop");
            app.desktop_ids.push(desktop_id.clone());
            listing.claimed_desktop_ids.insert(desktop_id);
            listing.apps.push(app);
        }
        Ok(listing)
    }
}

/// Parses decimal SI sizes as printed by flatpak (`34.4 MB`, `900 bytes`).
pub fn parse_si_size(s: &str) -> Option<u64> {
    let (num, unit) = s.trim().split_once(char::is_whitespace)?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" | "bytes" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some((n * mult).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::model::Installation;
    use crate::sources::ListOptions;
    use crate::testing::{FakeRunner, FixtureTree};

    const ARGS: [&str; 3] = [
        "list",
        "--app",
        "--columns=application,name,version,installation,size",
    ];

    const ROWS: &str = "app.xemu.xemu\txemu\t0.8.136\tuser\t34.4 MB\n\
com.dosbox_x.DOSBox-X\tDOSBox-X\t2026.10.01\tsystem\t51.4 MB\n\
com.obsproject.Studio\tOBS Studio\t33.0.0\tsystem\t593.5 MB\n";

    fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            status,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn list_with(t: &FixtureTree, runner: &FakeRunner) -> SourceListing {
        let env = t.env();
        let opts = ListOptions::default();
        let cx = SourceContext {
            env: &env,
            runner,
            desktop: &[],
            opts: &opts,
        };
        FlatpakSource.list(&cx).unwrap()
    }

    fn installed() -> FixtureTree {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/flatpak", b"");
        t
    }

    #[test]
    fn parses_flatpak_list() {
        let t = installed();
        let runner = FakeRunner::new().on("flatpak", &ARGS, out(0, ROWS, ""));
        let l = list_with(&t, &runner);
        assert!(l.errors.is_empty());
        assert_eq!(l.apps.len(), 3);

        let xemu = &l.apps[0];
        assert_eq!(xemu.id, "flatpak:user/app.xemu.xemu");
        assert_eq!(xemu.name, "app.xemu.xemu");
        assert_eq!(xemu.rdns_id.as_deref(), Some("app.xemu.xemu"));
        assert_eq!(xemu.display_name, "xemu");
        assert_eq!(xemu.version.as_deref(), Some("0.8.136"));
        assert_eq!(xemu.installation, Installation::User);
        assert_eq!(xemu.install_size, Some(34_400_000));
        assert_eq!(xemu.desktop_ids, ["app.xemu.xemu.desktop"]);
        assert!(xemu.has_gui);
        let loc = xemu.location.as_ref().unwrap();
        assert!(loc.ends_with("flatpak/app/app.xemu.xemu"));

        let obs = &l.apps[2];
        assert_eq!(obs.display_name, "OBS Studio");
        assert_eq!(obs.install_size, Some(593_500_000));
        assert_eq!(obs.installation, Installation::System);
        assert_eq!(
            obs.location.as_deref(),
            Some(std::path::Path::new(
                "/var/lib/flatpak/app/com.obsproject.Studio"
            ))
        );
        assert!(
            l.claimed_desktop_ids
                .contains("com.obsproject.Studio.desktop")
        );
        assert_eq!(runner.calls(), [format!("flatpak {}", ARGS.join(" "))]);
    }

    #[test]
    fn empty_version_is_none_and_unknown_installation_is_system() {
        let t = installed();
        let runner = FakeRunner::new().on("flatpak", &ARGS, out(0, "a.b.C\tC\t\tcustom\t?\n", ""));
        let l = list_with(&t, &runner);
        assert_eq!(l.apps.len(), 1);
        assert_eq!(l.apps[0].version, None);
        assert_eq!(l.apps[0].installation, Installation::System);
        assert_eq!(l.apps[0].install_size, None);
    }

    #[test]
    fn bad_rows_recorded_not_fatal() {
        let t = installed();
        let stdout = format!("\nshort\trow\n{ROWS}");
        let runner = FakeRunner::new().on("flatpak", &ARGS, out(0, &stdout, ""));
        let l = list_with(&t, &runner);
        assert_eq!(l.apps.len(), 3);
        assert_eq!(l.errors.len(), 1);
        assert_eq!(l.errors[0].source, SourceKind::Flatpak);
    }

    #[test]
    fn si_sizes() {
        assert_eq!(parse_si_size("12.0 kB"), Some(12_000));
        assert_eq!(parse_si_size("900 bytes"), Some(900));
        assert_eq!(parse_si_size("34.4 MB"), Some(34_400_000));
        assert_eq!(parse_si_size("1.2 GB"), Some(1_200_000_000));
        assert_eq!(parse_si_size("1 tb"), Some(1_000_000_000_000));
        assert_eq!(parse_si_size("5 B"), Some(5));
        assert_eq!(parse_si_size("?"), None);
        assert_eq!(parse_si_size(""), None);
        assert_eq!(parse_si_size("12 parsecs"), None);
        assert_eq!(parse_si_size("-3 MB"), None);
    }

    #[test]
    fn nonzero_exit_is_source_error() {
        let t = installed();
        let runner = FakeRunner::new().on("flatpak", &ARGS, out(1, "", "  boom\n"));
        let l = list_with(&t, &runner);
        assert!(l.apps.is_empty());
        assert_eq!(l.errors.len(), 1);
        assert_eq!(l.errors[0].source, SourceKind::Flatpak);
        assert!(l.errors[0].message.contains("boom"));
        assert!(!l.errors[0].message.contains("  boom"));
    }

    #[test]
    fn runner_failure_is_source_error() {
        let t = installed();
        let l = list_with(&t, &FakeRunner::new());
        assert!(l.apps.is_empty());
        assert_eq!(l.errors.len(), 1);
    }

    #[test]
    fn unavailable_when_not_in_trusted_dirs() {
        let t = FixtureTree::new();
        assert!(!FlatpakSource.available(&t.env()));
        t.write_exec("/opt/bin/flatpak", b"");
        assert!(!FlatpakSource.available(&t.env()));
        t.write_exec("/usr/bin/flatpak", b"");
        assert!(FlatpakSource.available(&t.env()));
        assert!(!FlatpakSource.is_package_db());
    }

    #[test]
    fn flatpak_ids_include_installation() {
        let t = installed();
        let rows = "org.x.Y\tY\t1\tuser\t1 MB\norg.x.Y\tY\t1\tsystem\t1 MB\n";
        let runner = FakeRunner::new().on("flatpak", &ARGS, out(0, rows, ""));
        let l = list_with(&t, &runner);
        let ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["flatpak:user/org.x.Y", "flatpak:system/org.x.Y"]);
        assert!(l.apps.iter().all(|a| a.name == "org.x.Y"));
    }
}
