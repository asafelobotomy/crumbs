use super::*;

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

#[test]
fn multi_round_trip_confirmation_over_stdio() {
    let t = fixture();
    let state = t.dir.path().join("home/user/.config/zzquux-thing/state");
    let mut server = Server::start_inline(&t);

    // A no: nothing runs.
    let id = preview_orphan_inline(&mut server);
    // On this lifecycle a request without `_meta` is refused by rmcp.
    let bare = server.request(
        "tools/call",
        json!({ "name": "apply", "arguments": { "plan_id": id } }),
    );
    assert_eq!(bare["error"]["code"], -32602, "{bare}");
    assert_eq!(std::fs::read(&state).unwrap(), b"orphan");
    let key = ask_inline(&mut server, &id);
    let r = answer_inline(&mut server, &id, &key, json!({ "action": "decline" }));
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(
        r["structuredContent"],
        json!({ "applied": false, "reason": "declined" }),
        "{r}"
    );
    assert_eq!(std::fs::read(&state).unwrap(), b"orphan");

    // A fresh preview and a yes: the orphan goes to the trash.
    let id = preview_orphan_inline(&mut server);
    let key = ask_inline(&mut server, &id);
    let yes = json!({ "action": "accept", "content": { "confirm": true } });
    let r = answer_inline(&mut server, &id, &key, yes);
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["applied"], true, "{r}");
    assert_eq!(r["structuredContent"]["exit_code"], 0, "{r}");
    assert!(!state.exists());
    let trashed = t
        .dir
        .path()
        .join("home/user/.local/share/Trash/files/zzquux-thing/state");
    assert_eq!(std::fs::read(&trashed).unwrap(), b"orphan");

    // The server never asked through `elicitation/create` on this path.
    assert!(server.asked.is_empty(), "{:?}", server.asked);
    let (lines, _) = server.finish();
    for line in &lines {
        let v: Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON: {e}: {line:?}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
    }
}
