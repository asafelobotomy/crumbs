//! Warnings as MCP log messages (`notifications/message`, level
//! `warning`): what the CLI prints as `warning: …` on stderr. Under `crumbs
//! mcp` stderr stays quiet and stdout carries only the protocol.

// MCP logging is the spec's channel for these warnings (spec §4); rmcp
// marks it deprecated by SEP-2577, which has no replacement yet.
#![allow(
    deprecated,
    reason = "MCP logging is how spec section 4 sends warnings"
)]

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};
use rmcp::{Peer, RoleServer};

use crate::cmd::context::note_line;
use crate::output::ErrorEntry;

/// The `logger` of every message.
const LOGGER: &str = "crumbs";

/// Where the server's warnings go. Clones share the level and the
/// pending warnings.
#[derive(Clone, Default)]
pub(crate) struct Log {
    /// The client's `logging/setLevel`, as [`rank`]; until it sets one,
    /// every message goes out.
    min_rank: Arc<AtomicU8>,
    /// Warnings from before a client could hear them (server start).
    pending: Arc<Mutex<Vec<String>>>,
}

/// Severity order, lowest first.
fn rank(level: LoggingLevel) -> u8 {
    match level {
        LoggingLevel::Debug => 0,
        LoggingLevel::Info => 1,
        LoggingLevel::Notice => 2,
        LoggingLevel::Warning => 3,
        LoggingLevel::Error => 4,
        LoggingLevel::Critical => 5,
        LoggingLevel::Alert => 6,
        LoggingLevel::Emergency => 7,
    }
}

impl Log {
    /// A sink that sends `pending` (already sanitised) with the first
    /// warnings it sends.
    pub(crate) fn new(pending: Vec<String>) -> Log {
        Log {
            min_rank: Arc::default(),
            pending: Arc::new(Mutex::new(pending)),
        }
    }

    pub(crate) fn set_level(&self, level: LoggingLevel) {
        self.min_rank.store(rank(level), Ordering::Relaxed);
    }

    /// Sends the warnings held since the start, once.
    pub(crate) async fn flush(&self, peer: &Peer<RoleServer>) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        self.warn_lines(peer, pending).await;
    }

    /// Sends each note as a warning, as the CLI words it.
    pub(crate) async fn warn(&self, peer: &Peer<RoleServer>, notes: &[ErrorEntry]) {
        self.flush(peer).await;
        self.warn_lines(peer, notes.iter().map(note_line)).await;
    }

    /// Sends sanitised lines as warnings. A client that went away gets
    /// none; that is not an error of the tool call.
    async fn warn_lines(&self, peer: &Peer<RoleServer>, lines: impl IntoIterator<Item = String>) {
        if rank(LoggingLevel::Warning) < self.min_rank.load(Ordering::Relaxed) {
            return;
        }
        for line in lines {
            let msg = LoggingMessageNotificationParam::new(LoggingLevel::Warning, line.into())
                .with_logger(LOGGER);
            let _ = peer.notify_logging_message(msg).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_rank_in_spec_order() {
        let all = [
            LoggingLevel::Debug,
            LoggingLevel::Info,
            LoggingLevel::Notice,
            LoggingLevel::Warning,
            LoggingLevel::Error,
            LoggingLevel::Critical,
            LoggingLevel::Alert,
            LoggingLevel::Emergency,
        ];
        let ranks: Vec<u8> = all.into_iter().map(rank).collect();
        assert_eq!(ranks, (0..8).collect::<Vec<u8>>());
    }
}
