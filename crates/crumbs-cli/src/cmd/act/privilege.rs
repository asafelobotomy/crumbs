//! How a run gets administrator rights: the escalation program, if any,
//! and the runner of the privileged batch.

use std::io::IsTerminal;

use crumbs_core::Env;
use crumbs_core::command::SystemRunner;
use crumbs_core::privilege::batch::{PrivilegedRunner, SystemPrivilegedRunner};
use crumbs_core::privilege::{Escalation, detect_escalation};

#[cfg(debug_assertions)]
use super::test_runner;

/// Whether crumbs can use `esc` now: one that needs a terminal only when
/// stdin is one.
pub fn usable(esc: &Escalation) -> bool {
    !esc.requires_tty || std::io::stdin().is_terminal()
}

/// How this run gets administrator rights: the escalation program (if
/// any) and the runner of the privileged batch.
pub(super) struct Privilege {
    pub(super) esc: Option<Escalation>,
    pub(super) runner: Box<dyn PrivilegedRunner>,
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
    pub(super) fn for_run(env: &Env, needed: bool) -> Privilege {
        let system =
            || -> Box<dyn PrivilegedRunner> { Box::new(SystemPrivilegedRunner::new(&env.root)) };
        if !needed {
            return Privilege {
                esc: None,
                runner: system(),
            };
        }
        #[cfg(debug_assertions)]
        {
            let value = std::env::var("CRUMBS_TEST_PRIV_RUNNER").ok();
            if let Some(runner) = test_runner::select(&env.root, value.as_deref()) {
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
        let esc = detect_escalation(env, &SystemRunner::new()).filter(usable);
        Privilege {
            esc,
            runner: system(),
        }
    }
}
