//! Keeps SIGINT and SIGQUIT from ending crumbs while the privileged batch
//! runs. std has no signal API and rustix offers only its experimental
//! `runtime` one, so this declares libc's `signal` (std already links
//! libc). A no-op handler is installed rather than `SIG_IGN`: a caught
//! signal is reset to its default across `exec`, so the escalation program
//! and the steps still see Ctrl-C, while an ignored one would be inherited.
//! glibc's `signal` sets `SA_RESTART`, and std retries on `EINTR` anyway.
//! A signal that is already ignored is left ignored.

use std::os::raw::c_int;

const SIGINT: c_int = 2;
const SIGQUIT: c_int = 3;
const SIG_IGN: usize = 1;
const SIG_ERR: usize = usize::MAX;

unsafe extern "C" {
    fn signal(signum: c_int, handler: usize) -> usize;
}

extern "C" fn ignore(_: c_int) {}

/// Restores the previous handlers when dropped.
pub(super) struct Held(Vec<(c_int, usize)>);

pub(super) fn hold() -> Held {
    let handler = ignore as extern "C" fn(c_int) as usize;
    let mut previous = Vec::new();
    for sig in [SIGINT, SIGQUIT] {
        // SAFETY: `ignore` does nothing, so it is async-signal-safe.
        let old = unsafe { signal(sig, handler) };
        if old == SIG_IGN {
            // Already ignored (e.g. started under `nohup`): keep it so,
            // children included.
            // SAFETY: puts back the disposition `signal` returned.
            unsafe { signal(sig, SIG_IGN) };
        } else if old != SIG_ERR {
            previous.push((sig, old));
        }
    }
    Held(previous)
}

impl Drop for Held {
    fn drop(&mut self) {
        for &(sig, old) in &self.0 {
            // SAFETY: `old` is the handler `signal` returned for `sig`.
            unsafe { signal(sig, old) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn raise(sig: c_int) -> c_int;
    }

    /// Signal dispositions are process-wide and tests run in parallel:
    /// every test that changes them holds this.
    static SIGNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SIGNAL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The disposition of `sig`, briefly set to ignored to read it.
    fn current(sig: c_int) -> usize {
        // SAFETY: puts the old disposition straight back.
        unsafe {
            let old = signal(sig, SIG_IGN);
            signal(sig, old);
            old
        }
    }

    #[test]
    fn held_signals_do_not_end_the_process() {
        let _guard = lock();
        let before = current(SIGINT);
        let held = hold();
        // SAFETY: SIGINT and SIGQUIT have the no-op handler now.
        assert_eq!(unsafe { raise(SIGINT) }, 0);
        assert_eq!(unsafe { raise(SIGQUIT) }, 0);
        drop(held);
        assert_eq!(current(SIGINT), before, "restored");
    }

    #[test]
    fn ignored_signal_stays_ignored() {
        let _guard = lock();
        // SAFETY: sets SIGQUIT to ignored, restored at the end.
        let original = unsafe { signal(SIGQUIT, SIG_IGN) };
        let held = hold();
        assert_eq!(current(SIGQUIT), SIG_IGN, "no handler installed");
        drop(held);
        assert_eq!(current(SIGQUIT), SIG_IGN);
        // SAFETY: restores what the test found.
        unsafe { signal(SIGQUIT, original) };
    }
}
