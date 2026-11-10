use super::*;
use crate::model::SourceKind;

#[test]
fn absolute_home_rule_path_does_not_warn() {
    let t = FixtureTree::new();
    user_rule_file(
        &t,
        "home.toml",
        "[[app]]\nmatch = { package = [\"x\"] }\npaths = [\"/home/user/.foo\", \"/usr/bin/x\"]\n",
    );
    let (_, errors) = load_rules(&t.env());
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn invalid_absolute_rule_path_warns_but_keeps_rule() {
    let t = FixtureTree::new();
    user_rule_file(
        &t,
        "sys.toml",
        "[[app]]\nmatch = { package = [\"x\"] }\npaths = [\"/etc/x\", \"/etc/x/y\", \"/opt/a/b\", \"/usr/bin/x\", \"~/.x\"]\n",
    );
    let (rules, errors) = load_rules(&t.env());
    let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    for p in ["/etc/x/y", "/opt/a/b"] {
        assert!(
            msgs.iter().any(|m| m.contains(&format!(
                "rule path {p} ignored: only /etc/<name> or /opt/<name> are allowed outside home"
            ))),
            "{p}: {msgs:?}"
        );
    }
    assert!(rules.iter().any(|r| r.paths.len() == 5));
}

#[test]
fn system_rule_paths_are_etc_or_opt_children_only() {
    let p = |s: &str| expand_system_rule_path(s);
    assert_eq!(p("/etc/foo"), Some(PathBuf::from("/etc/foo")));
    assert_eq!(p("/opt/foo/"), Some(PathBuf::from("/opt/foo")));
    for bad in [
        "/etc/foo/bar",
        "/etc",
        "/etc/",
        "/opt/..",
        "/opt/foo/..",
        "/etc/foo*",
        "/etc/ssh",
        "/etc/foo.bak",
        "/usr/foo",
        "/etcfoo",
        "etc/foo",
        "~/etc/foo",
        "",
    ] {
        assert_eq!(p(bad), None, "{bad}");
    }
}
use crate::testing::FixtureTree;

fn user_rule_file(t: &FixtureTree, name: &str, body: &str) {
    t.write(
        &format!("/home/user/.config/crumbs/rules.d/{name}"),
        body.as_bytes(),
    );
}

#[test]
fn builtin_rules_parse() {
    let t = FixtureTree::new();
    let (rules, errors) = load_rules(&t.env());
    assert!(errors.is_empty(), "{errors:?}");
    assert!(rules.len() >= 20, "only {} rules", rules.len());
    assert!(rules.iter().all(|r| r.origin == RuleOrigin::Builtin));
    assert!(rules.iter().all(|r| !r.paths.is_empty()));
    assert!(
        rules
            .iter()
            .any(|r| r.packages.iter().any(|p| p == "brave-origin-bin"))
    );
    assert!(rules.iter().all(|r| r.flatpaks.is_empty()));
    assert!(
        rules
            .iter()
            .all(|r| !r.paths.iter().any(|p| p == "~/.android"))
    );
}

#[test]
fn user_rules_override_builtin() {
    let t = FixtureTree::new();
    user_rule_file(
        &t,
        "ff.toml",
        "[[app]]\nmatch = { package = [\"firefox\"] }\npaths = [\"~/.mine\"]\n",
    );
    let (rules, errors) = load_rules(&t.env());
    assert!(errors.is_empty(), "{errors:?}");
    let app = App::new(SourceKind::Pacman, "firefox");
    let hits = rules_for(&rules, &app);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].paths, ["~/.mine"]);
    assert!(matches!(hits[0].origin, RuleOrigin::User(_)));

    let other = App::new(SourceKind::Pacman, "thunderbird");
    let hits = rules_for(&rules, &other);
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|r| r.origin == RuleOrigin::Builtin));
}

#[test]
fn bad_user_rule_reported_and_skipped() {
    let t = FixtureTree::new();
    user_rule_file(&t, "a-bad.toml", "this is = not [ valid toml");
    user_rule_file(
        &t,
        "b-partial.toml",
        "[[app]]\nmatch = { package = [\"x-app\"] }\npaths = []\n\n\
             [[app]]\nmatch = {}\npaths = [\"~/.nomatch\"]\n\n\
             [[app]]\nmatch = { package = [\"good-app\"] }\npaths = [\"~/.good\"]\n",
    );
    let (rules, errors) = load_rules(&t.env());
    assert_eq!(errors.len(), 3, "{errors:?}");
    let user: Vec<_> = rules
        .iter()
        .filter(|r| matches!(r.origin, RuleOrigin::User(_)))
        .collect();
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].packages, ["good-app"]);
    assert!(rules.iter().any(|r| r.origin == RuleOrigin::Builtin));
}

#[test]
fn expand_rule_path_tokens() {
    let t = FixtureTree::new();
    let env = t.env();
    let p = |s: &str| expand_rule_path(&env, s);
    assert_eq!(p("~/.mozilla"), Some("/home/user/.mozilla".into()));
    assert_eq!(p("$HOME/.vscode"), Some("/home/user/.vscode".into()));
    assert_eq!(
        p("$XDG_CONFIG_HOME/Code"),
        Some("/home/user/.config/Code".into())
    );
    assert_eq!(p("$XDG_CACHE_HOME/x"), Some("/home/user/.cache/x".into()));
    assert_eq!(
        p("$XDG_DATA_HOME/Steam"),
        Some("/home/user/.local/share/Steam".into())
    );
    assert_eq!(
        p("$XDG_STATE_HOME/x"),
        Some("/home/user/.local/state/x".into())
    );
    assert_eq!(
        p("$XDG_CONFIG_HOME/Google/AndroidStudio*"),
        Some("/home/user/.config/Google/AndroidStudio*".into())
    );
}

#[test]
fn expand_rejects_outside_home_and_dotdot() {
    let t = FixtureTree::new();
    let env = t.env();
    for bad in [
        "/etc/foo",
        "/home/other/.x",
        "~/../other",
        "$XDG_CONFIG_HOME/../../x",
        "~",
        "$HOME",
        "relative/dir",
        "$UNKNOWN/x",
    ] {
        assert_eq!(expand_rule_path(&env, bad), None, "{bad}");
    }
}

fn rule(f: impl FnOnce(&mut Rule)) -> Rule {
    let mut r = Rule {
        packages: vec![],
        flatpaks: vec![],
        appimages: vec![],
        desktops: vec![],
        paths: vec!["~/.x".into()],
        origin: RuleOrigin::Builtin,
    };
    f(&mut r);
    r
}

#[test]
fn rule_matches_by_source() {
    let mut fp = App::new(SourceKind::Flatpak, "Firefox");
    fp.rdns_id = Some("org.mozilla.firefox".into());
    let r = rule(|r| r.flatpaks = vec!["org.mozilla.firefox".into()]);
    assert!(rule_matches(&r, &fp));
    let r_pkg = rule(|r| r.packages = vec!["Firefox".into()]);
    assert!(!rule_matches(&r_pkg, &fp));

    let ai = App::new(SourceKind::AppImage, "Cursor");
    let r = rule(|r| r.appimages = vec!["cursor".into()]);
    assert!(rule_matches(&r, &ai));
    let ai2 = App::new(SourceKind::AppImage, "My_Cursor");
    assert!(!rule_matches(&r, &ai2));
    let r_pkg = rule(|r| r.packages = vec!["Cursor".into()]);
    assert!(!rule_matches(&r_pkg, &ai));

    let pac = App::new(SourceKind::Pacman, "firefox");
    let r = rule(|r| r.packages = vec!["firefox".into()]);
    assert!(rule_matches(&r, &pac));
    let un = App::new(SourceKind::Unmanaged, "firefox");
    assert!(rule_matches(&r, &un));

    let mut d = App::new(SourceKind::Unmanaged, "zzz");
    d.desktop_ids = vec!["org.foo.Bar.desktop".into()];
    let r = rule(|r| r.desktops = vec!["org.foo.Bar".into()]);
    assert!(rule_matches(&r, &d));
    let none = rule(|_| {});
    assert!(!rule_matches(&none, &d));
}

#[test]
fn prefix_glob_expands() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/Google/AndroidStudio2025.1");
    t.mkdir("/home/user/.config/Google/AndroidStudio2025.2");
    t.mkdir("/home/user/.config/Google/Other");
    t.mkdir("/home/user/.android");
    let env = t.env();
    let r = rule(|r| {
        r.paths = vec![
            "~/.android".into(),
            "$XDG_CONFIG_HOME/Google/AndroidStudio*".into(),
            "~/.missing".into(),
        ]
    });
    let got = resolve_rule_paths(&env, &r);
    assert_eq!(
        got,
        [
            PathBuf::from("/home/user/.android"),
            PathBuf::from("/home/user/.config/Google/AndroidStudio2025.1"),
            PathBuf::from("/home/user/.config/Google/AndroidStudio2025.2"),
        ]
    );
}
