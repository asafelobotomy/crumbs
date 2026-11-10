#[test]
fn exclude_pattern_expands_and_normalises() {
    let t = crate::testing::FixtureTree::new();
    let env = t.env();
    assert_eq!(
        exclude_pattern(&env, "~/.cache/../.cache/x*").as_deref(),
        Some("/home/user/.cache/x*")
    );
    assert_eq!(exclude_pattern(&env, "/srv/./a").as_deref(), Some("/srv/a"));
    assert_eq!(exclude_pattern(&env, "relative"), None);
}

use super::*;
use crate::testing::FixtureTree;

#[test]
fn missing_config_is_default() {
    let t = FixtureTree::new();
    let c = load_config(&t.env()).unwrap();
    assert!(c.protect.is_empty() && c.exclude.is_empty() && c.appimage_dirs.is_empty());
}

#[test]
fn parses_keys_and_expands_tilde() {
    let t = FixtureTree::new();
    t.write(
            "/home/user/.config/crumbs/config.toml",
            b"protect = [\"~/Documents\", \"/srv/keep\", \"relative/dir\"]\nexclude = [\"~/.cache/**\", \"/tmp/*.log\"]\nappimage_dirs = [\"~/Tools\", \"/opt/appimages\"]\n",
        );
    let c = load_config(&t.env()).unwrap();
    assert_eq!(
        c.protect,
        [
            PathBuf::from("/home/user/Documents"),
            PathBuf::from("/srv/keep")
        ]
    );
    assert_eq!(c.exclude, ["/home/user/.cache/**", "/tmp/*.log"]);
    assert_eq!(
        c.appimage_dirs,
        [
            PathBuf::from("/home/user/Tools"),
            PathBuf::from("/opt/appimages")
        ]
    );
}

#[test]
fn invalid_entries_warn() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"protect = [\"foo/bar\", \"/ok\"]\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!(c.protect, [PathBuf::from("/ok")]);
    assert_eq!(c.warnings.len(), 1);
    assert!(c.warnings[0].contains("foo/bar"), "{:?}", c.warnings);
}

#[test]
fn unknown_keys_ignored() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"future_key = 3\nprotect = [\"/a/b\"]\n[section]\nx = 1\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!(c.protect, [PathBuf::from("/a/b")]);
}

#[test]
fn bad_toml_is_parse_error() {
    let t = FixtureTree::new();
    t.write("/home/user/.config/crumbs/config.toml", b"protect = [");
    match load_config(&t.env()) {
        Err(CoreError::Parse { what, .. }) => assert_eq!(what, "config.toml"),
        other => panic!("expected parse error, got {other:?}"),
    }
}

#[test]
fn glob_match_table() {
    let cases = [
        ("/home/u/.cache/**", "/home/u/.cache/a/b/c", true),
        ("/home/u/.cache/**", "/home/u/.cache/a", true),
        ("/home/u/.cache/**", "/home/u/.config/a", false),
        ("/home/u/*.log", "/home/u/x.log", true),
        ("/home/u/*.log", "/home/u/sub/x.log", false),
        ("/home/*/x", "/home/u/x", true),
        ("/home/*/x", "/home/u/v/x", false),
        ("/home/**/x", "/home/u/v/x", true),
        ("/home/**/x", "/home/x", true), // `**` may match zero segments
        ("/a/b", "/a/b", true),
        ("/a/b", "/a/b/c/d", true),
        ("/a/b", "/a/bc", false),
        ("/a/b", "/a", false),
        ("/a/b/", "/a/b", true),
        ("/a/b/", "/a/b/c", true),
        ("/a/b.c", "/a/bxc", false),
    ];
    for (pat, path, want) in cases {
        assert_eq!(glob_match(pat, path), want, "{pat} vs {path}");
    }
}

#[test]
fn glob_double_star_matches_dir_itself() {
    assert!(glob_match("/a/.cache/**", "/a/.cache"));
    assert!(glob_match("/a/**/z", "/a/z"));
    assert!(glob_match("/a/**/z", "/a/b/c/z"));
    assert!(!glob_match("/a/**/z", "/a/b/c"));
}

#[test]
fn glob_many_double_stars_is_fast() {
    let path = format!("/{}", vec!["seg"; 40].join("/"));
    let start = std::time::Instant::now();
    assert!(!glob_match("/**/**/**/**/**/**/x", &path));
    assert!(glob_match("/**/**/**/**/**/**/seg", &path));
    assert!(start.elapsed() < std::time::Duration::from_millis(50));
}

#[test]
fn glob_double_star_is_linear() {
    let pattern = format!("/{}/x", vec!["**"; 30].join("/"));
    let path = format!("/{}", "a/".repeat(100));
    assert_eq!(path.len(), 201);
    let start = std::time::Instant::now();
    assert!(!glob_match(&pattern, &path));
    assert!(start.elapsed() < std::time::Duration::from_millis(50));
}

#[test]
fn glob_dir_double_star_matches_dir() {
    assert!(glob_match("/h/.cache/**", "/h/.cache"));
    assert!(glob_match("/h/.cache/**", "/h/.cache/x/y"));
    assert!(!glob_match("/h/.cache/**", "/h/.cachex"));
}

#[test]
fn could_match_below_table() {
    let star = "/h/.config/*/keep";
    let cases = [
        (star, "/h/.config/Foobar", true),
        (star, "/h/.config", true),
        (star, "/h/.cache/x", false),
        ("/h/a/**/z", "/h/a/b/c", true),
        ("/h/a/b", "/h/a", true),
        ("/h/a/b", "/h/a/b", false),
        ("/h/a/b", "/h/x", false),
        ("/h/a/**", "/h/a", true),
    ];
    for (pat, dir, want) in cases {
        assert_eq!(could_match_below(pat, dir), want, "{pat} below {dir}");
    }
}

#[test]
fn config_normalises_dotdot() {
    let t = FixtureTree::new();
    t.write(
            "/home/user/.config/crumbs/config.toml",
            b"protect = [\"/a/b/../c\", \"/a/./d\", \"/../x\"]\nexclude = [\"~/.cache/../.config/*/keep\", \"/a/../../y\"]\nappimage_dirs = [\"/opt/./ai/\"]\n",
        );
    let c = load_config(&t.env()).unwrap();
    // `..` at `/` stays at `/`, as the kernel resolves it: the entry is
    // kept, never dropped.
    assert_eq!(
        c.protect,
        [
            PathBuf::from("/a/c"),
            PathBuf::from("/a/d"),
            PathBuf::from("/x")
        ]
    );
    assert_eq!(c.exclude, ["/home/user/.config/*/keep", "/y"]);
    assert_eq!(c.appimage_dirs, [PathBuf::from("/opt/ai")]);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
fn config_age_days_at_least_one() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"temp_age_days = 0\nlog_age_days = -3\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.temp_age_days, c.log_age_days), (1, 1));
    assert_eq!(
        c.warnings,
        [
            "config.toml: temp_age_days must be at least 1; using 1",
            "config.toml: log_age_days must be at least 1; using 1"
        ]
    );
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"temp_age_days = 1\nlog_age_days = 2\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.temp_age_days, c.log_age_days), (1, 2));
    assert!(c.warnings.is_empty());
}

#[test]
fn config_bloat_keys_and_unknown_category_warns() {
    let t = FixtureTree::new();
    let d = load_config(&t.env()).unwrap();
    assert_eq!((d.temp_age_days, d.log_age_days), (7, 14));
    assert_eq!(d.categories_off, [Category::DevCache]);
    assert!(d.categories_on.is_empty());
    assert!(d.project_dirs.is_empty());
    assert_eq!((d.build_age_days, d.snapshots_keep), (7, 3));

    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"temp_age_days = 3\nlog_age_days = 30\ncategories_off = [\"logs\", \"bogus\", \"temp\"]\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.temp_age_days, c.log_age_days), (3, 30));
    assert_eq!(c.categories_off, [Category::Logs, Category::Temp]);
    assert_eq!(
        c.warnings,
        ["config.toml: unknown category 'bogus' in categories_off"]
    );

    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"categories_off = []\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert!(c.categories_off.is_empty());
    assert!(c.warnings.is_empty());

    // The build-output and snapshot keys.
    let t = crate::testing::FixtureTree::new();
    t.write(
        "home/user/.config/crumbs/config.toml",
        b"project_dirs = [\"~/work\", \"rel\"]\nbuild_age_days = 30\nsnapshots_keep = 0\n",
    );
    let c = load_config(&t.env()).unwrap();
    assert_eq!(c.project_dirs, [PathBuf::from("/home/user/work")]);
    assert_eq!(c.build_age_days, 30);
    assert_eq!(c.snapshots_keep, 1);
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("project_dirs entry 'rel'")),
        "{:?}",
        c.warnings
    );
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("snapshots_keep must be at least 1")),
        "{:?}",
        c.warnings
    );

    // A keep count beyond u32 keeps everything rather than wrapping.
    let tk = crate::testing::FixtureTree::new();
    tk.write(
        "home/user/.config/crumbs/config.toml",
        b"snapshots_keep = 4294967296\n",
    );
    assert_eq!(load_config(&tk.env()).unwrap().snapshots_keep, u32::MAX);

    // build-output runs unnamed only when categories_on lists it: an
    // older config's categories_off cannot switch it on.
    let t = crate::testing::FixtureTree::new();
    let cfg_at = |text: &[u8]| {
        t.write("home/user/.config/crumbs/config.toml", text);
        load_config(&t.env()).unwrap()
    };
    let d = Config::default();
    assert!(!d.enabled(Category::BuildOutput) && !d.enabled(Category::DevCache));
    assert!(d.enabled(Category::Cache));
    let c = cfg_at(b"categories_off = []\n");
    assert!(!c.enabled(Category::BuildOutput));
    assert!(c.enabled(Category::DevCache), "dev-cache keeps its rule");
    let c = cfg_at(b"categories_on = [\"build-output\", \"nope\"]\n");
    assert!(c.enabled(Category::BuildOutput));
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("unknown category 'nope' in categories_on")),
        "{:?}",
        c.warnings
    );
    let c = cfg_at(b"categories_on = [\"logs\"]\ncategories_off = [\"logs\"]\n");
    assert!(c.enabled(Category::Logs), "categories_on wins");
}

#[test]
fn system_keys_defaults_and_invalid_values_warn() {
    let t = FixtureTree::new();
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (2, "500M"));

    let write = |body: &str| t.write("/home/user/.config/crumbs/config.toml", body.as_bytes());
    write("pkg_cache_keep = 0\njournal_max = \"2G\"\n");
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (0, "2G"));
    assert!(c.warnings.is_empty());

    write("journal_max = \"0\"\n");
    let c = load_config(&t.env()).unwrap();
    assert_eq!(c.journal_max, "500M");
    assert_eq!(c.warnings.len(), 1);

    write("pkg_cache_keep = -1\njournal_max = \"1.5G\"\n");
    let c = load_config(&t.env()).unwrap();
    assert_eq!((c.pkg_cache_keep, c.journal_max.as_str()), (2, "500M"));
    assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
    assert!(c.warnings[0].contains("pkg_cache_keep"));
    assert!(c.warnings[1].contains("journal_max"));
}
