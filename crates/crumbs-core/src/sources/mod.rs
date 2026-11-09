pub mod appimage;
pub mod dpkg;
pub mod flatpak;
pub mod pacman;
pub mod rpm;
pub mod snap;
pub mod unmanaged;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::command::CommandRunner;
use crate::config::Config;
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
    pub config: &'a Config,
}

#[derive(Debug, Default)]
pub struct SourceListing {
    pub apps: Vec<App>,
    /// In-system absolute paths owned by any package of this source.
    pub owned_paths: HashSet<PathBuf>,
    /// In-system absolute directories owned by any package of this source.
    pub owned_dirs: HashSet<PathBuf>,
    pub claimed_desktop_ids: HashSet<String>,
    pub errors: Vec<SourceError>,
}

pub trait AppSource: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn available(&self, env: &Env) -> bool;
    fn is_package_db(&self) -> bool;
    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError>;
}

/// Links an app to the desktop entries among its owned `files` (under
/// `/usr/share/applications/`), fills its GUI fields from them and records
/// the claimed ids.
pub(crate) fn link_desktop(
    app: &mut App,
    files: &[PathBuf],
    by_id: &std::collections::HashMap<&str, &DesktopEntry>,
    claimed: &mut HashSet<String>,
) {
    let mut linked: Vec<&DesktopEntry> = Vec::new();
    for f in files {
        let Some(rel) = f
            .strip_prefix("/usr/share/applications/")
            .ok()
            .and_then(|r| r.to_str())
        else {
            continue;
        };
        if !rel.ends_with(".desktop") {
            continue;
        }
        // Nested directories map to dashes in desktop IDs.
        let id = rel.replace('/', "-");
        if let Some(entry) = by_id.get(id.as_str()) {
            linked.push(entry);
            claimed.insert(id);
        }
    }
    for e in &linked {
        app.desktop_ids.push(e.id.clone());
        app.exec_names.extend(crate::desktop::exec_basename(e));
        app.wm_classes.extend(e.wm_class.clone());
    }
    app.has_gui = linked
        .iter()
        .any(|e| e.is_application && !e.no_display && !e.hidden);
    if let Some(n) = linked.first().and_then(|e| e.name.clone()) {
        app.display_name = n;
    }
}
