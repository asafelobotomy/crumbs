//! System orphans: unowned directories in `/opt` that no detected app claims or still uses.

use super::*;

/// Unowned real directories directly in `/opt` that no detected app claims
/// (by any name, weak or strong, or a rule path) and that are not the
/// program directory or `location` of one. Never looks in `/etc`. Always
/// `Low`, never pre-selected, root-deleted (`Scope::SystemTree`) and
/// verified now. Empty unless the package database was read completely
/// and `tmpfiles.d` is readable. Sorted by path.
pub fn find_system_orphans(env: &Env, cfg: &Config, idx: &LeftoverIndex) -> Vec<Finding> {
    let inv = idx.inventory();
    let owned = &inv.owned;
    if owned.is_empty() || !owned.is_complete() {
        return Vec::new();
    }
    let Ok(declared) = tmpfiles_declared(env) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(env.sys("/opt")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let in_use = opt_dirs_in_use(env);
    let mut claimants: Option<Vec<Claimant>> = None;
    let mut out = Vec::new();
    for name in names {
        let path = Path::new("/opt").join(&name);
        let is_real_dir = std::fs::symlink_metadata(env.sys_path(&path)).is_ok_and(|m| m.is_dir());
        if !is_real_dir
            || !safety::is_system_leftover_name(std::ffi::OsStr::new(&name))
            || in_use.contains(&name)
            || owned.owns_at_or_below(&path)
            || declared.covers(&path)
            || inv.apps.iter().any(|a| {
                [&a.program, &a.location]
                    .into_iter()
                    .flatten()
                    .any(|p| p.starts_with(&path))
            })
        {
            continue;
        }
        let claimants = claimants
            .get_or_insert_with(|| inv.apps.iter().map(|a| Claimant::of(idx, a)).collect());
        if claimants.iter().any(|c| c.claim(&path, &name).is_some()) {
            continue;
        }
        let Ok(fingerprint) = safety::verify_now(env, cfg, Scope::SystemTree, &path) else {
            continue;
        };
        out.push(Finding {
            size: disk_usage(env, &path),
            path,
            kind: FindingKind::SystemData,
            confidence: Confidence::Low,
            shared: false,
            reason: "not owned by any package and matched by no app".into(),
            preselected: false,
            scope: Scope::SystemTree,
            fingerprint,
            mime_lines: Vec::new(),
        });
    }
    out
}

/// Where a program installed by hand into `/opt` is usually linked from.
const LINK_DIRS: [&str; 6] = [
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/bin",
    "/sbin",
];

/// The names of the `/opt` directories something still points into, which
/// are in use even though no package owns them and no app was detected: the
/// target of a symlink in `/opt` itself (`/opt/gradle -> gradle-8.14`), in a
/// program directory (`/usr/local/bin/gcloud -> /opt/google-cloud-sdk/...`)
/// or in `~/.local/bin` and `~/bin`, every directory on the user's `PATH`
/// (`/opt/node22/bin`), and the program of every running process this user
/// can see.
fn opt_dirs_in_use(env: &Env) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let mut note = |p: &Path| {
        if let Ok(rest) = p.strip_prefix("/opt")
            && let Some(std::path::Component::Normal(n)) = rest.components().next()
            && let Some(n) = n.to_str()
        {
            names.insert(n.to_string());
        }
    };
    for dir in &env.path_dirs {
        if let Some(p) = normalize(dir) {
            note(&p);
        }
    }
    if let Ok(rd) = std::fs::read_dir(env.sys("/proc")) {
        for e in rd.flatten() {
            if let Ok(exe) = std::fs::read_link(e.path().join("exe")) {
                note(&env.unsys(&exe));
            }
        }
    }
    let home = env.unsys(&env.home);
    let dirs = LINK_DIRS.iter().map(PathBuf::from).chain([
        PathBuf::from("/opt"),
        home.join(".local/bin"),
        home.join("bin"),
    ]);
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(env.sys_path(&dir)) else {
            continue;
        };
        for e in rd.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_symlink()) {
                continue;
            }
            let mut link = dir.join(e.file_name());
            // Follow a chain of links (`/usr/local/bin/x -> /opt/x/bin/x`,
            // where `/opt/x` is itself a link), noting each hop under /opt.
            for _ in 0..8 {
                let Ok(target) = std::fs::read_link(env.sys_path(&link)) else {
                    break;
                };
                let parent = link.parent().unwrap_or(Path::new("/"));
                let Some(next) = normalize(&parent.join(target)) else {
                    break;
                };
                note(&next);
                link = next;
            }
        }
    }
    names
}

/// `p` made absolute-lexical with `..` applied (a symlink's relative target
/// joined to its directory); `None` for a relative path.
fn normalize(p: &Path) -> Option<PathBuf> {
    if !p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            std::path::Component::Normal(n) => out.push(n),
            std::path::Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    Some(out)
}
