//! `snapshots`: Timeshift snapshots beyond the newest `snapshots_keep`.
//! Opt-in. Timeshift lists its snapshots only to root and the scan never
//! escalates, so the item has no count or size; the root step asks
//! Timeshift for its list and deletes the oldest, by name, through
//! Timeshift itself ([`PRUNE_TIMESHIFT`]).

use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{not_installed, privileged_item};
use crate::command::resolve_trusted;
use crate::error::CoreError;

/// The root step, run as `sh -c PRUNE_TIMESHIFT crumbs-snapshots <keep>`
/// in the privileged batch (whose `PATH` holds only the trusted system
/// directories). It lists Timeshift's snapshots, checks that every name is
/// Timeshift's `YYYY-MM-DD_HH-MM-SS` before deleting any, and deletes all
/// but the newest `<keep>` (at least 1), oldest first, with Timeshift.
pub const PRUNE_TIMESHIFT: &str = r#"keep=${1-}
case "$keep" in ''|*[!0-9]*) echo "crumbs: bad keep count '$keep'" >&2; exit 2 ;; esac
[ "$keep" -ge 1 ] || { echo "crumbs: keep at least 1 snapshot" >&2; exit 2; }
list=$(timeshift --list --scripted) || exit 1
names=$(printf '%s\n' "$list" | awk '$2 == ">" { print $3 }' | sort)
for n in $names; do
  case "$n" in
    [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_[0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
    *) echo "crumbs: unexpected snapshot name '$n'; nothing deleted" >&2; exit 1 ;;
  esac
done
total=$(printf '%s\n' "$names" | grep -c .)
drop=$((total - keep))
[ "$drop" -gt 0 ] || exit 0
for n in $(printf '%s\n' "$names" | head -n "$drop"); do
  timeshift --delete --snapshot "$n" --scripted >/dev/null || exit 1
done
"#;

/// Timeshift's settings; without them it was never set up.
const TIMESHIFT_CONFIG: &str = "/etc/timeshift/timeshift.json";

pub struct SnapshotsScanner;

impl Scanner for SnapshotsScanner {
    fn category(&self) -> Category {
        Category::Snapshots
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        resolve_trusted(cx.env, "timeshift")
            .is_none()
            .then(|| not_installed("timeshift"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        if !cx.env.sys(TIMESHIFT_CONFIG).is_file() {
            return Ok(Vec::new());
        }
        let keep = cx.cfg.snapshots_keep.max(1);
        let mut item = privileged_item(
            Category::Snapshots,
            format!("Timeshift snapshots beyond the newest {keep}"),
            None,
            Some(
                "Timeshift lists them only with administrator rights: which and how many show \
                 when cleaned; oldest first"
                    .into(),
            ),
            "sh",
            vec![
                "-c".into(),
                PRUNE_TIMESHIFT.into(),
                "crumbs-snapshots".into(),
                keep.to_string(),
            ],
        );
        // Opt-in: a snapshot is a way back.
        item.preselected = false;
        Ok(vec![item])
    }
}
