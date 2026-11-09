//! Environment health report for `crumbs doctor`.

use crate::config::Config;
use std::path::PathBuf;

use rustix::fs::{Access, access};
use serde::Serialize;

use crate::command::CommandRunner;
use crate::env::{Distro, Env};
use crate::error::SourceError;
use crate::inventory::{SourceStatus, discover};
use crate::privilege::{Escalation, detect_escalation};
use crate::sources::ListOptions;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct TrashStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub writable: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct DoctorReport {
    pub crumbs_version: String,
    pub distro: Distro,
    pub sources: Vec<SourceStatus>,
    pub escalation: Option<Escalation>,
    pub trash: TrashStatus,
    pub data_dirs: Vec<PathBuf>,
    pub errors: Vec<SourceError>,
}

fn trash_status(env: &Env) -> TrashStatus {
    let real = env.data_home.join("Trash");
    let exists = real.is_dir();
    TrashStatus {
        path: env.unsys(&real),
        exists,
        writable: exists && access(&real, Access::WRITE_OK).is_ok(),
    }
}

pub fn doctor(env: &Env, runner: &dyn CommandRunner, cfg: &Config) -> DoctorReport {
    let inv = discover(env, runner, &ListOptions::default(), cfg);
    DoctorReport {
        crumbs_version: env!("CARGO_PKG_VERSION").to_string(),
        distro: env.distro.clone(),
        sources: inv.statuses,
        escalation: detect_escalation(env, runner),
        trash: trash_status(env),
        data_dirs: env.data_dirs.iter().map(|d| env.unsys(d)).collect(),
        errors: inv.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;
    use crate::testing::{FakeRunner, FixtureTree};

    #[test]
    fn doctor_reports_sources_and_trash() {
        let t = FixtureTree::new();
        t.os_release("manjaro", "arch", "Manjaro Linux");
        t.mkdir("/var/lib/pacman/local");
        t.mkdir("/home/user/.local/share/Trash");
        t.write_exec("/usr/bin/sudo", b"");
        let r = crate::privilege::allow_detection_in_test(|| {
            doctor(
                &crate::testing::non_root(t.env()),
                &FakeRunner::new(),
                &Config::default(),
            )
        });
        assert_eq!(r.crumbs_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(r.distro.id, "manjaro");
        let kinds: Vec<_> = r.sources.iter().map(|s| s.source).collect();
        assert_eq!(kinds[0], SourceKind::Pacman);
        assert!(r.sources[0].available);
        assert_eq!(
            r.escalation.as_ref().unwrap().kind,
            crate::privilege::EscalationKind::Sudo
        );
        assert_eq!(
            r.trash,
            TrashStatus {
                path: PathBuf::from("/home/user/.local/share/Trash"),
                exists: true,
                writable: true
            }
        );
        assert_eq!(
            r.data_dirs,
            [
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share")
            ]
        );
        assert!(r.errors.is_empty());
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["escalation"]["kind"], "sudo");
    }

    #[test]
    fn missing_trash_is_not_writable() {
        let t = FixtureTree::new();
        let r = doctor(&t.env(), &FakeRunner::new(), &Config::default());
        assert!(!r.trash.exists);
        assert!(!r.trash.writable);
        assert!(r.escalation.is_none());
    }
}
