use super::*;

#[tokio::test]
async fn mrtr_no_and_auto_driven_yes() {
    let t = inky_with_system();
    let env = t.env();
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::yes())).await;
    // A no in the final round removes the plan.
    let id = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let (key, _, _) = input_required(&c.call_once("apply", args.clone(), None, None).await);
    let no = json!({ "action": "accept", "content": { "confirm": false } });
    let r = complete(
        c.call_once("apply", args.clone(), Some(&id), Some(responses(&key, no)))
            .await,
    );
    assert_eq!(data(&r), json!({ "applied": false, "reason": "declined" }));
    assert!(!stored(&c, &id));
    assert!(env.sys(CACHE).join("blob").exists());
    // rmcp's client drives the rounds itself, asking its handler.
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    assert_eq!(c.handler.asked().len(), 1);
    assert!(!env.sys(CACHE).exists());
    c.close().await;
}

#[tokio::test]
async fn mrtr_plan_expires_while_waiting() {
    let t = inky_with_system();
    let env = t.env();
    let server = test_server(env.clone(), None);
    let c = Connected::start_modern(server.clone(), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let (key, _, _) = input_required(&c.call_once("apply", args.clone(), None, None).await);
    let mut late = server;
    late.env.set_clock_shift(Duration::from_secs(601));
    let c2 = Connected::start_modern(late, Asker::new(Answer::yes())).await;
    let yes = json!({ "action": "accept", "content": { "confirm": true } });
    let r = complete(
        c2.call_once("apply", args, Some(&id), Some(responses(&key, yes)))
            .await,
    );
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
    c2.close().await;
}

#[tokio::test]
async fn mrtr_client_without_elicitation_is_refused() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_modern(test_server(env.clone(), None), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let r = complete(
        c.call_once("apply", json!({ "plan_id": id }), None, None)
            .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(text(&r).contains("crumbs uninstall inky"), "{r:?}");
    assert!(stored(&c, &id));
    c.close().await;
}

#[tokio::test]
async fn mrtr_answer_before_the_question_is_refused() {
    let t = inky_with_system();
    let env = t.env();
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[]).await;
    // The real key, sent before the server ever asked.
    let key = c
        .plans
        .lock()
        .unwrap()
        .peek(&PlanId::parse(&id).unwrap())
        .unwrap()
        .confirm_key
        .clone();
    let yes = json!({ "action": "accept", "content": { "confirm": true } });
    let r = complete(
        c.call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses(&key, yes.clone())),
        )
        .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    assert!(stored(&c, &id));
    // Once asked, the same answer runs it.
    let (asked_key, _, _) = input_required(
        &c.call_once("apply", json!({ "plan_id": id }), None, None)
            .await,
    );
    assert_eq!(asked_key, key);
    let r = complete(
        c.call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses(&key, yes)),
        )
        .await,
    );
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    c.close().await;
}

#[tokio::test]
async fn privileged_plan_without_rights_returns_manual_commands() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[CACHE, "/opt/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 5, "{d}");
    let manual = d["manual_commands"].as_array().unwrap();
    assert!(
        manual
            .iter()
            .any(|m| m.as_str().unwrap().contains("'-rf' '--one-file-system'")),
        "{d}"
    );
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    // Nothing ran: not even the user's own files.
    assert!(env.sys("/opt/inky/bin/inky").exists());
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn privileged_plan_runs_in_one_batch_without_a_terminal() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(
        test_server(env.clone(), Some("sh")),
        Asker::new(Answer::yes()),
    )
    .await;
    let id = preview(&c, &[CACHE, "/opt/inky", "/etc/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["exit_code"], 0, "{d}");
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/etc/inky").exists());
    assert!(!env.sys(CACHE).exists());
    let h = history(&env);
    assert!(
        h.iter()
            .filter(|e| e.action == HistoryAction::Delete)
            .all(|e| e.ok),
        "{h:?}"
    );
    c.close().await;
}

#[tokio::test]
async fn denied_escalation_of_a_system_only_plan_applies_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(denying_server(env.clone()), Asker::new(Answer::yes())).await;
    let id = preview(&c, &["/opt/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 5, "{d}");
    assert!(!d["manual_commands"].as_array().unwrap().is_empty(), "{d}");
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    assert!(
        d["escalation_output"]
            .as_str()
            .unwrap()
            .contains("not authorized"),
        "{d}"
    );
    // Nothing ran, so there is no report.
    assert!(d.get("report").is_none(), "{d}");
    assert!(env.sys("/opt/inky/bin/inky").exists());
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn denied_escalation_returns_its_output_sanitised() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(denying_server(env.clone()), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[CACHE, "/opt/inky"]).await;
    let r = apply(&c, &id).await;
    let d = data(&r);
    assert_eq!(d["exit_code"], 5, "{d}");
    // The user's own files were handled before the prompt: something ran.
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    let output = d["escalation_output"].as_str().unwrap();
    assert!(output.contains("not authorized"), "{output:?}");
    assert!(!output.contains('\x1b'), "{output:?}");
    assert!(!d["manual_commands"].as_array().unwrap().is_empty(), "{d}");
    // The user's own files went; the report says what ran.
    assert!(!env.sys(CACHE).exists());
    assert!(!d["report"].is_null(), "{d}");
    assert!(env.sys("/opt/inky/bin/inky").exists());
    c.close().await;
}

#[tokio::test]
async fn restore_applies_exactly_the_previewed_entries() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    // Trash the cache once.
    let id = preview(&c, &[]).await;
    assert_eq!(data(&apply(&c, &id).await)["applied"], true);
    let first = history(&env)
        .into_iter()
        .find(|e| e.action == HistoryAction::Trash && e.path.ends_with(".cache/inky"))
        .unwrap();
    // Preview putting it back, by path.
    let r = c.call("preview_restore", json!({ "entry": CACHE })).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    assert_eq!(data(&r)["needs_admin"], false);
    let restore_id = data(&r)["plan_id"].as_str().unwrap().to_string();
    {
        let plans = c.plans.lock().unwrap();
        let plan = plans.peek(&PlanId::parse(&restore_id).unwrap()).unwrap();
        let PlanPayload::Restore { entry_ids } = &plan.payload else {
            panic!("not a restore");
        };
        assert_eq!(entry_ids, std::slice::from_ref(&first.id));
    }
    // A newer trash entry for the same path, before the restore runs.
    t.write("/home/user/.cache/inky/blob2", b"second");
    let id = preview(&c, &[]).await;
    assert_eq!(data(&apply(&c, &id).await)["applied"], true);
    assert!(!env.sys(CACHE).exists());

    let r = apply(&c, &restore_id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["exit_code"], 0, "{d}");
    // The first trashing came back, not the newer one.
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(!env.sys(CACHE).join("blob2").exists());
    let restores: Vec<HistoryEntry> = history(&env)
        .into_iter()
        .filter(|e| e.action == HistoryAction::Restore)
        .collect();
    assert_eq!(restores.len(), 1, "{restores:?}");
    assert_eq!(restores[0].restores.as_deref(), Some(first.id.as_str()));
    c.close().await;
}

#[tokio::test]
async fn preview_past_the_cap_is_refused() {
    let t = inky_with_system();
    let c = connect(t.env(), Answer::yes()).await;
    {
        let mut plans = c.plans.lock().unwrap();
        for _ in 0..MAX_PLANS {
            plans.insert(dummy_plan()).unwrap();
        }
    }
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), TOO_MANY_PLANS);
    assert_eq!(c.plans.lock().unwrap().len(), MAX_PLANS);
    c.close().await;
}

#[tokio::test]
async fn preview_fails_when_the_expiry_is_out_of_range() {
    let t = inky_with_system();
    let mut server: Server = test_server(t.env(), None);
    server
        .env
        .set_clock_shift(Duration::from_secs(300_000_000_000));
    let c = Connected::start_server(server, Asker::new(Answer::yes())).await;
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(c.plans.lock().unwrap().is_empty());
    c.close().await;
}
