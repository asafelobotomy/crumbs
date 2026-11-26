use super::*;

#[tokio::test]
async fn apply_confirmed_runs_plan_and_writes_history() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    let preview_text = data(&r)["preview_text"].as_str().unwrap().to_string();
    let id = data(&r)["plan_id"].as_str().unwrap().to_string();

    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["exit_code"], 0, "{d}");
    let results = &d["report"][0]["results"];
    assert!(
        results.as_array().unwrap().iter().all(|r| r["ok"] == true),
        "{d}"
    );
    // The user was asked once, with the preview the agent was shown, one
    // blank line, then the question.
    assert!(
        preview_text.ends_with('\n') && !preview_text.ends_with("\n\n"),
        "{preview_text:?}"
    );
    assert_eq!(
        c.handler.asked(),
        [format!("{preview_text}\nApply this plan?")]
    );
    // In the trash, and in the history.
    assert!(!env.sys(CACHE).exists());
    let trash = env.sys("/home/user/.local/share/Trash/files/inky");
    assert!(trash.join("blob").exists(), "{}", trash.display());
    let h = history(&env);
    assert!(
        h.iter()
            .any(|e| e.action == HistoryAction::Trash && e.ok && e.path.ends_with(".cache/inky")),
        "{h:?}"
    );
    // The compact JSON text block carries the same data.
    assert_eq!(serde_json::from_str::<Value>(&text(&r)).unwrap(), d);
    // System leftovers were never selected.
    assert!(env.sys("/opt/inky/bin/inky").exists());
    c.close().await;
}

#[tokio::test]
async fn apply_without_confirmation_changes_nothing() {
    for (answer, reason) in [(Answer::Decline, "declined"), (Answer::Cancel, "cancelled")] {
        let t = inky_with_system();
        let env = t.env();
        let c = connect(env.clone(), answer).await;
        let id = preview(&c, &[]).await;
        let r = apply(&c, &id).await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        assert_eq!(data(&r), json!({ "applied": false, "reason": reason }));
        assert!(env.sys(CACHE).join("blob").exists());
        assert!(history(&env).is_empty());
        // The first round's no removes the plan.
        assert!(!stored(&c, &id));
        c.close().await;
    }

    // A client that cannot ask: refused before anything, the plan kept.
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start(env.clone(), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let msg = text(&r);
    assert_eq!(
        msg,
        "this client can't ask you to confirm; run this in a terminal instead: \
         crumbs uninstall inky --leftovers-only"
    );
    assert!(c.handler.asked().is_empty());
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    assert!(stored(&c, &id));
    c.close().await;
}

#[tokio::test]
async fn apply_rejects_accept_without_confirm_true() {
    for answer in [
        Answer::Accept(json!({ "confirm": false })),
        Answer::AcceptEmpty,
        Answer::Accept(json!({ "confirm": "true" })),
        Answer::Accept(json!({})),
    ] {
        let t = inky_with_system();
        let env = t.env();
        let c = connect(env.clone(), answer.clone()).await;
        let id = preview(&c, &[]).await;
        let r = apply(&c, &id).await;
        assert_eq!(
            data(&r),
            json!({ "applied": false, "reason": "declined" }),
            "{answer:?}"
        );
        assert!(env.sys(CACHE).join("blob").exists(), "{answer:?}");
        assert!(history(&env).is_empty(), "{answer:?}");
        c.close().await;
    }
}

#[tokio::test]
async fn apply_refuses_swapped_target() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    // Another directory at the same path: a new inode.
    std::fs::rename(env.sys(CACHE), env.sys("/home/user/.cache/inky.old")).unwrap();
    t.write("/home/user/.cache/inky/new", b"new");
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    // Its one action failed: nothing was applied, and the report says why.
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 3, "{d}");
    let result = &d["report"][0]["results"][0];
    assert_eq!(result["ok"], false, "{d}");
    assert!(
        result["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{d}"
    );
    assert!(env.sys("/home/user/.cache/inky/new").exists());
    c.close().await;
}

#[tokio::test]
async fn input_responses_never_confirm_before_2026() {
    // A pre-2026 client answers the elicitation with no; MRTR fields it
    // sends on its own count for nothing.
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::Decline).await;
    let id = preview(&c, &[]).await;
    let key = c
        .plans
        .lock()
        .unwrap()
        .peek(&PlanId::parse(&id).unwrap())
        .unwrap()
        .confirm_key
        .clone();
    let mut responses = InputResponses::new();
    responses.insert(
        key,
        json!({ "action": "accept", "content": { "confirm": true } }),
    );
    let r = c
        .call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses),
        )
        .await;
    let CallToolResponse::Complete(r) = r else {
        panic!("{r:?}")
    };
    assert_eq!(data(&r), json!({ "applied": false, "reason": "declined" }));
    assert_eq!(c.handler.asked().len(), 1);
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn plan_id_is_single_use() {
    let t = inky_with_system();
    let c = connect(t.env(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    // An id never issued, or not shaped like one: the same answer.
    for bogus in ["0".repeat(32), "nope".to_string()] {
        let r = apply(&c, &bogus).await;
        assert_eq!(text(&r), PLAN_NOT_FOUND);
    }
    // Only one confirmation was asked for.
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}

#[tokio::test]
async fn plan_that_expires_while_the_user_answers_runs_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let server = test_server(env.clone(), None);
    let shift = server.clock_shift.clone();
    // A slow yes: by the time it arrives the plan has expired.
    let mut asker = Asker::new(Answer::yes());
    asker.while_asked = Some(Arc::new(move || {
        *shift.lock().unwrap() = Some(Duration::from_secs(601));
    }));
    let c = Connected::start_server(server.clone(), asker).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), EXPIRED_WHILE_ASKING);
    assert_eq!(c.handler.asked().len(), 1);
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    // The plan stays dropped, even with the clock back.
    *server.clock_shift.lock().unwrap() = None;
    assert!(!stored(&c, &id));
    let r = apply(&c, &id).await;
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}

#[tokio::test]
async fn unanswered_confirmation_times_out_and_drops_plan() {
    let t = inky_with_system();
    let env = t.env();
    let mut server = test_server(env.clone(), None);
    server.confirm_timeout = Duration::from_millis(200);
    let c = Connected::start_server(server, Asker::new(Answer::Never)).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r), json!({ "applied": false, "reason": "timed out" }));
    // The lock was let go: another apply gets as far as asking.
    let second = preview(&c, &[]).await;
    let r = apply(&c, &second).await;
    assert_eq!(data(&r), json!({ "applied": false, "reason": "timed out" }));
    assert_eq!(c.handler.asked().len(), 2);
    let r = apply(&c, &id).await;
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    c.close().await;
}

#[tokio::test]
async fn mrtr_flow_requires_matching_state() {
    let t = inky_with_system();
    let env = t.env();
    // The client would answer no if asked through `elicitation/create`:
    // on 2026-07-28 it never is.
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::Decline)).await;
    let id = preview(&c, &[]).await;
    let other = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let first = c.call_once("apply", args.clone(), None, None).await;
    let (key, state, message) = input_required(&first);
    assert_eq!(state, id);
    assert!(message.ends_with(CONFIRM_SUFFIX), "{message}");
    assert!(stored(&c, &id), "the plan waits for the answer");
    let yes = json!({ "action": "accept", "content": { "confirm": true } });

    // Another plan's state, a state without answers, answers without a
    // state, or an answer under another key: nothing runs.
    let refused = [
        (Some(other.as_str()), Some(responses(&key, yes.clone()))),
        (Some(id.as_str()), None),
        (None, Some(responses(&key, yes.clone()))),
        (
            Some(id.as_str()),
            Some(responses("confirm-other", yes.clone())),
        ),
        (Some(id.as_str()), Some(responses(&id, yes.clone()))),
    ];
    for (state, answers) in refused {
        let r = complete(c.call_once("apply", args.clone(), state, answers).await);
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert!(env.sys(CACHE).join("blob").exists());
        assert!(history(&env).is_empty());
        assert!(stored(&c, &id));
    }
    // The other plan's own key does not confirm this one.
    let other_first = c
        .call_once("apply", json!({ "plan_id": other }), None, None)
        .await;
    let (other_key, _, _) = input_required(&other_first);
    assert_ne!(other_key, key);
    let r = complete(
        c.call_once(
            "apply",
            args.clone(),
            Some(&id),
            Some(responses(&other_key, yes.clone())),
        )
        .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());

    // The right state and the issued key, with confirm: true.
    let r = complete(
        c.call_once("apply", args.clone(), Some(&id), Some(responses(&key, yes)))
            .await,
    );
    assert_ne!(r.is_error, Some(true), "{r:?}");
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    assert!(!env.sys(CACHE).exists());
    assert!(!stored(&c, &id));
    assert!(c.handler.asked().is_empty());
    c.close().await;
}
