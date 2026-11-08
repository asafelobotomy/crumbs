pub mod appimage;
pub mod flatpak;
pub mod pacman;
pub mod unmanaged;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::command::CommandRunner;
use crate::desktop::DesktopEntry;
use crate::env::Env;
use crate::error::{CoreError, SourceError};
use crate::model::{App, SourceKind};

#[derive(Debug, Clone, Copy, Default)]
pub struct ListOptions {
    pub include_all: bool,
}

pub struct SourceContext<'a> {
    pub env: &'a Env,
    pub runner: &'a dyn CommandRunner,
    pub desktop: &'a [DesktopEntry],
    pub opts: &'a ListOptions,
}

#[derive(Debug, Default)]
pub struct SourceListing {
    pub apps: Vec<App>,
    /// In-system absolute paths owned by any package of this source.
    pub owned_paths: HashSet<PathBuf>,
    pub claimed_desktop_ids: HashSet<String>,
    pub errors: Vec<SourceError>,
}

pub trait AppSource: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn available(&self, env: &Env) -> bool;
    fn is_package_db(&self) -> bool;
    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError>;
}
