use super::*;

#[test]
fn restore_by_batch() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.cache/b/y", b"B");
    let env = t.env();
    let rep = run(
        &env,
        vec![trash("/home/user/.config/a"), trash("/home/user/.cache/b")],
    );
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|o| o.ok), "{out:?}");
    assert_eq!(out[0].entry_id, rep.results[0].history_id);
    assert_eq!(out[0].path, Path::new("/home/user/.config/a"));
    assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
    assert_eq!(fs::read(env.sys("/home/user/.cache/b/y")).unwrap(), b"B");
    let logged = restore_entries(&env);
    assert_eq!(logged.len(), 2);
    assert!(logged.iter().all(|e| e.ok));
    assert_eq!(
        logged[0].restores.as_deref(),
        Some(rep.results[0].history_id.as_str())
    );
    assert_ne!(logged[0].batch, rep.batch);
}

/// Each Restore entry is appended right after its action, on its own, so
/// a failing append costs only that entry.
#[test]
fn restore_appends_per_entry() {
    use std::cell::RefCell;
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.config/b/y", b"B");
    t.write("home/user/.config/c/z", b"C");
    let env = t.env();
    let rep = run(
        &env,
        vec![
            trash("/home/user/.config/a"),
            trash("/home/user/.config/b"),
            trash("/home/user/.config/c"),
        ],
    );
    let calls: RefCell<Vec<usize>> = RefCell::new(Vec::new());
    let append = |env: &Env, entries: &[HistoryEntry]| {
        let mut calls = calls.borrow_mut();
        calls.push(entries.len());
        if calls.len() == 2 {
            return Err(CoreError::Parse {
                what: "history entry".into(),
                message: "boom".into(),
            });
        }
        history::append(env, entries)
    };
    let out = restore_with(&env, &Config::default(), &sel(&rep.batch), false, &append).unwrap();
    assert_eq!(*calls.borrow(), [1, 1, 1]);
    assert!(out.iter().all(|o| o.ok), "{out:?}");
    assert_eq!(out[0].error, None, "{out:?}");
    assert!(
        out[1]
            .error
            .as_deref()
            .unwrap()
            .ends_with("; history not written"),
        "{out:?}"
    );
    assert_eq!(out[2].error, None, "{out:?}");
    // The restores themselves all happened, and the other two are logged.
    for d in ["a/x", "b/y", "c/z"] {
        assert!(env.sys(&format!("/home/user/.config/{d}")).exists());
    }
    let logged = restore_entries(&env);
    assert_eq!(logged.len(), 2);
    assert_eq!(
        logged[0].restores.as_deref(),
        Some(rep.results[0].history_id.as_str())
    );
    assert_eq!(
        logged[1].restores.as_deref(),
        Some(rep.results[2].history_id.as_str())
    );
}

#[test]
fn restore_by_path_newest() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/v", b"old");
    let env = t.env();
    let first = run(&env, vec![trash("/home/user/.config/foo")]);
    t.write("home/user/.config/foo/v", b"new");
    let second = run(&env, vec![trash("/home/user/.config/foo")]);
    assert_ne!(first.batch, second.batch);
    let out = restore(
        &env,
        &Config::default(),
        &sel("/home/user/.config/foo"),
        false,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(out[0].entry_id, second.results[0].history_id);
    assert_eq!(
        fs::read(env.sys("/home/user/.config/foo/v")).unwrap(),
        b"new"
    );
    let (hist, _) = read_history(&env).unwrap();
    let old = hist[0].trashed.as_ref().unwrap();
    assert!(env.sys_path(&old.files_path).exists());
}

/// Review Focus #4.
#[test]
fn restore_partial_when_occupied() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.config/b/y", b"B");
    let env = t.env();
    let rep = run(
        &env,
        vec![trash("/home/user/.config/a"), trash("/home/user/.config/b")],
    );
    t.write("home/user/.config/a/x", b"recreated");
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert!(!out[0].ok);
    assert!(
        out[0].error.as_deref().unwrap().contains("exists"),
        "{out:?}"
    );
    assert!(out[1].ok, "{out:?}");
    assert_eq!(
        fs::read(env.sys("/home/user/.config/a/x")).unwrap(),
        b"recreated"
    );
    assert_eq!(fs::read(env.sys("/home/user/.config/b/y")).unwrap(), b"B");
    let (hist, _) = read_history(&env).unwrap();
    let a = hist[0].trashed.as_ref().unwrap();
    assert_eq!(
        fs::read(env.sys_path(&a.files_path).join("x")).unwrap(),
        b"A"
    );
    assert!(env.sys_path(&a.info_path).exists());
    let logged: Vec<bool> = restore_entries(&env).iter().map(|e| e.ok).collect();
    assert_eq!(logged, [false, true]);
}

#[test]
fn restore_twice_is_noop_error() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let id = rep.results[0].history_id.clone();
    assert!(restore(&env, &Config::default(), &sel(&id), false).unwrap()[0].ok);
    let again = restore(&env, &Config::default(), &sel(&id), false).unwrap();
    assert_eq!(again.len(), 1);
    assert!(!again[0].ok);
    assert_eq!(again[0].error.as_deref(), Some("already restored"));
    assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
}

#[test]
fn restore_edit_file_keeps_newer_content() {
    let t = FixtureTree::new();
    let env = t.env();
    let original = b"text/plain=foo.desktop\na=b\n";
    let rep = edit_mime(&env, &t, original);
    let file = env.sys("/home/user/.config/mimeapps.list");
    fs::write(&file, b"a=b\nnewer=de.desktop\n").unwrap();
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fs::read(&file).unwrap(), original);
    let logged = restore_entries(&env);
    let backup = logged[0].backup.clone().unwrap();
    assert_eq!(
        backup,
        Path::new(&format!(
            "/home/user/.config/mimeapps.list.crumbs-bak-{}",
            logged[0].batch
        ))
    );
    assert_eq!(
        fs::read(env.sys_path(&backup)).unwrap(),
        b"a=b\nnewer=de.desktop\n"
    );
}

#[test]
fn restore_edit_file_refuses_symlink() {
    let t = FixtureTree::new();
    let env = t.env();
    let rep = edit_mime(&env, &t, b"text/plain=foo.desktop\na=b\n");
    let file = env.sys("/home/user/.config/mimeapps.list");
    fs::remove_file(&file).unwrap();
    t.write("home/user/other.list", b"other");
    t.symlink("home/user/.config/mimeapps.list", "../other.list");
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some("refusing: ~/.config/mimeapps.list is not a regular file")
    );
    assert!(
        fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(env.sys("/home/user/other.list")).unwrap(),
        b"other"
    );
}

#[test]
fn restore_respects_user_protect() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let cfg = Config {
        protect: vec!["/home/user/.config/a".into()],
        ..Config::default()
    };
    let out = restore(&env, &cfg, &sel(&rep.batch), false).unwrap();
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some(INVALID_ENTRY));
    assert!(!env.sys("/home/user/.config/a").exists());
}

#[test]
fn restore_edit_file_from_backup() {
    let t = FixtureTree::new();
    let original = b"[Default Applications]\ntext/plain=foo.desktop\na=b\n";
    t.write("home/user/.config/mimeapps.list", original);
    let env = t.env();
    let rep = run(
        &env,
        vec![Action::EditFile {
            path: "/home/user/.config/mimeapps.list".into(),
            remove_lines: vec!["text/plain=foo.desktop".into()],
        }],
    );
    let file = env.sys("/home/user/.config/mimeapps.list");
    assert_ne!(fs::read(&file).unwrap(), original);
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fs::read(&file).unwrap(), original);
}

#[test]
fn restore_unknown_selector() {
    let t = FixtureTree::new();
    let env = t.env();
    let out = restore(&env, &Config::default(), &sel("12345-0"), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some("no matching history entry"));
    assert!(read_history(&env).unwrap().0.is_empty());
}

#[test]
fn restore_dry_run_changes_nothing() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let out = restore(&env, &Config::default(), &sel(&rep.batch), true).unwrap();
    assert!(out[0].ok, "{out:?}");
    assert!(!env.sys("/home/user/.config/a").exists());
    assert!(restore_entries(&env).is_empty());
}
