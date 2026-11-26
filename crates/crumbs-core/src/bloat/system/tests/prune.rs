use super::*;

#[test]
fn prune_script_never_globs_a_listed_name() {
    // `2026-1*` must not become a file of root's working directory that
    // looks like a snapshot name and is then deleted.
    let odd = "0    >  2026-1*  D\n1    >  2026-11-10_08-18-24  O\n";
    let (ok, deleted) = run_prune_in(odd, "1", Some("2026-11-09_00-00-00"));
    assert!(!ok, "the literal name is not a snapshot name");
    assert!(deleted.is_empty(), "{deleted:?}");
}
