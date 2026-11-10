use super::*;

#[test]
fn lexical_table() {
    let l = |s: &str| lexical(Path::new(s)).map(|p| p.display().to_string());
    assert_eq!(l("/a/b/").as_deref(), Some("/a/b"));
    assert_eq!(l("/a/./b").as_deref(), Some("/a/b"));
    assert_eq!(l("/a//b").as_deref(), Some("/a/b"));
    assert_eq!(l("/a/../b"), None);
    assert_eq!(l("a/b"), None);
}

#[test]
fn exec_start_table() {
    let p = |s: &str| exec_start_program(s);
    assert_eq!(
        p("[Service]\nExecStart=-/usr/bin/foo --x\n").as_deref(),
        Some("/usr/bin/foo")
    );
    assert_eq!(
        p("[Service]\nExecStart=/usr/bin/env A=1 B=2 bar\n").as_deref(),
        Some("bar")
    );
    assert_eq!(p("[Unit]\nExecStart=/usr/bin/foo\n"), None);
    assert_eq!(
        p("[Service]\nExecStart=\"/opt/My App/run\" x\n").as_deref(),
        Some("/opt/My App/run")
    );
}
