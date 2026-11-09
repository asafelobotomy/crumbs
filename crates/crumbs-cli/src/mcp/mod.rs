//! `crumbs mcp`: crumbs as an MCP server over stdio, for AI agents.
//!
//! stdout carries nothing but protocol frames, and stderr stays quiet:
//! warnings go to the client as log messages ([`log`]). Blocking crumbs
//! work runs on tokio's blocking pool, each call with its own `Env`.

mod log;
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

    use super::testing::Connected;

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
    async fn read_tools_are_listed_read_only() {
        let t = fixture();
        let c = Connected::new(t.env()).await;
        let tools = c.client.list_all_tools().await.unwrap();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "app_show",
                "apps_list",
                "doctor",
                "history",
                "orphans",
                "scan"
            ]
        );
        for tool in &tools {
            let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
            assert_eq!(read_only, Some(true), "{}", tool.name);
        }
        c.close().await;
    }
}
