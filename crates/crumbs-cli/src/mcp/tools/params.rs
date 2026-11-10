//! The tools' parameter types.

use super::*;

/// What a read tool computes: the `--json` data and its warnings.
pub(super) type ReadOutcome = Result<(Value, Vec<ErrorEntry>), Refusal>;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppsListParams {
    /// Only apps from these sources: pacman, flatpak, appimage, unmanaged.
    #[serde(default)]
    pub source: Vec<String>,
    /// Include apps without a graphical launcher.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppShowParams {
    /// App id, name, desktop id or display name.
    pub app: String,
    /// Include low-confidence leftovers.
    #[serde(default)]
    pub show_low: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanParams {
    /// Only these categories, even if switched off in the config.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Scan your own files (the default).
    #[serde(default)]
    pub user: bool,
    /// Scan the system categories (read-only).
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrphansParams {
    /// Unowned directories in /opt instead of per-user orphans.
    #[serde(default)]
    pub system: bool,
    /// Per-user orphans (the default).
    #[serde(default)]
    pub user: bool,
    /// Include low-confidence findings.
    #[serde(default)]
    pub show_low: bool,
}

pub(super) fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryParams {
    /// Number of batches, newest first.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewUninstallParams {
    /// App id, name, desktop id or display name (exact match).
    pub app: String,
    /// Keep the app itself; remove only its leftovers.
    #[serde(default)]
    pub leftovers_only: bool,
    /// Keep data and settings; remove only caches.
    #[serde(default)]
    pub keep_data: bool,
    /// Include low-confidence leftovers among the findings `select` can name.
    #[serde(default)]
    pub show_low: bool,
    /// Plan exactly the leftovers at these paths, each one crumbs found
    /// for this app (`app_show` lists them). System leftovers are planned
    /// only when named here. Without it, crumbs' defaults apply.
    #[serde(default)]
    pub select: Vec<String>,
    /// Allow removing a package that is part of the uQ desktop (`part_of`
    /// is set on the app). Only when the user asked for that package by
    /// name: without it, such a removal is refused.
    #[serde(default)]
    pub desktop: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewCleanParams {
    /// Only these categories, even if switched off in the config.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Your own files (the default).
    #[serde(default)]
    pub user: bool,
    /// The system categories (needs administrator rights to apply).
    #[serde(default)]
    pub system: bool,
    /// Plan exactly these items (paths, or category ids for a category's
    /// items), each one `scan` found. Without it, the defaults apply.
    #[serde(default)]
    pub select: Vec<String>,
    /// Leave out what matches these patterns, as `exclude` in the config
    /// does: each starts with `/` or `~/`; `*` matches within a path
    /// segment, `**` across segments.
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewOrphansParams {
    /// The orphans to remove: absolute paths `orphans` found.
    pub paths: Vec<String>,
    /// Unowned directories in /opt instead of per-user orphans.
    #[serde(default)]
    pub system: bool,
    /// Include low-confidence findings among the orphans `paths` can name.
    #[serde(default)]
    pub show_low: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyParams {
    /// The `plan_id` a preview returned.
    pub plan_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewRestoreParams {
    /// A history entry id, a batch id, or a path to put back.
    pub entry: String,
}
