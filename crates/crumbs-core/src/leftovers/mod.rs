//! Leftovers: per-user files an app leaves behind, and how sure we are.

pub(crate) mod candidates;
pub mod matcher;
mod orphans;
pub(crate) mod score;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use serde::Serialize;

use crate::safety::Fingerprint;
use crate::size::DiskUsage;

pub use matcher::{LeftoverIndex, find_leftovers};
pub use orphans::find_orphans;

/// How sure the matcher is that a finding belongs to the app. Ordered
/// `Low < Medium < High`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    /// One level lower; `Low` stays `Low`.
    pub fn downgraded(self) -> Confidence {
        match self {
            Confidence::High => Confidence::Medium,
            Confidence::Medium | Confidence::Low => Confidence::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    Config,
    Cache,
    Data,
    State,
    /// A directory directly in home whose name starts with `.`.
    DotDir,
    /// A file (or symlink) directly in home whose name starts with `.`.
    DotFile,
    FlatpakData,
    SnapData,
    Autostart,
    DesktopEntry,
    Icon,
    SystemdUnit,
    MimeAssoc,
}

/// One leftover of an app. `path` is in-system and canonical.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub path: PathBuf,
    pub kind: FindingKind,
    pub confidence: Confidence,
    pub shared: bool,
    pub reason: String,
    pub size: DiskUsage,
    pub preselected: bool,
    /// Taken when the index was built; `safety::verify` checks against it.
    #[serde(skip)]
    pub fingerprint: Fingerprint,
    /// `MimeAssoc` only: the exact `mimeapps.list` lines to remove.
    pub mime_lines: Vec<String>,
}
