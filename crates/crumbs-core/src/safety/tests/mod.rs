use super::*;
use crate::testing::FixtureTree;

fn setup() -> (FixtureTree, Env, Config) {
    let t = FixtureTree::new();
    let env = t.env();
    (t, env, Config::default())
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn mkfifo(t: &FixtureTree, rel: &str) {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    mknodat(
        CWD,
        t.dir.path().join(rel),
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
}

/// A fixture `/var/lib/systemd/coredump` with every directory from the
/// root down set to 0755, so the root-owned chain check passes (the
/// fixture user stands in for root, see `Env::system_owner_uid`).
fn coredump_dir(t: &FixtureTree) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    for c in ["var", "lib", "systemd", "coredump"] {
        cur.push(c);
        fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// `/`, `/etc` and `/opt` as 0755 directories, so the root-owned chain
/// check passes (the fixture user stands in for root).
fn system_tree_env(t: &FixtureTree) -> Env {
    use std::os::unix::fs::PermissionsExt;
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = t.env();
    env.set_system_owner_uid(env.uid);
    env
}

mod dotdot_verify;
mod home_ssh;
mod system_user;
