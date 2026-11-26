use super::*;

// ---- unmanaged apps ----

mod unmanaged {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config::Config;
    use crate::env::Env;
    use crate::inventory::Inventory;
    use crate::ownership::OwnedIndex;
    use crate::safety::Scope;

    fn sys_env(t: &FixtureTree) -> Env {
        t.mkdir("opt");
        for rel in ["", "opt"] {
            fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = t.env();
        env.set_system_owner_uid(env.uid);
        env
    }

    fn app(name: &str, program: &str) -> App {
        let mut a = App::new(SourceKind::Unmanaged, &format!("{name}.desktop"));
        a.name = name.into();
        a.display_name = name.into();
        a.desktop_ids.push(format!("{name}.desktop"));
        a.program = Some(PathBuf::from(program));
        a.exec_names.extend(
            Path::new(program)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
        );
        a
    }

    fn inv(apps: Vec<App>, owned: &[&str], complete: bool) -> Inventory {
        Inventory {
            apps,
            statuses: Vec::new(),
            errors: Vec::new(),
            owned: OwnedIndex::new(
                owned.iter().map(PathBuf::from),
                Vec::<PathBuf>::new(),
                complete,
            ),
        }
    }

    fn plan(env: &Env, i: &Inventory, a: &App) -> Result<UnmanagedTarget, RemovalError> {
        plan_unmanaged(env, &Config::default(), i, a)
    }

    fn reason(r: Result<UnmanagedTarget, RemovalError>) -> String {
        match r.unwrap_err() {
            RemovalError::Unsupported(m) => m,
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn unmanaged_opt_dir_planned_as_root_delete() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/opt/inky"));
        assert_eq!(tg.scope, Scope::SystemTree);
        assert_eq!(tg.left_link, None);
        assert_eq!(tg.size.files, 1);
    }

    #[test]
    fn unmanaged_vendor_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/google/chrome/chrome", b"");
        let env = sys_env(&t);
        let a = app("google-chrome-dev", "/opt/google/chrome/chrome");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let m = reason(plan(&env, &i, &a));
        assert_eq!(
            m,
            "removing this unmanaged app's files isn't possible: /opt/google does not match the app's name; use --leftovers-only"
        );
    }

    #[test]
    fn unmanaged_dir_shared_by_two_apps_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/suite/a", b"");
        t.write_exec("opt/suite/b", b"");
        let env = sys_env(&t);
        let a = app("suite", "/opt/suite/a");
        let b = app("other", "/opt/suite/b");
        let i = inv(vec![a.clone(), b], &["/usr/bin/true"], true);
        let m = reason(plan(&env, &i, &a));
        assert!(m.contains("/opt/suite is shared with other"), "{m}");
    }

    #[test]
    fn unmanaged_owned_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/opt/inky/data"], true);
        assert!(reason(plan(&env, &i, &a)).contains("/opt/inky belongs to a package"));
    }

    #[test]
    fn unmanaged_tmpfiles_declared_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.write("usr/lib/tmpfiles.d/inky.conf", b"d /opt/inky/run 0755\n");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &a)).contains("/opt/inky belongs to a package"));
    }

    #[test]
    fn unmanaged_opt_without_database_refused_but_home_allowed() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.write_exec("home/user/opt/foo/foo", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        for complete in [true, false] {
            let own: &[&str] = if complete { &[] } else { &["/usr/bin/true"] };
            let i = inv(vec![a.clone()], own, complete);
            let m = reason(plan(&env, &i, &a));
            assert!(
                m.contains("package ownership is unknown (no pacman database)"),
                "{m}"
            );
        }
        let h = app("foo", "/home/user/opt/foo/foo");
        let i = inv(vec![h.clone()], &[], false);
        assert!(plan(&env, &i, &h).is_ok());
    }

    #[test]
    fn unmanaged_home_dir_trashed() {
        let t = FixtureTree::new();
        t.write_exec("home/user/opt/foo/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/home/user/opt/foo/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/home/user/opt/foo"));
        assert_eq!(tg.scope, Scope::Home);
    }

    #[test]
    fn unmanaged_local_bin_file_trashed() {
        let t = FixtureTree::new();
        t.write_exec("home/user/.local/bin/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/home/user/.local/bin/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/home/user/.local/bin/foo"));
        assert_eq!(tg.scope, Scope::Home);
        let other = app("bar", "/home/user/.local/bin/foo");
        let i = inv(vec![other.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &other)).contains("does not match the app's name"));
    }

    #[test]
    fn unmanaged_usr_local_refused() {
        let t = FixtureTree::new();
        t.write_exec("usr/local/bin/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/usr/local/bin/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        assert_eq!(
            reason(plan(&env, &i, &a)),
            "removing this unmanaged app's files isn't possible: its program is in a protected or shared location; use --leftovers-only"
        );
    }

    #[test]
    fn unmanaged_symlinked_program_notes_link() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.symlink("usr/bin/inky", "/opt/inky/Inky");
        let env = sys_env(&t);
        let mut a = app("inky", "/opt/inky/Inky");
        a.program_link = Some("/usr/bin/inky".into());
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/opt/inky"));
        assert_eq!(tg.left_link.as_deref(), Some(Path::new("/usr/bin/inky")));
        assert!(tg.home_link.is_none());
    }

    #[test]
    fn unmanaged_name_check_ignores_exec_names() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.write_exec("home/user/.local/bin/foo", b"");
        let env = sys_env(&t);
        // A planted entry whose only link to /opt/inky is its Exec line.
        let mut evil = app("evil", "/opt/inky/Inky");
        evil.display_name = "Totally Unrelated".into();
        let i = inv(vec![evil.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &evil)).contains("does not match the app's name"));
        let bar = app("bar", "/home/user/.local/bin/foo");
        let i = inv(vec![bar.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &bar)).contains("does not match the app's name"));
        // The display name counts; the genuine case still works.
        let mut shown = app("evil", "/opt/inky/Inky");
        shown.display_name = "Inky".into();
        let i = inv(vec![shown.clone()], &["/usr/bin/true"], true);
        assert!(plan(&env, &i, &shown).is_ok());
        let inky = app("inky", "/opt/inky/Inky");
        let i = inv(vec![inky.clone()], &["/usr/bin/true"], true);
        assert!(plan(&env, &i, &inky).is_ok());
    }

    #[test]
    fn unmanaged_same_id_is_not_sharing_but_other_app_is() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone(), a.clone()], &["/usr/bin/true"], true);
        assert!(plan(&env, &i, &a).is_ok());
        let mut other = a.clone();
        other.id = "unmanaged:other.desktop".into();
        let i = inv(vec![a.clone(), other], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &a)).contains("is shared with"));
    }

    #[test]
    fn unmanaged_home_symlink_planned_for_removal() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.symlink("home/user/.local/bin/inky", "/opt/inky/Inky");
        let env = sys_env(&t);
        let mut a = app("inky", "/opt/inky/Inky");
        a.program_link = Some("/home/user/.local/bin/inky".into());
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        let (link, fp) = tg.home_link.unwrap();
        assert_eq!(link, Path::new("/home/user/.local/bin/inky"));
        assert!(fp.is_symlink);
        assert!(tg.left_link.is_none());
    }
}
