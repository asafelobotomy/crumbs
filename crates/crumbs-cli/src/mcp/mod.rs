//! `crumbs mcp`: crumbs as an MCP server over stdio, for AI agents.
//!
//! stdout carries nothing but protocol frames, and stderr stays quiet:
//! warnings go to the client as log messages ([`log`]). Blocking crumbs
//! work runs on tokio's blocking pool, each call with its own `Env`.

mod apply;
#[cfg(test)]
mod apply_tests;
mod confirm;
mod log;
mod plans;
mod server;
#[cfg(test)]
pub(crate) mod testing;
mod tools;

use std::process::ExitCode;

use crumbs_core::Env;
use rmcp::ServiceExt;

use crate::cmd::render::sanitise;

/// Serves on stdin and stdout until the client closes stdin.
pub fn serve(env: Env) -> anyhow::Result<ExitCode> {
    // What the CLI warns about on every run is held for the client.
    let startup = crumbs_core::history::harden_files(&env)
        .iter()
        .map(|w| format!("history: {}", sanitise(w)))
        .collect();
    let server = server::Server::new(env, log::Log::new(startup));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let running = server.serve(rmcp::transport::stdio_pipes()).await?;
        running.waiting().await?;
        anyhow::Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use crumbs_core::testing::FixtureTree;
    use serde_json::json;

    use super::testing::{Connected, inky_with_system};

    fn fixture() -> FixtureTree {
        let t = FixtureTree::new();
        t.os_release("manjaro", "arch", "Manjaro Linux");
        t.write("/home/user/.cache/inky/blob", b"cache");
        t
    }

    #[tokio::test]
    async fn user_and_system_together_is_tool_error() {
        let t = fixture();
        let c = Connected::new(t.env()).await;
        let r = c
            .call("scan", json!({ "user": true, "system": true }))
            .await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(r.structured_content.as_ref().unwrap()["exit_code"], 2);
        let text = &r.content[0].as_text().unwrap().text;
        assert_eq!(text, crate::cmd::scan::USER_AND_SYSTEM);
        let r = c
            .call("orphans", json!({ "user": true, "system": true }))
            .await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(r.structured_content.as_ref().unwrap()["exit_code"], 2);
        c.close().await;
    }

    #[tokio::test]
    async fn unknown_names_are_tool_errors() {
        let t = fixture();
        let c = Connected::new(t.env()).await;
        let r = c.call("scan", json!({ "categories": ["bogus"] })).await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(
            r.content[0].as_text().unwrap().text,
            "unknown category 'bogus'"
        );
        let r = c.call("apps_list", json!({ "source": ["snap"] })).await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        let r = c.call("app_show", json!({ "app": "nope" })).await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(
            r.content[0].as_text().unwrap().text,
            "no installed app matches 'nope'"
        );
        c.close().await;
    }

    #[tokio::test]
    async fn config_warnings_arrive_as_log_messages() {
        let t = fixture();
        t.write(
            "/home/user/.config/crumbs/config.toml",
            b"categories_off = [\"bo\\u001bgus\"]\n",
        );
        let c = Connected::new(t.env()).await;
        let r = c.call("scan", json!({})).await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let warnings = c.warnings_with("unknown category").await;
        assert!(
            warnings.iter().any(|w| w.contains("unknown category")),
            "{warnings:?}"
        );
        assert!(warnings.iter().all(|w| !w.contains('\x1b')), "{warnings:?}");
        c.close().await;
    }

    #[tokio::test]
    async fn only_apply_is_listed_as_changing_anything() {
        let t = fixture();
        let c = Connected::new(t.env()).await;
        let tools = c.client.list_all_tools().await.unwrap();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "app_show",
                "apply",
                "apps_list",
                "doctor",
                "history",
                "orphans",
                "preview_clean",
                "preview_orphans",
                "preview_restore",
                "preview_uninstall",
                "scan"
            ]
        );
        for tool in &tools {
            let a = tool.annotations.as_ref();
            let read_only = a.and_then(|a| a.read_only_hint);
            let destructive = a.and_then(|a| a.destructive_hint);
            if tool.name == "apply" {
                assert_eq!(read_only, Some(false));
                assert_eq!(destructive, Some(true));
            } else {
                assert_eq!(read_only, Some(true), "{}", tool.name);
            }
        }
        c.close().await;
    }

    fn paths(plan_id_result: &serde_json::Value) -> Vec<(String, String)> {
        plan_id_result["plans"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["actions"].as_array().unwrap())
            .filter_map(|a| {
                Some((
                    a["action"]["type"].as_str()?.to_string(),
                    a["action"]["path"].as_str()?.to_string(),
                ))
            })
            .collect()
    }

    #[tokio::test]
    async fn preview_returns_plan_and_stores_it() {
        let t = inky_with_system();
        let env = t.env();
        let c = Connected::new(env.clone()).await;
        let r = c
            .call(
                "preview_uninstall",
                json!({ "app": "inky", "leftovers_only": true }),
            )
            .await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let data = r.structured_content.clone().unwrap();
        let id = data["plan_id"].as_str().unwrap();
        assert_eq!(id.len(), 32, "{id}");
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(data["report"], serde_json::Value::Null);
        assert_eq!(data["needs_admin"], false);
        assert!(
            data["expires_at"].as_str().unwrap().ends_with('Z'),
            "{data}"
        );
        assert_eq!(
            data["cli_command"],
            "crumbs uninstall inky --leftovers-only"
        );
        // The preview text is the CLI's, for the plan the store holds.
        let shown = data["preview_text"].as_str().unwrap();
        {
            let stored = c.plans.lock().unwrap();
            let plan = stored
                .peek(&super::plans::PlanId::parse(id).unwrap())
                .expect("stored");
            let super::plans::PlanPayload::Plans(targets) = &plan.payload else {
                panic!("not plans");
            };
            let want = crate::cmd::act::preview_text(&env, targets, &[], &[]);
            // Then the CLI's `note:` lines (here, the unselected system
            // leftovers).
            assert!(shown.starts_with(&want), "{shown}");
            assert_eq!(plan.preview_text, shown);
        }
        assert!(
            shown.contains("note: 2 system leftovers not selected"),
            "{shown}"
        );
        // The compact JSON text block carries the same data.
        let text = r.content[0].as_text().unwrap().text.clone();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            data
        );
        c.close().await;
    }

    #[tokio::test]
    async fn select_rejects_unknown_paths() {
        let t = inky_with_system();
        let c = Connected::new(t.env()).await;
        for bad in ["/etc/passwd", "~/.ssh"] {
            let r = c
                .call(
                    "preview_uninstall",
                    json!({ "app": "inky", "leftovers_only": true, "select": [bad] }),
                )
                .await;
            assert_eq!(r.is_error, Some(true), "{bad}: {r:?}");
            assert_eq!(r.structured_content.as_ref().unwrap()["exit_code"], 2);
            let text = &r.content[0].as_text().unwrap().text;
            assert!(text.starts_with("not found by crumbs:"), "{text}");
        }
        assert!(c.plans.lock().unwrap().is_empty());
        c.close().await;
    }

    #[tokio::test]
    async fn system_leftovers_need_explicit_select() {
        let t = inky_with_system();
        let c = Connected::new(t.env()).await;
        let args = json!({ "app": "inky", "leftovers_only": true });
        let r = c.call("preview_uninstall", args).await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let data = r.structured_content.unwrap();
        assert_eq!(data["needs_admin"], false, "{data}");
        let got = paths(&data);
        assert!(!got.is_empty(), "{data}");
        assert!(
            got.iter()
                .all(|(_, p)| !p.starts_with("/opt") && !p.starts_with("/etc"))
        );

        let r = c
            .call(
                "preview_uninstall",
                json!({ "app": "inky", "leftovers_only": true, "select": ["/opt/inky"] }),
            )
            .await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let data = r.structured_content.unwrap();
        assert_eq!(data["needs_admin"], true, "{data}");
        let text = data["preview_text"].as_str().unwrap();
        assert!(text.contains("/opt/inky"), "{text}");
        let actions = serde_json::to_string(&data["plans"]).unwrap();
        assert!(
            actions.contains("system-tree") || actions.contains("/opt/inky"),
            "{actions}"
        );
        c.close().await;
    }

    #[tokio::test]
    async fn expired_plan_is_purged() {
        let t = inky_with_system();
        let env = t.env();
        let server = super::server::Server::new(env.clone(), super::log::Log::default());
        let plans = server.plans.clone();
        let c =
            Connected::start_server(server.clone(), super::testing::TestClient::default()).await;
        let r = c
            .call(
                "preview_uninstall",
                json!({ "app": "inky", "leftovers_only": true }),
            )
            .await;
        let id = r.structured_content.unwrap()["plan_id"]
            .as_str()
            .unwrap()
            .to_string();
        let id = super::plans::PlanId::parse(&id).unwrap();
        // Just under the time to live: still there.
        let mut early = server.clone();
        early
            .env
            .set_clock_shift(std::time::Duration::from_secs(599));
        let c2 = Connected::start_server(early, super::testing::TestClient::default()).await;
        c2.call("doctor", json!({})).await;
        assert!(plans.lock().unwrap().peek(&id).is_some());
        let mut late = server;
        late.env
            .set_clock_shift(std::time::Duration::from_secs(601));
        let c3 = Connected::start_server(late, super::testing::TestClient::default()).await;
        c3.call("doctor", json!({})).await;
        assert!(plans.lock().unwrap().peek(&id).is_none());
        c.close().await;
        c2.close().await;
        c3.close().await;
    }

    #[tokio::test]
    async fn preview_clean_and_orphans_and_restore() {
        let t = fixture();
        let c = Connected::new(t.env()).await;
        let r = c
            .call("preview_clean", json!({ "categories": ["cache"] }))
            .await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        assert!(r.structured_content.unwrap()["plan_id"].is_string());
        // No selection: refused, nothing stored.
        let before = c.plans.lock().unwrap().len();
        let r = c.call("preview_orphans", json!({ "paths": [] })).await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(c.plans.lock().unwrap().len(), before);
        // Nothing in history: restore refuses.
        let r = c.call("preview_restore", json!({ "entry": "nope" })).await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert_eq!(r.structured_content.as_ref().unwrap()["exit_code"], 2);
        assert_eq!(c.plans.lock().unwrap().len(), before);
        c.close().await;
    }
}
