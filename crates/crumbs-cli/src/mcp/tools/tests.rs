use super::*;

#[test]
fn refusal_text_and_details_are_sanitised() {
    let r = Refusal::new(2, "no app '\x1b[2Jx'").with_details(vec!["a\x07b".into()]);
    let result = refusal_result(&r);
    assert_eq!(result.is_error, Some(true));
    let text = result.content[0].as_text().unwrap().text.clone();
    assert!(!text.contains('\x1b'), "{text:?}");
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["exit_code"], 2);
    let detail = structured["details"][0].as_str().unwrap();
    assert!(!detail.contains('\x07'), "{detail:?}");
}

#[test]
fn crumbs_stopped_is_sanitised() {
    let e = crumbs_stopped(&"task \x1b[2Jpanicked\r");
    assert!(
        e.message.starts_with("crumbs stopped: task"),
        "{}",
        e.message
    );
    assert!(!e.message.contains('\x1b') && !e.message.contains('\r'));
}

#[test]
fn history_limit_defaults_to_50() {
    let p: HistoryParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(p.limit, 50);
}
