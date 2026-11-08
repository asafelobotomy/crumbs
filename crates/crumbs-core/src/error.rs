use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("I/O error on {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("refusing to use {} as a fake root: not under the temp dir", .0.display())]
    UnsafeRoot(PathBuf),
    #[error("command {program} failed: {message}")]
    Command { program: String, message: String },
    #[error("failed to parse {what}: {message}")]
    Parse { what: String, message: String },
}

/// A failure from one app source, reported alongside partial results.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct SourceError {
    pub source: crate::model::SourceKind,
    pub message: String,
}
