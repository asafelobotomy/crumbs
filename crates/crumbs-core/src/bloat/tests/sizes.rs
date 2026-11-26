use super::*;

#[test]
fn sizes_on_compressing_btrfs_are_marked_uncompressed() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let t = FixtureTree::new();
    t.write("home/user/.cache/alpha/f", &[1u8; 9000]);
    t.write("home/user/.cache/beta/f", &[1u8; 9000]);
    let beta = t.dir.path().join("home/user/.cache/beta");
    t.write(
        "proc/self/mountinfo",
        format!(
            "1 0 0:1 / / rw - ext4 /dev/a rw\n2 1 0:2 / {} rw - btrfs /dev/b rw,compress=zstd\n",
            beta.display()
        )
        .as_bytes(),
    );
    let env = t.env();
    let runner = FakeRunner::new();
    let cfg = Config::default();
    let inventory = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &inventory,
        runner: &runner,
        now: test_now(),
    };
    let report = scan_all(&cx, &[Category::Cache]);
    let rep = &report.categories[0];
    let flagged: Vec<_> = rep
        .items
        .iter()
        .filter(|i| i.size_uncompressed)
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(flagged, ["beta"]);
    assert_eq!(rep.uncompressed_sizes, 1);
    let json = serde_json::to_value(rep).unwrap();
    assert_eq!(json["uncompressed_sizes"], 1);
    let items = json["items"].as_array().unwrap();
    let beta_json = items.iter().find(|i| i["label"] == "beta").unwrap();
    let alpha_json = items.iter().find(|i| i["label"] == "alpha").unwrap();
    assert_eq!(beta_json["size_uncompressed"], true);
    assert!(alpha_json.get("size_uncompressed").is_none());
    // A clean plan of the flagged item says so.
    let plan =
        crate::plan::build_clean_plan(&env, &rep.items, &crate::plan::PlanOptions::default())
            .unwrap();
    assert!(
        plan.notes
            .iter()
            .any(|n| n == crate::plan::NOTE_UNCOMPRESSED)
    );
    let alpha: Vec<BloatItem> = rep
        .items
        .iter()
        .filter(|i| !i.size_uncompressed)
        .cloned()
        .collect();
    let plan =
        crate::plan::build_clean_plan(&env, &alpha, &crate::plan::PlanOptions::default()).unwrap();
    assert!(
        !plan
            .notes
            .iter()
            .any(|n| n == crate::plan::NOTE_UNCOMPRESSED)
    );
}
