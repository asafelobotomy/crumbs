//! `crumbs mcp` as an MCP client sees it: the built binary over stdio.
#![cfg(all(feature = "mcp", debug_assertions))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use crumbs_core::testing::FixtureTree;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(60);
const FLATPAK_OK: &str = "#!/bin/sh\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n'\n";

fn fixture() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg(
        "firefox",
        "159.0.1-1",
        Some(300_000_000),
        &[
            "usr/bin/firefox",
            "usr/share/applications/",
            "usr/share/applications/firefox.desktop",
        ],
    );
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.desktop(
        "/usr/share/applications/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox %u\n",
    );
    t.write_exec("/usr/bin/firefox", b"");
    t.write_exec("/usr/bin/ls", b"");
    t.write("/home/user/.cache/mozilla/firefox/blob", b"cache");
    t.write("/home/user/.config/zzquux-thing/state", b"orphan");
    t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
    t.write("/home/user/.config/Tool/settings", b"x");
    t.write_exec("/usr/bin/flatpak", FLATPAK_OK.as_bytes());
    t
}

/// The fixture with a config value and a rule file crumbs warns about.
fn fixture_with_warnings() -> FixtureTree {
    let t = fixture();
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"categories_off = [\"bogus\"]\n",
    );
    t.write("/home/user/.config/crumbs/rules.d/bad.toml", b"[[rule]\n");
    t
}

/// Adds a package `inky` with unowned `/opt/inky` and `/etc/inky`, and
/// makes `/`, `/etc` and `/opt` pass the system-tree checks (this user
/// stands in for root).
fn with_inky_system(t: &FixtureTree) {
    t.pacman_pkg("inky", "1.0-1", Some(1_000), &["usr/bin/inky"]);
    t.write_exec("/usr/bin/inky", b"");
    t.write("/home/user/.cache/inky/blob", b"cache");
    t.write("/opt/inky/bin/inky", b"x");
    t.write("/etc/inky/inky.conf", b"x");
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        std::fs::set_permissions(
            t.dir.path().join(rel),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.write_exec("/usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    t.write_exec(
        "/usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    t.write(
        "/proc/self/mountinfo",
        b"36 35 98:0 /mnt1 / rw,noatime master:1 - ext3 /dev/root rw\n",
    );
}

fn command(t: &FixtureTree) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_crumbs"));
    cmd.env("CRUMBS_ROOT", t.dir.path());
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("CRUMBS_TEST_PRIV_RUNNER");
    cmd.env_remove("CRUMBS_ALLOW_REAL_ESCALATION");
    cmd.env_remove("CRUMBS_TEST_BEFORE_EXECUTE");
    cmd.env_remove("CRUMBS_TEST_CONFIRM_TIMEOUT_MS");
    cmd
}

/// `crumbs <args> --json`'s `data`.
fn cli_data(t: &FixtureTree, args: &[&str]) -> Value {
    let out = command(t).args(args).arg("--json").output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
    v["data"].clone()
}

/// A running `crumbs mcp`, spoken to one JSON-RPC line at a time.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// Every stdout line so far.
    seen: Vec<String>,
    next_id: u64,
    /// The result sent back for every `elicitation/create`; `None`: never
    /// answered.
    elicit_answer: Option<Value>,
    /// The message of each `elicitation/create` received.
    asked: Vec<String>,
}

impl Server {
    fn start(t: &FixtureTree) -> Server {
        Server::start_with(t, &[], json!({}))
    }

    /// `crumbs mcp` with `envs` set, for a client declaring `capabilities`.
    fn start_with(t: &FixtureTree, envs: &[(&str, &str)], capabilities: Value) -> Server {
        let mut cmd = command(t);
        cmd.envs(envs.iter().copied());
        let mut child = cmd
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut server = Server {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            next_id: 1,
            elicit_answer: Some(json!({ "action": "decline" })),
            asked: Vec::new(),
        };
        let init = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": capabilities,
                "clientInfo": { "name": "crumbs-test", "version": "0" },
            }),
        );
        assert!(init.get("result").is_some(), "{init}");
        server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        server
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    /// Sends a request and waits for its response, answering each
    /// `elicitation/create` on the way with `elicit_answer`.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = match self.lines.recv_timeout(TIMEOUT) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => panic!("no response to {method}"),
                Err(RecvTimeoutError::Disconnected) => panic!("stdout closed before {method}"),
            };
            self.seen.push(line.clone());
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["method"] == "elicitation/create" {
                let message = v["params"]["message"].as_str().unwrap_or_default();
                self.asked.push(message.to_string());
                if let Some(answer) = self.elicit_answer.clone() {
                    self.send(&json!({ "jsonrpc": "2.0", "id": v["id"], "result": answer }));
                }
                continue;
            }
            if v.get("method").is_none() && v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// `tools/call`'s result.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let v = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        v.get("result")
            .unwrap_or_else(|| panic!("{tool}: {v}"))
            .clone()
    }

    /// Closes stdin and waits for the exit: every stdout line, and stderr.
    fn finish(mut self) -> (Vec<String>, String) {
        drop(self.stdin.take());
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        loop {
            match self.lines.recv_timeout(TIMEOUT) {
                Ok(line) => self.seen.push(line),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("crumbs mcp did not exit"),
            }
        }
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status}: {stderr}");
        (self.seen, stderr)
    }
}

#[test]
fn read_tools_match_cli_json() {
    let t = fixture();
    // Some history to list.
    let out = command(&t)
        .args(["uninstall", "Tool", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let cases: [(&str, Value, &[&str]); 9] = [
        ("doctor", json!({}), &["doctor"]),
        ("apps_list", json!({}), &["apps", "list"]),
        (
            "apps_list",
            json!({ "all": true, "source": ["pacman"] }),
            &["apps", "list", "--all", "--source", "pacman"],
        ),
        (
            "app_show",
            json!({ "app": "firefox" }),
            &["apps", "show", "firefox"],
        ),
        ("scan", json!({}), &["scan"]),
        ("scan", json!({ "system": true }), &["scan", "--system"]),
        (
            "scan",
            json!({ "categories": ["cache", "trash"] }),
            &["scan", "--category", "cache", "--category", "trash"],
        ),
        ("orphans", json!({}), &["orphans"]),
        ("history", json!({}), &["history", "--limit", "50"]),
    ];
    let mut server = Server::start(&t);
    for (tool, arguments, cli) in cases {
        let result = server.call(tool, arguments.clone());
        assert_ne!(
            result["isError"],
            json!(true),
            "{tool} {arguments}: {result}"
        );
        let want = cli_data(&t, cli);
        assert!(!want.is_null(), "{cli:?}");
        assert_eq!(result["structuredContent"], want, "{tool} {arguments}");
        // The same data as compact JSON text, for clients without
        // structured content.
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), want);
    }
    assert!(
        !cli_data(&t, &["history"])["batches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    server.finish();
}

/// The stand-in privileged runner, and a before-execute hook that writes
/// on stdout and stderr.
const APPLY_ENVS: [(&str, &str); 2] = [
    ("CRUMBS_TEST_PRIV_RUNNER", "sh"),
    (
        "CRUMBS_TEST_BEFORE_EXECUTE",
        "echo hook-stdout; echo hook-stderr >&2; cat >/dev/null",
    ),
];

/// Previews `inky`'s cache plus `select` and applies it, answering the
/// confirmation with yes; the apply result.
fn preview_and_apply(server: &mut Server, select: &[&str]) -> Value {
    let mut all = vec!["/home/user/.cache/inky"];
    all.extend_from_slice(select);
    let r = server.call(
        "preview_uninstall",
        json!({ "app": "inky", "leftovers_only": true, "select": all }),
    );
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["needs_admin"], true, "{r}");
    let id = r["structuredContent"]["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    server.elicit_answer = Some(json!({ "action": "accept", "content": { "confirm": true } }));
    let r = server.call("apply", json!({ "plan_id": id }));
    assert_eq!(server.asked.len(), 1, "{:?}", server.asked);
    assert!(server.asked[0].ends_with("\n\nApply this plan?"));
    r
}

#[test]
fn stdout_carries_only_protocol() {
    let t = fixture_with_warnings();
    with_inky_system(&t);
    let mut server = Server::start_with(&t, &APPLY_ENVS, json!({ "elicitation": {} }));
    let scan = server.call("scan", json!({}));
    assert_ne!(scan["isError"], json!(true), "{scan}");
    // A privileged apply, whose hook and root batch both write output.
    let r = preview_and_apply(&mut server, &["/opt/inky"]);
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["applied"], true, "{r}");
    assert_eq!(r["structuredContent"]["exit_code"], 0, "{r}");
    assert!(!t.dir.path().join("opt/inky").exists());
    // The protocol still works after the run.
    let doctor = server.call("doctor", json!({}));
    assert_ne!(doctor["isError"], json!(true), "{doctor}");
    let (lines, stderr) = server.finish();
    // stderr holds only what the test stand-ins write; the hook's stdout
    // went there too.
    assert!(stderr.lines().any(|l| l == "hook-stdout"), "{stderr}");
    for line in stderr.lines() {
        assert!(
            [
                "hook-stdout",
                "hook-stderr",
                "crumbs-test: privileged batch"
            ]
            .contains(&line)
                || line.starts_with("crumbs-test: batch stdin "),
            "stderr: {stderr}"
        );
    }
    let mut warnings = Vec::new();
    for line in &lines {
        let v: Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        assert!(
            v.get("id").is_some() || v.get("method").is_some(),
            "not a JSON-RPC message: {line}"
        );
        if v["method"] == "notifications/message" && v["params"]["level"] == "warning" {
            warnings.push(v["params"]["data"].clone());
        }
    }
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().is_some_and(|s| s.contains("bogus"))),
        "{lines:#?}"
    );
}

#[test]
fn preview_tools_match_cli_dry_run() {
    let t = fixture();
    let want = cli_data(&t, &["uninstall", "Tool", "--dry-run"]);
    let mut server = Server::start(&t);
    let r = server.call("preview_uninstall", json!({ "app": "Tool" }));
    assert_ne!(r["isError"], json!(true), "{r}");
    let got = &r["structuredContent"];
    assert_eq!(got["plans"], want["plans"]);
    assert_eq!(got["report"], Value::Null);
    assert_eq!(got["cli_command"], "crumbs uninstall Tool");
    assert!(got["plan_id"].as_str().is_some_and(|id| id.len() == 32));
    assert!(got["preview_text"].as_str().unwrap().contains("ACTION"));
    // The preview changed nothing.
    assert!(
        t.dir
            .path()
            .join("home/user/Applications/Tool-1.0-x86_64.AppImage")
            .exists()
    );

    // After a real removal (by the CLI, in the fixture), restore previews.
    let out = command(&t)
        .args(["uninstall", "Tool", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let r = server.call(
        "preview_restore",
        json!({ "entry": "/home/user/.config/Tool" }),
    );
    assert_ne!(r["isError"], json!(true), "{r}");
    let got = &r["structuredContent"];
    assert!(got["plan_id"].is_string(), "{got}");
    assert_eq!(got["needs_admin"], false);
    assert_eq!(got["plans"], json!([]));
    let text = got["preview_text"].as_str().unwrap();
    assert!(text.contains("would restore ~/.config/Tool"), "{text}");
    // Previewing restored nothing.
    assert!(
        !t.dir
            .path()
            .join("home/user/.config/Tool/settings")
            .exists()
    );
    server.finish();
}

#[test]
fn privileged_plan_uses_one_batch_and_dev_null_stdin() {
    let t = fixture();
    with_inky_system(&t);
    let mut server = Server::start_with(&t, &APPLY_ENVS, json!({ "elicitation": {} }));
    let r = preview_and_apply(&mut server, &["/opt/inky", "/etc/inky"]);
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["exit_code"], 0, "{r}");
    assert!(!t.dir.path().join("opt/inky").exists());
    assert!(!t.dir.path().join("etc/inky").exists());
    assert!(!t.dir.path().join("home/user/.cache/inky").exists());
    let (_, stderr) = server.finish();
    // Both trees in one batch, whose shell found /dev/null on its stdin
    // (not the protocol pipe).
    assert_eq!(
        stderr.matches("crumbs-test: privileged batch").count(),
        1,
        "{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "crumbs-test: batch stdin /dev/null"),
        "{stderr}"
    );
    let rm = std::fs::read_to_string(t.dir.path().join("rm-args")).unwrap();
    assert_eq!(rm.lines().count(), 2, "{rm}");
}

#[test]
fn unanswered_confirmation_times_out_over_stdio() {
    let t = fixture();
    with_inky_system(&t);
    // Set on the child only: the test process's environment is untouched.
    let mut server = Server::start_with(
        &t,
        &[("CRUMBS_TEST_CONFIRM_TIMEOUT_MS", "200")],
        json!({ "elicitation": {} }),
    );
    server.elicit_answer = None;
    for round in 1..=2 {
        let r = server.call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        );
        let id = r["structuredContent"]["plan_id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = server.call("apply", json!({ "plan_id": id }));
        assert_eq!(
            r["structuredContent"],
            json!({ "applied": false, "reason": "timed out" }),
            "round {round}: {r}"
        );
        assert_eq!(server.asked.len(), round);
    }
    assert!(t.dir.path().join("home/user/.cache/inky/blob").exists());
    server.finish();
}
