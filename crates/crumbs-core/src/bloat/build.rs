//! `build-output`: what builds leave in your projects and recreate on the
//! next build: Cargo's `target`, `node_modules` and the caches of web
//! bundlers, Gradle's and CMake's `build`, a project's `.gradle`, Python
//! virtual environments. Off unless named or switched on.
//!
//! Only inside git repositories under the project folders, and only a
//! folder that is the known kind (its project file beside it, or its own
//! marker) and that git ignores, so nothing tracked is ever offered. A
//! folder used recently, or any in a repository a process works in, is
//! listed but not preselected.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::command::resolve_trusted;
use crate::error::CoreError;
use crate::executor::{process_using, process_working_in};

/// Project folders looked in when `project_dirs` is not set (below home).
const DEFAULT_ROOTS: [&str; 8] = [
    "Projects",
    "projects",
    "src",
    "code",
    "dev",
    "git",
    "repos",
    "workspace",
];
/// Levels below a project folder searched for repositories.
const REPO_DEPTH: usize = 3;
/// Levels below a repository searched for build output.
const OUTPUT_DEPTH: usize = 4;
/// Directories looked at per project folder, so a huge tree stays cheap.
const DIR_LIMIT: usize = 50_000;

pub struct BuildOutputScanner;

impl Scanner for BuildOutputScanner {
    fn category(&self) -> Category {
        Category::BuildOutput
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        resolve_trusted(cx.env, "git")
            .is_none()
            .then(|| "git not installed".into())
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let Some(git) = resolve_trusted(cx.env, "git") else {
            return Ok(Vec::new());
        };
        let home = cx.env.unsys(&cx.env.home);
        let roots: Vec<PathBuf> = if cx.cfg.project_dirs.is_empty() {
            DEFAULT_ROOTS.iter().map(|r| home.join(r)).collect()
        } else {
            cx.cfg.project_dirs.clone()
        };
        let mut out = Vec::new();
        let mut seen_repos: Vec<PathBuf> = Vec::new();
        for root in roots {
            let mut queue = repositories(&cx.env.sys_path(&root));
            while let Some(repo) = queue.pop() {
                if seen_repos.contains(&repo) {
                    continue;
                }
                seen_repos.push(repo.clone());
                let (items, nested) = repo_items(cx, &git, &root, &repo);
                out.extend(items);
                // A repository inside this one answers for itself.
                queue.extend(nested);
            }
        }
        Ok(out)
    }
}

/// The git repositories (real paths) in the real folder `root` or up to
/// [`REPO_DEPTH`] levels below it; a repository's own subfolders are not
/// searched for more.
fn repositories(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut level = vec![root.to_path_buf()];
    let mut seen = 0usize;
    for depth in 0..=REPO_DEPTH {
        let mut next = Vec::new();
        for dir in level {
            seen += 1;
            if seen > DIR_LIMIT {
                return found;
            }
            if fs::symlink_metadata(dir.join(".git")).is_ok() {
                found.push(dir);
                continue;
            }
            if depth == REPO_DEPTH {
                continue;
            }
            next.extend(real_subdirs(&dir));
        }
        level = next;
    }
    found.sort();
    found
}

fn real_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Whether the real folder `dir` is build output of a known kind.
fn is_build_output(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let parent = dir.parent().unwrap_or(dir);
    let beside = |files: &[&str]| files.iter().any(|f| parent.join(f).is_file());
    match name {
        "target" => beside(&["Cargo.toml"]) || dir.join("CACHEDIR.TAG").is_file(),
        "node_modules" | ".next" | ".svelte-kit" | ".turbo" | ".parcel-cache" => {
            beside(&["package.json"])
        }
        "build" => beside(&["build.gradle", "build.gradle.kts", "CMakeLists.txt"]),
        ".gradle" => beside(&[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]),
        ".venv" | "venv" => dir.join("pyvenv.cfg").is_file(),
        _ => false,
    }
}

/// Names that are never searched below, whether or not they qualify.
fn is_output_name(name: &str) -> bool {
    matches!(
        name,
        "target"
            | "node_modules"
            | ".next"
            | ".svelte-kit"
            | ".turbo"
            | ".parcel-cache"
            | "build"
            | ".gradle"
            | ".venv"
            | "venv"
            | ".git"
    )
}

/// The build output in one repository (real path `repo`), found below its
/// project folder `root` (in-system), and the repositories nested in it
/// (a `.git` directory or file below it: a submodule, a worktree, a
/// clone), which are not searched here: their own git decides for them.
fn repo_items(
    cx: &ScanContext,
    git: &Path,
    root: &Path,
    repo: &Path,
) -> (Vec<BloatItem>, Vec<PathBuf>) {
    // Candidates, relative to the repository.
    let mut rels: Vec<String> = Vec::new();
    let mut nested: Vec<PathBuf> = Vec::new();
    let mut level = vec![repo.to_path_buf()];
    for depth in 0..OUTPUT_DEPTH {
        let mut next = Vec::new();
        for dir in level {
            for sub in real_subdirs(&dir) {
                let name = sub.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if name != ".git" && fs::symlink_metadata(sub.join(".git")).is_ok() {
                    nested.push(sub);
                    continue;
                }
                if is_output_name(name) {
                    if name != ".git"
                        && is_build_output(&sub)
                        && let Ok(rel) = sub.strip_prefix(repo)
                        && let Some(rel) = rel.to_str()
                    {
                        rels.push(rel.to_string());
                    }
                } else if depth + 1 < OUTPUT_DEPTH {
                    next.push(sub);
                }
            }
        }
        level = next;
    }
    if rels.is_empty() {
        return (Vec::new(), nested);
    }
    rels.sort();
    let ignored = ignored_by_git(cx, git, repo, &rels);
    let in_system_repo = cx.env.unsys(repo);
    let repo_label = in_system_repo
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| repo.file_name().and_then(|n| n.to_str()).unwrap_or("."))
        .to_string();
    let busy = process_working_in(cx.env, &in_system_repo);
    let mut out = Vec::new();
    for rel in rels.iter().filter(|r| ignored.contains(r)) {
        let path = in_system_repo.join(rel);
        let Some(mut item) = new_item(
            cx,
            Category::BuildOutput,
            path,
            format!("{repo_label}/{rel}"),
            true,
        ) else {
            continue;
        };
        let days = last_changed_days(&repo.join(rel), cx.now);
        let changed = match days {
            0 => "last changed today".to_string(),
            1 => "last changed 1 day ago".to_string(),
            d => format!("last changed {d} days ago"),
        };
        if let Some((pid, comm)) = &busy {
            item.preselected = false;
            item.note = Some(format!("{repo_label} is in use by {comm} (pid {pid})"));
        } else if let Some((pid, comm)) = process_using(cx.env, &in_system_repo.join(rel)) {
            item.preselected = false;
            item.note = Some(format!("in use by {comm} (pid {pid})"));
        } else if is_venv(rel) {
            // Something outside the project may run from it, and it may
            // have no requirements file to be rebuilt from.
            item.preselected = false;
            item.rebuildable = false;
            item.note = Some(format!(
                "a virtual environment can be in use from outside the project; {changed}"
            ));
        } else {
            item.preselected = days >= cx.cfg.build_age_days;
            item.note = Some(changed);
        }
        out.push(item);
    }
    (out, nested)
}

/// The candidates git ignores: those `git ls-files --others --ignored
/// --exclude-standard --directory` lists, or that lie in a folder it lists
/// (git lists a folder whole only when nothing in it is tracked, so a
/// build folder holding a tracked file never matches). None when git fails
/// (not a repository, or any other error).
///
/// A repository's own config can name commands git runs (`core.fsmonitor`),
/// and a scan must stay read-only: those are switched off on the command
/// line, which overrides the repository's config. `-z` gives every path
/// exactly, unquoted.
pub(super) fn ignored_by_git(
    cx: &ScanContext,
    git: &Path,
    repo: &Path,
    rels: &[String],
) -> Vec<String> {
    let repo_arg = repo.to_string_lossy();
    let args = [
        "-C",
        &repo_arg,
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "-z",
    ];
    let listed: Vec<String> = match cx.runner.run(git, &args) {
        Ok(o) if o.status == 0 && !o.timed_out => o
            .stdout
            .split('\0')
            .filter(|e| e.ends_with('/'))
            .map(str::to_string)
            .collect(),
        _ => return Vec::new(),
    };
    rels.iter()
        .filter(|rel| {
            let dir = format!("{rel}/");
            listed.iter().any(|e| dir.starts_with(e.as_str()))
        })
        .cloned()
        .collect()
}

fn is_venv(rel: &str) -> bool {
    Path::new(rel)
        .file_name()
        .is_some_and(|n| n == ".venv" || n == "venv")
}

/// Whole days since the folder (or anything directly in it) last changed.
/// Running what it holds changes nothing, so this is not when it was last
/// used; [`process_using`] covers what runs from it now.
fn last_changed_days(dir: &Path, now: SystemTime) -> u64 {
    let mtime = |p: &Path| fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
    let mut newest = mtime(dir);
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(t) = mtime(&e.path()) {
                newest = Some(newest.map_or(t, |n| n.max(t)));
            }
        }
    }
    newest
        .and_then(|t| now.duration_since(t).ok())
        .map_or(0, |d| d.as_secs() / 86_400)
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
