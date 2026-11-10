use std::path::PathBuf;
use std::time::SystemTime;

use super::*;
use crate::command::CmdOutput;
use crate::config::Config;
use crate::inventory::Inventory;
use crate::testing::{FakeRunner, FixtureTree};

const IMAGES: [&str; 5] = ["images", "--filter", "dangling=true", "--format", "json"];
const VOLUMES: [&str; 6] = [
    "volume",
    "ls",
    "--filter",
    "dangling=true",
    "--format",
    "json",
];

fn out(status: i32, stdout: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn scan(t: &FixtureTree, runner: &FakeRunner) -> Result<Vec<BloatItem>, CoreError> {
    let env = t.env();
    let cfg = Config::default();
    let inv = Inventory {
        apps: vec![],
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    };
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &inv,
        runner,
        now: SystemTime::now(),
    };
    ContainersScanner.scan(&cx)
}

#[test]
fn podman_untagged_images_and_unused_volumes() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/podman", b"#!/bin/sh\n");
    t.write(
        "home/user/.local/share/containers/storage/volumes/cache/_data/f",
        &[1u8; 8192],
    );
    let images = r#"[{"Id":"aaa","Size":3000000000},{"Id":"bbb","Size":100}]"#;
    let volumes = r#"[{"Name":"cache","Mountpoint":"/home/user/.local/share/containers/storage/volumes/cache/_data"}]"#;
    let runner = FakeRunner::new().on("podman", &IMAGES, out(0, images)).on(
        "podman",
        &VOLUMES,
        out(0, volumes),
    );
    let items = scan(&t, &runner).unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    let img = &items[0];
    assert_eq!(img.category, Category::Containers);
    assert_eq!(img.label, "2 untagged podman images");
    assert_eq!(img.size, Some(3_000_000_100));
    assert!(img.preselected && img.rebuildable);
    assert_eq!(
        img.action,
        ItemAction::Command {
            program: "podman".into(),
            args: ["image", "prune", "-f"].map(String::from).to_vec(),
        }
    );
    let vol = &items[1];
    assert_eq!(vol.label, "podman volume cache");
    assert!(!vol.preselected && !vol.rebuildable);
    assert_eq!(
        vol.note.as_deref(),
        Some("used by no container, but a volume can hold data you want")
    );
    assert_eq!(
        vol.action,
        ItemAction::Command {
            program: "podman".into(),
            args: ["volume", "rm", "cache"].map(String::from).to_vec(),
        }
    );
    assert!(vol.size.is_some_and(|s| s >= 8192), "{:?}", vol.size);
}

#[test]
fn docker_json_lines_and_human_sizes() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/docker", b"#!/bin/sh\n");
    let images = "{\"ID\":\"aaa\",\"Size\":\"1.5GB\"}\n{\"ID\":\"bbb\",\"Size\":\"200MB\"}\n";
    let runner =
        FakeRunner::new()
            .on("docker", &IMAGES, out(0, images))
            .on("docker", &VOLUMES, out(0, ""));
    let items = scan(&t, &runner).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "2 untagged docker images");
    assert_eq!(items[0].size, Some(1_700_000_000));
    assert_eq!(
        items[0].action,
        ItemAction::Command {
            program: "docker".into(),
            args: ["image", "prune", "-f"].map(String::from).to_vec(),
        }
    );
}

#[test]
fn nothing_untagged_or_unreachable_engine_lists_nothing() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/podman", b"#!/bin/sh\n");
    t.write_exec("usr/bin/docker", b"#!/bin/sh\n");
    let runner = FakeRunner::new()
        .on("podman", &IMAGES, out(0, "[]"))
        .on("podman", &VOLUMES, out(0, "[]"))
        // Docker installed but its daemon is not reachable for this user.
        .on("docker", &IMAGES, out(1, ""))
        .on("docker", &VOLUMES, out(1, ""));
    assert!(scan(&t, &runner).unwrap().is_empty());
    // No engine at all.
    let t = FixtureTree::new();
    assert!(scan(&t, &FakeRunner::new()).unwrap().is_empty());
}

#[test]
fn volume_names_are_checked() {
    // A name that could be read as an option is never passed on.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/podman", b"#!/bin/sh\n");
    let volumes = r#"[{"Name":"-rf","Mountpoint":"/x"},{"Name":"ok_1.2-a","Mountpoint":"/y"}]"#;
    let runner = FakeRunner::new().on("podman", &IMAGES, out(0, "[]")).on(
        "podman",
        &VOLUMES,
        out(0, volumes),
    );
    let items = scan(&t, &runner).unwrap();
    let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(labels, ["podman volume ok_1.2-a"]);
    assert_eq!(items[0].size, None);
    let _ = PathBuf::new();
}
