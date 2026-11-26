use super::*;

#[tokio::test]
async fn preview_text_carries_no_control_bytes() {
    let t = inky_with_system();
    let env = t.env();
    t.write("/home/user/.cache/inky\x1b[31mred\rback/blob", b"x");
    let c = connect(env.clone(), Answer::Decline).await;
    let r = c
        .call("preview_clean", json!({ "categories": ["cache"] }))
        .await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    let shown = d["preview_text"].as_str().unwrap();
    assert!(shown.contains("red"), "{shown:?}");
    let control = |s: &str| s.chars().any(|ch| ch.is_control() && ch != '\n');
    assert!(!control(shown), "{shown:?}");
    let id = d["plan_id"].as_str().unwrap().to_string();
    apply(&c, &id).await;
    let asked = c.handler.asked();
    assert_eq!(asked.len(), 1);
    assert!(!control(&asked[0]), "{:?}", asked[0]);
    c.close().await;
}

#[tokio::test]
async fn failed_confirmation_runs_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::Error).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(text(&r).contains("nothing was run"), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    c.close().await;
}

#[tokio::test]
async fn request_and_handshake_must_both_allow_a_form() {
    // The handshake declared no elicitation; this request's `_meta` claims
    // it. Nothing is asked, and the plan stays.
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start(env.clone(), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let mut meta = rmcp::model::RequestMetaObject::new();
    meta.set_client_capabilities(
        rmcp::model::ClientCapabilities::builder()
            .enable_elicitation()
            .build(),
    );
    let r = c
        .call_with_meta("apply", json!({ "plan_id": id }), meta)
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(
        text(&r).starts_with("this client can't ask you to confirm"),
        "{r:?}"
    );
    assert!(text(&r).contains("crumbs uninstall inky"), "{r:?}");
    assert!(c.handler.asked().is_empty());
    assert!(stored(&c, &id));
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn two_applies_of_one_plan_run_it_once() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    let (a, b) = tokio::join!(apply(&c, &id), apply(&c, &id));
    let ran: Vec<&CallToolResult> = [&a, &b]
        .into_iter()
        .filter(|r| data(r)["applied"] == true)
        .collect();
    assert_eq!(ran.len(), 1, "{a:?} {b:?}");
    let refused: Vec<&CallToolResult> = [&a, &b]
        .into_iter()
        .filter(|r| r.is_error == Some(true))
        .collect();
    assert_eq!(refused.len(), 1, "{a:?} {b:?}");
    assert_eq!(text(refused[0]), PLAN_NOT_FOUND);
    let mut batches: Vec<String> = history(&env).into_iter().map(|e| e.batch).collect();
    batches.dedup();
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}
