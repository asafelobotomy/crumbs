//! The plans a preview made and `apply` may run once: in memory, never
//! written anywhere, gone after [`PLAN_TTL`] or when the server stops.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, SystemTime};

use crumbs_core::restore::RestoreOutcome;

use crate::cmd::act::Target;

/// How long a previewed plan can be applied.
pub(crate) const PLAN_TTL: Duration = Duration::from_secs(600);

/// 128 random bits as 32 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PlanId(String);

impl PlanId {
    /// A fresh id from `/dev/urandom`. When that cannot be read there is
    /// no id: nothing weaker stands in for it.
    fn generate() -> std::io::Result<PlanId> {
        let mut bytes = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(PlanId(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// The id `s` spells, if it is shaped like one.
    #[cfg_attr(not(test), allow(dead_code, reason = "used by apply in a later task"))]
    pub(crate) fn parse(s: &str) -> Option<PlanId> {
        let ok = s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| PlanId(s.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// What `apply` runs.
#[allow(dead_code, reason = "apply reads the payload in a later task")]
pub(crate) enum PlanPayload {
    /// Plans with their scan-time fingerprints, for `execute_many`.
    Plans(Vec<Target>),
    /// The core restores without a plan: the selectors run again, and the
    /// dry run's outcomes are what the user was shown.
    Restore {
        selectors: Vec<String>,
        outcomes: Vec<RestoreOutcome>,
    },
}

/// A previewed plan and what the user is shown for it.
#[allow(dead_code, reason = "apply reads the stored plan in a later task")]
pub(crate) struct StoredPlan {
    pub payload: PlanPayload,
    pub preview_text: String,
    pub needs_admin: bool,
    /// The equivalent `crumbs …` argv, for a client that cannot confirm.
    pub cli_command: Vec<String>,
    pub expires_at: SystemTime,
}

#[derive(Default)]
pub(crate) struct PlanStore {
    plans: HashMap<PlanId, StoredPlan>,
}

impl PlanStore {
    pub(crate) fn insert(&mut self, plan: StoredPlan) -> std::io::Result<PlanId> {
        loop {
            let id = PlanId::generate()?;
            if !self.plans.contains_key(&id) {
                self.plans.insert(id.clone(), plan);
                return Ok(id);
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "used by apply in a later task"))]
    pub(crate) fn peek(&self, id: &PlanId) -> Option<&StoredPlan> {
        self.plans.get(id)
    }

    /// Removes the plan: it can be used once.
    #[cfg_attr(not(test), allow(dead_code, reason = "used by apply in a later task"))]
    pub(crate) fn take(&mut self, id: &PlanId) -> Option<StoredPlan> {
        self.plans.remove(id)
    }

    /// Drops every plan that has expired at `now`.
    pub(crate) fn purge_expired(&mut self, now: SystemTime) {
        self.plans.retain(|_, p| p.expires_at > now);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.plans.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn plan(expires_at: SystemTime) -> StoredPlan {
        StoredPlan {
            payload: PlanPayload::Plans(Vec::new()),
            preview_text: String::new(),
            needs_admin: false,
            cli_command: Vec::new(),
            expires_at,
        }
    }

    #[test]
    fn plan_ids_are_unique() {
        let mut store = PlanStore::default();
        let ids: HashSet<String> = (0..1000)
            .map(|_| {
                let id = store.insert(plan(SystemTime::now())).unwrap();
                assert!(PlanId::parse(id.as_str()).is_some(), "{}", id.as_str());
                id.as_str().to_string()
            })
            .collect();
        assert_eq!(ids.len(), 1000);
        assert_eq!(store.len(), 1000);
    }

    #[test]
    fn parse_wants_32_lowercase_hex() {
        assert!(PlanId::parse(&"a1".repeat(16)).is_some());
        assert!(PlanId::parse(&"A1".repeat(16)).is_none());
        assert!(PlanId::parse("abc").is_none());
        assert!(PlanId::parse(&"g".repeat(32)).is_none());
    }

    #[test]
    fn purge_drops_only_the_expired_and_take_removes() {
        let now = SystemTime::now();
        let mut store = PlanStore::default();
        let old = store.insert(plan(now + Duration::from_secs(5))).unwrap();
        let fresh = store.insert(plan(now + PLAN_TTL)).unwrap();
        store.purge_expired(now + Duration::from_secs(5));
        assert!(store.peek(&old).is_none());
        assert!(store.peek(&fresh).is_some());
        assert!(store.take(&fresh).is_some());
        assert!(store.take(&fresh).is_none());
    }
}
