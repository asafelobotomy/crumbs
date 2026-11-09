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

fn command(t: &FixtureTree) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_crumbs"));
    cmd.env("CRUMBS_ROOT", t.dir.path());
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("CRUMBS_TEST_PRIV_RUNNER");
    cmd.env_remove("CRUMBS_ALLOW_REAL_ESCALATION");
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
}

impl Server {
    fn start(t: &FixtureTree) -> Server {
        let mut child = command(t)
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
        };
        let init = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
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

    /// Sends a request and waits for its response.
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
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v["id"] == json!(id)
            {
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

#[test]
fn stdout_carries_only_protocol() {
    let t = fixture_with_warnings();
    let mut server = Server::start(&t);
    let scan = server.call("scan", json!({}));
    assert_ne!(scan["isError"], json!(true), "{scan}");
    let (lines, stderr) = server.finish();
    assert!(stderr.is_empty(), "stderr: {stderr}");
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
