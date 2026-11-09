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

fn test_runner_var() -> Option<String> {
    std::env::var("CRUMBS_TEST_PRIV_RUNNER").ok()
}

fn allow_real_var() -> bool {
    std::env::var_os("CRUMBS_ALLOW_REAL_ESCALATION").is_some_and(|v| v == "1")
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
        Privilege::build(
            env,
            needed,
            Interaction::Terminal,
            test_runner_var().as_deref(),
            allow_real_var(),
        )
    }

    /// For a server whose stdin and stdout carry a protocol: the same
    /// gating as [`for_run`](Self::for_run), always `needed`, but with no
    /// terminal: stdin from `/dev/null`, stderr captured, never `sudo`.
    #[allow(dead_code)] // used by the MCP server
    pub(crate) fn for_server(env: &Env) -> Privilege {
        Privilege::build(
            env,
            true,
            Interaction::NoTerminal,
            test_runner_var().as_deref(),
            allow_real_var(),
        )
    }

    /// The one place that gates real escalation, so `for_run` and
    /// `for_server` cannot drift apart.
    ///
    /// `test_runner` is `CRUMBS_TEST_PRIV_RUNNER` and `allow_real` is
    /// `CRUMBS_ALLOW_REAL_ESCALATION=1`; both are read by the callers, once,
    /// and only consulted in debug builds.
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    fn build(
        env: &Env,
        needed: bool,
        i: Interaction,
        test_runner: Option<&str>,
        allow_real: bool,
    ) -> Privilege {
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
            if let Some(runner) = test_runner::select(&env.root, test_runner, i) {
                return Privilege {
                    esc: Some(test_runner::escalation()),
                    runner,
                };
            }
            if !allow_real {
                return Privilege {
                    esc: None,
                    runner: system(),
                };
            }
        }
        let esc = detect_escalation_for(env, &SystemRunner::new(), i).filter(|e| match i {
            Interaction::NoTerminal => !e.requires_tty,
            Interaction::Terminal => usable(e),
        });
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

    #[test]
    fn for_server_gating_matches_for_run_and_has_no_terminal() {
        let t = FixtureTree::new();
        let fake = t.env();
        let mut real = t.env();
        real.root = std::path::PathBuf::from("/");
        let both = |env: &Env, runner: Option<&str>, allow: bool| {
            (
                Privilege::build(env, true, Interaction::Terminal, runner, allow),
                Privilege::build(env, true, Interaction::NoTerminal, runner, allow),
            )
        };
        // The test runner is used under a fake root, with its interaction.
        let (run, server) = both(&fake, Some("sh"), false);
        assert!(run.esc.is_some() && server.esc.is_some());
        assert_eq!(run.runner.interaction(), Interaction::Terminal);
        assert_eq!(server.runner.interaction(), Interaction::NoTerminal);
        // Under a fake root without a test runner and without the real
        // escalation flag: nothing is detected.
        let (run, server) = both(&fake, None, false);
        assert!(run.esc.is_none() && server.esc.is_none());
        // On the real root the test runner is ignored; without the flag
        // nothing is detected (the allow_real branch is not exercised).
        let (run, server) = both(&real, Some("sh"), false);
        assert!(run.esc.is_none() && server.esc.is_none());
        assert_eq!(server.runner.interaction(), Interaction::NoTerminal);
        // Not needed: no escalation at all.
        let p = Privilege::build(&fake, false, Interaction::Terminal, Some("sh"), false);
        assert!(p.esc.is_none());
    }
}
