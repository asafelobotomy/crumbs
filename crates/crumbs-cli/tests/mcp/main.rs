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
        let mut server = Server::spawn(t, envs);
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

    /// `crumbs mcp` for a client on the 2026-07-28 inline lifecycle: no
    /// `initialize`, a `server/discover`, then `_meta` on every request.
    fn start_inline(t: &FixtureTree) -> Server {
        let mut server = Server::spawn(t, &[]);
        let discover = server.request("server/discover", json!({ "_meta": inline_meta() }));
        assert!(discover.get("result").is_some(), "{discover}");
        server
    }

    fn spawn(t: &FixtureTree, envs: &[(&str, &str)]) -> Server {
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
        Server {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            next_id: 1,
            elicit_answer: Some(json!({ "action": "decline" })),
            asked: Vec::new(),
        }
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

    /// `tools/call`'s result on the inline lifecycle: `extra` (an MRTR
    /// retry's `requestState` and `inputResponses`) beside the `_meta`.
    fn call_inline(&mut self, tool: &str, arguments: Value, extra: Value) -> Value {
        let mut params = json!({ "name": tool, "arguments": arguments, "_meta": inline_meta() });
        if let Value::Object(extra) = extra {
            params.as_object_mut().unwrap().extend(extra);
        }
        let v = self.request("tools/call", params);
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

/// The per-request `_meta` of the 2026-07-28 inline lifecycle, for a
/// client that can show a form.
fn inline_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "crumbs-test", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } },
    })
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

/// Previews the orphan `zzquux-thing` on the inline lifecycle; its id.
fn preview_orphan_inline(server: &mut Server) -> String {
    let r = server.call_inline(
        "preview_orphans",
        json!({ "paths": ["/home/user/.config/zzquux-thing"], "show_low": true }),
        json!({}),
    );
    assert_ne!(r["isError"], json!(true), "{r}");
    r["structuredContent"]["plan_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no plan: {r}"))
        .to_string()
}

/// The first `apply` round on the inline lifecycle: `input_required` with
/// one `confirm-<hex>` request and the plan id as state; the key.
fn ask_inline(server: &mut Server, id: &str) -> String {
    let r = server.call_inline("apply", json!({ "plan_id": id }), json!({}));
    assert_eq!(r["resultType"], "input_required", "{r}");
    assert_eq!(r["requestState"], id, "{r}");
    let requests = r["inputRequests"].as_object().unwrap();
    assert_eq!(requests.len(), 1, "{r}");
    let (key, request) = requests.iter().next().unwrap();
    let hex = key
        .strip_prefix("confirm-")
        .unwrap_or_else(|| panic!("{key}"));
    assert!(
        hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "{key}"
    );
    assert_eq!(request["method"], "elicitation/create", "{r}");
    key.clone()
}

/// The final `apply` round: the retry carrying `answer` under `key`.
fn answer_inline(server: &mut Server, id: &str, key: &str, answer: Value) -> Value {
    server.call_inline(
        "apply",
        json!({ "plan_id": id }),
        json!({ "requestState": id, "inputResponses": { key: answer } }),
    )
}

mod read_stdout;
