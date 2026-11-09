//! How a run gets administrator rights: the escalation program, if any,
//! and the runner of the privileged batch.

use std::io::IsTerminal;

use crumbs_core::Env;
use crumbs_core::command::SystemRunner;
use crumbs_core::privilege::batch::{PrivilegedRunner, SystemPrivilegedRunner};
use crumbs_core::privilege::{Escalation, Interaction, detect_escalation_for};

#[cfg(debug_assertions)]
use super::test_runner;

/// Whether crumbs can use `esc` now: one that needs a terminal only when
/// stdin is one.
pub fn usable(esc: &Escalation) -> bool {
    !esc.requires_tty || std::io::stdin().is_terminal()
}

/// How this run gets administrator rights: the escalation program (if
/// any) and the runner of the privileged batch.
pub(crate) struct Privilege {
    pub(crate) esc: Option<Escalation>,
    pub(crate) runner: Box<dyn PrivilegedRunner>,
}

impl Privilege {
    /// Detects the escalation program when `needed`. One that needs a
    /// terminal counts only when stdin is one.
    ///
    /// Debug builds never escalate for real unless
    /// `CRUMBS_ALLOW_REAL_ESCALATION=1`. Under a fake root (`CRUMBS_ROOT`)
    /// only, `CRUMBS_TEST_PRIV_RUNNER=sh` runs the batch with `/bin/sh` as
    /// the current user (`deny`: nothing runs, as when the prompt is
    /// dismissed; `partial:<n>`: like `sh`, but the shell stops before step
    /// `n`, as if it were killed); on the real root it is ignored. Release
    /// builds refuse to start when `CRUMBS_TEST_*` is set.
    pub(crate) fn for_run(env: &Env, needed: bool) -> Privilege {
        Privilege::build(env, needed, Interaction::Terminal)
    }

    /// For a server whose stdin and stdout carry a protocol: the same
    /// gating as [`for_run`](Self::for_run), always `needed`, but with no
    /// terminal: stdin from `/dev/null`, stderr captured, never `sudo`.
    #[allow(dead_code)] // used by the MCP server
    pub(crate) fn for_server(env: &Env) -> Privilege {
        Privilege::build(env, true, Interaction::NoTerminal)
    }

    /// The one place that gates real escalation, so `for_run` and
    /// `for_server` cannot drift apart.
    fn build(env: &Env, needed: bool, i: Interaction) -> Privilege {
        let system = || -> Box<dyn PrivilegedRunner> {
            Box::new(SystemPrivilegedRunner::with_interaction(&env.root, i))
        };
        if !needed {
            return Privilege {
                esc: None,
                runner: system(),
            };
        }
        #[cfg(debug_assertions)]
        {
            let value = std::env::var("CRUMBS_TEST_PRIV_RUNNER").ok();
            if let Some(runner) = test_runner::select(&env.root, value.as_deref(), i) {
                return Privilege {
                    esc: Some(test_runner::escalation()),
                    runner,
                };
            }
            if std::env::var_os("CRUMBS_ALLOW_REAL_ESCALATION").is_none_or(|v| v != "1") {
                return Privilege {
                    esc: None,
                    runner: system(),
                };
            }
        }
        let esc = detect_escalation_for(env, &SystemRunner::new(), i)
            .filter(|e| i == Interaction::NoTerminal || usable(e));
        Privilege {
            esc,
            runner: system(),
        }
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use crumbs_core::testing::FixtureTree;

    // One test mutates the process environment; it is the only one here.
    #[test]
    fn for_server_shares_the_gating_and_has_no_terminal() {
        let t = FixtureTree::new();
        let fake = t.env();
        let mut real = t.env();
        real.root = std::path::PathBuf::from("/");
        unsafe {
            std::env::set_var("CRUMBS_TEST_PRIV_RUNNER", "sh");
            std::env::remove_var("CRUMBS_ALLOW_REAL_ESCALATION");
        }
        let server = Privilege::for_server(&fake);
        let run = Privilege::for_run(&fake, true);
        assert!(server.esc.is_some() && run.esc.is_some());
        assert_eq!(server.runner.interaction(), Interaction::NoTerminal);
        assert_eq!(run.runner.interaction(), Interaction::Terminal);
        // On the real root the test runner is ignored, and without
        // CRUMBS_ALLOW_REAL_ESCALATION nothing is detected.
        let server = Privilege::for_server(&real);
        assert!(server.esc.is_none());
        assert_eq!(server.runner.interaction(), Interaction::NoTerminal);
        unsafe { std::env::remove_var("CRUMBS_TEST_PRIV_RUNNER") };
    }
}
