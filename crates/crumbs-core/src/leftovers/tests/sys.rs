use super::*;

// ---- system leftovers (/etc, /opt) ----

mod system {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::env::Env;
    use crate::leftovers::find_system_leftovers;
    use crate::ownership::OwnedIndex;
    use crate::safety::Scope;

    /// A fake root whose `/`, `/etc` and `/opt` pass the system-tree chain
    /// check, with this user standing in for root.
    fn sys_env(t: &FixtureTree) -> Env {
        t.mkdir("etc");
        t.mkdir("opt");
        for rel in ["", "etc", "opt"] {
            fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = t.env();
        env.set_system_owner_uid(env.uid);
        env
    }

    fn owned(files: &[&str]) -> OwnedIndex {
        OwnedIndex::new(files.iter().map(PathBuf::from), Vec::<PathBuf>::new(), true)
    }

    struct Sys {
        env: Env,
        idx: LeftoverIndex,
        inv: Inventory,
    }

    fn sys(t: &FixtureTree, apps: Vec<App>, own: OwnedIndex, rules: &[Rule]) -> Sys {
        let env = sys_env(t);
        let mut i = inv(apps);
        i.owned = own;
        let idx = LeftoverIndex::build(&env, &Config::default(), rules, &i);
        Sys { env, idx, inv: i }
    }

    impl Sys {
        fn find(&self, i: usize, show_low: bool) -> Vec<Finding> {
            find_system_leftovers(
                &self.env,
                &Config::default(),
                &self.idx,
                &self.inv.apps[i],
                show_low,
            )
        }
    }

    fn sys_at<'a>(fs: &'a [Finding], path: &str) -> Option<&'a Finding> {
        fs.iter().find(|f| f.path == Path::new(path))
    }

    #[test]
    fn system_leftover_exact_name_is_medium_not_preselected() {
        let t = FixtureTree::new();
        t.write("opt/inky/bin", b"x");
        t.write("etc/inky/conf", b"x");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, false);
        assert_eq!(fs.len(), 2, "{fs:#?}");
        for (path, kind) in [
            ("/opt/inky", FindingKind::SystemData),
            ("/etc/inky", FindingKind::SystemConfig),
        ] {
            let f = sys_at(&fs, path).unwrap_or_else(|| panic!("{path}: {fs:#?}"));
            assert_eq!(f.kind, kind);
            assert_eq!(f.confidence, Confidence::Medium);
            assert!(!f.preselected);
            assert!(!f.shared);
            assert_eq!(f.scope, Scope::SystemTree);
            assert_eq!(f.size.files, 1);
            assert_eq!(f.fingerprint, fingerprint(&s.env, &f.path).unwrap());
        }
    }

    #[test]
    fn owned_system_dir_not_offered() {
        let t = FixtureTree::new();
        t.write("opt/foo/file", b"x");
        let s = sys(&t, vec![pacman("foo", &[])], owned(&["/opt/foo/file"]), &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn no_or_incomplete_package_database_offers_nothing() {
        let t = FixtureTree::new();
        t.mkdir("opt/inky");
        let s = sys(&t, vec![pacman("inky", &[])], OwnedIndex::default(), &[]);
        assert!(s.find(0, true).is_empty());
        let partial = OwnedIndex::new(
            [PathBuf::from("/usr/bin/true")],
            Vec::<PathBuf>::new(),
            false,
        );
        let s = sys(&t, vec![pacman("inky", &[])], partial, &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn normalised_match_is_low_and_hidden() {
        let t = FixtureTree::new();
        t.mkdir("opt/Inky_App");
        let s = sys(
            &t,
            vec![pacman("inky-app", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, false).is_empty());
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(fs[0].confidence, Confidence::Low);
        assert!(!fs[0].preselected);
    }

    #[test]
    fn substring_names_are_not_matched() {
        let t = FixtureTree::new();
        t.mkdir("opt/Inky-App");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn etc_never_names_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("etc/ssh");
        let s = sys(&t, vec![pacman("ssh", &[])], owned(&["/usr/bin/true"]), &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn system_leftover_of_installed_app_is_shared_and_not_preselected() {
        let t = FixtureTree::new();
        t.mkdir("etc/brave");
        let s = sys(
            &t,
            vec![
                pacman("brave", &["brave"]),
                pacman("brave-beta", &["brave"]),
            ],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert!(fs[0].shared);
        assert_eq!(fs[0].confidence, Confidence::Low);
        assert!(!fs[0].preselected);
        assert!(fs[0].reason.contains("shared with"), "{}", fs[0].reason);
        assert!(s.find(0, false).is_empty());
    }

    #[test]
    fn unreadable_system_leftover_listed_with_unknown_size() {
        if crate::testing::running_as_root() {
            eprintln!("skipped: root can read a 0o000 directory");
            return;
        }
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let t = FixtureTree::new();
        t.write("etc/proton/a", b"x");
        let dir = t.dir.path().join("etc/proton");
        let _guard = Restore(dir.clone());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let s = sys(
            &t,
            vec![pacman("proton", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, false);
        let f = sys_at(&fs, "/etc/proton").unwrap_or_else(|| panic!("{fs:#?}"));
        assert!(f.size.errors > 0, "{:?}", f.size);
    }

    #[test]
    fn symlink_entry_is_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("srv/real");
        t.symlink("opt/inky", "/srv/real");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn writable_opt_offers_nothing() {
        let t = FixtureTree::new();
        t.mkdir("opt/inky");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        fs::set_permissions(t.dir.path().join("opt"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn rule_may_name_etc_and_opt_children() {
        let t = FixtureTree::new();
        t.mkdir("etc/foo");
        t.mkdir("opt/other");
        t.write("etc/foo/bar", b"x");
        let rules = [user_rule(
            &["acme"],
            &["/etc/foo", "/etc/foo/bar", "/opt/other", "/usr/bin/x"],
        )];
        let s = sys(
            &t,
            vec![pacman("acme", &[])],
            owned(&["/usr/bin/true"]),
            &rules,
        );
        let fs = s.find(0, false);
        let paths: Vec<&Path> = fs.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/etc/foo"), Path::new("/opt/other")]);
        assert!(fs.iter().all(|f| f.confidence == Confidence::Medium));
        assert!(fs[0].reason.contains("rule"), "{}", fs[0].reason);
    }

    #[test]
    fn tmpfiles_declared_directory_is_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("etc/openvpn");
        t.write(
            "usr/lib/tmpfiles.d/openvpn.conf",
            b"d /etc/openvpn/client 0750 openvpn network -\n",
        );
        let s = sys(
            &t,
            vec![pacman("openvpn", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn exec_basename_alone_is_low() {
        let t = FixtureTree::new();
        t.mkdir("etc/wine");
        let s = sys(
            &t,
            vec![pacman("wine-staging", &["wine"])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, false).is_empty());
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(fs[0].confidence, Confidence::Low);
    }

    #[test]
    fn another_apps_low_claim_marks_shared() {
        let t = FixtureTree::new();
        t.mkdir("etc/brave");
        let s = sys(
            &t,
            vec![pacman("brave", &[]), pacman("Brave", &["x-brave"])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert!(fs[0].shared);
    }

    #[test]
    fn cost_does_not_grow_with_unclaimed_inventory() {
        let t = FixtureTree::new();
        for n in ["zeta", "yota", "xi"] {
            t.mkdir(&format!("etc/{n}"));
            t.mkdir(&format!("opt/{n}"));
        }
        let apps: Vec<App> = (0..2000)
            .map(|i| pacman(&format!("app{i:04}"), &[]))
            .collect();
        let s = sys(&t, apps, owned(&["/usr/bin/true"]), &[]);
        let start = std::time::Instant::now();
        for i in 0..s.inv.apps.len() {
            assert!(s.find(i, true).is_empty());
        }
        let took = start.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn app_name_sets_lists_exact_names_and_keys() {
        let mut app = pacman("inky-app", &["inky"]);
        app.desktop_ids = vec!["org.inky.Inky.desktop".into()];
        app.rdns_id = Some("org.inky.Inky".into());
        let (names, keys) = crate::leftovers::system::app_name_sets(&app);
        for n in ["inky-app", "org.inky.Inky", "inky"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        assert_eq!(names.len(), keys.len());
        assert!(keys.iter().any(|k| k.norm == "inkyapp"));
    }

    fn sys_orphans(s: &Sys) -> Vec<Finding> {
        crate::leftovers::find_system_orphans(&s.env, &Config::default(), &s.idx)
    }

    #[test]
    fn system_orphan_is_unowned_unclaimed_opt_dir_only() {
        let t = FixtureTree::new();
        t.write("opt/old-app/bin", b"x");
        t.write("etc/old-app/conf", b"x");
        t.write("opt/plain-file", b"x");
        t.write("opt/owned-dir/f", b"x");
        t.write("opt/inky/bin", b"x");
        t.write("opt/vendor/bin", b"x");
        let mut inky = pacman("inky", &[]);
        inky.program = Some(PathBuf::from("/opt/vendor/bin"));
        let s = sys(
            &t,
            vec![inky],
            owned(&["/usr/bin/true", "/opt/owned-dir/f"]),
            &[],
        );
        let fs = sys_orphans(&s);
        let paths: Vec<&Path> = fs.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/opt/old-app")], "{fs:#?}");
        let f = &fs[0];
        assert_eq!(f.kind, FindingKind::SystemData);
        assert_eq!(f.confidence, Confidence::Low);
        assert!(!f.preselected);
        assert_eq!(f.scope, Scope::SystemTree);
        assert_eq!(f.reason, "not owned by any package and matched by no app");
        assert_eq!(f.size.files, 1);
    }

    #[test]
    fn system_orphan_skips_opt_dirs_still_linked_or_on_path() {
        let t = FixtureTree::new();
        for d in [
            "sdk", "ruby-3.3", "gradle-8", "node22", "rel", "mine", "gone",
        ] {
            t.write(&format!("opt/{d}/bin/x"), b"x");
        }
        // A program linked into /usr/local/bin, through an absolute target.
        t.symlink("usr/local/bin/gcloud", "/opt/sdk/bin/x");
        t.symlink("usr/bin/gem", "/opt/ruby-3.3/bin/x");
        // A versioned directory that a link in /opt itself names.
        t.symlink("opt/gradle", "gradle-8");
        // A relative target with `..`.
        t.symlink("usr/local/sbin/rel", "../../../opt/rel/bin/x");
        // The user's own link.
        t.symlink("home/user/.local/bin/mine", "/opt/mine/bin/x");
        // A dangling link to something that isn't there is no use of /opt/gone.
        t.symlink("usr/local/bin/other", "/srv/nothing");
        let mut s = sys(&t, Vec::new(), owned(&["/usr/bin/true"]), &[]);
        s.env.path_dirs = vec![PathBuf::from("/opt/node22/bin"), PathBuf::from("/usr/bin")];
        let paths: Vec<PathBuf> = sys_orphans(&s).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec![PathBuf::from("/opt/gone")]);
    }

    #[test]
    fn system_orphan_skips_weak_claims_and_tmpfiles_and_incomplete_index() {
        let t = FixtureTree::new();
        t.write("opt/weak/bin", b"x");
        t.write("opt/runtime/bin", b"x");
        t.write("opt/old-app/bin", b"x");
        t.write(
            "usr/lib/tmpfiles.d/x.conf",
            b"d /opt/runtime 0755 root root -\n",
        );
        // `weak` is only an Exec basename of the app.
        let s = sys(
            &t,
            vec![pacman("something", &["weak"])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let paths: Vec<_> = sys_orphans(&s).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, [PathBuf::from("/opt/old-app")]);
        let s = sys(&t, Vec::new(), OwnedIndex::default(), &[]);
        assert!(sys_orphans(&s).is_empty());
    }
}
