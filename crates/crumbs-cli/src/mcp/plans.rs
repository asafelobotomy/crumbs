//! The plans a preview made and `apply` may run once: in memory, never
//! written anywhere, gone after [`PLAN_TTL`] or when the server stops.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, SystemTime};

use crate::cmd::act::Target;

/// How long a previewed plan can be applied.
pub(crate) const PLAN_TTL: Duration = Duration::from_secs(600);

/// The most plans held at once.
pub(crate) const MAX_PLANS: usize = 64;

/// Why a preview past [`MAX_PLANS`] is refused.
pub(crate) const TOO_MANY_PLANS: &str = "too many pending plans; apply or let some expire first";

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
    pub(crate) fn parse(s: &str) -> Option<PlanId> {
        let ok = s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| PlanId(s.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// What `apply` runs.
pub(crate) enum PlanPayload {
    /// Plans with their scan-time fingerprints, for `execute_many`.
    Plans(Vec<Target>),
    /// The core restores without a plan: these are the history entries the
    /// dry run said it would restore, as the user was shown them. Apply
    /// restores exactly those, never what the selectors match by then.
    Restore { entry_ids: Vec<String> },
}

/// A previewed plan and what the user is shown for it.
pub(crate) struct StoredPlan {
    pub payload: PlanPayload,
    pub preview_text: String,
    /// The equivalent `crumbs …` argv, for a client that cannot confirm.
    pub cli_command: Vec<String>,
    pub expires_at: SystemTime,
    /// The key of this plan's confirmation among the input requests of an
    /// `InputRequiredResult`: made by the server when the plan is stored,
    /// so an answer counts only under the key this server issued.
    pub confirm_key: String,
    /// The server has returned the `InputRequiredResult` that asks for
    /// this plan's confirmation: only then can a retry answer it.
    pub asked: bool,
}

/// Why [`PlanStore::insert`] did not store a plan.
#[derive(Debug)]
pub(crate) enum InsertError {
    /// [`MAX_PLANS`] are held already.
    Full,
    /// No random id could be made.
    Id(std::io::Error),
}

impl std::fmt::Display for InsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InsertError::Full => f.write_str(TOO_MANY_PLANS),
            InsertError::Id(e) => write!(f, "cannot make a plan id: {e}"),
        }
    }
}

#[derive(Default)]
pub(crate) struct PlanStore {
    plans: HashMap<PlanId, StoredPlan>,
}

impl PlanStore {
    /// Stores `plan` under a fresh id, with a fresh `confirm_key` (whatever
    /// the caller put there is replaced).
    pub(crate) fn insert(&mut self, mut plan: StoredPlan) -> Result<PlanId, InsertError> {
        if self.plans.len() >= MAX_PLANS {
            return Err(InsertError::Full);
        }
        plan.asked = false;
        plan.confirm_key = format!("confirm-{}", PlanId::generate().map_err(InsertError::Id)?.0);
        loop {
            let id = PlanId::generate().map_err(InsertError::Id)?;
            if !self.plans.contains_key(&id) {
                self.plans.insert(id.clone(), plan);
                return Ok(id);
            }
        }
    }

    pub(crate) fn peek(&self, id: &PlanId) -> Option<&StoredPlan> {
        self.plans.get(id)
    }

    /// Records that the server asked for `id`'s confirmation; false when
    /// there is no such plan.
    pub(crate) fn mark_asked(&mut self, id: &PlanId) -> bool {
        self.plans.get_mut(id).map(|p| p.asked = true).is_some()
    }

    /// Puts back a plan taken out under `id`, as it was (no new id or key).
    pub(crate) fn put_back(&mut self, id: PlanId, plan: StoredPlan) {
        self.plans.insert(id, plan);
    }

    /// Removes the plan: it can be used once.
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
            cli_command: Vec::new(),
            expires_at,
            confirm_key: String::new(),
            asked: false,
        }
    }

    #[test]
    fn plan_ids_are_unique() {
        let ids: HashSet<String> = (0..1000)
            .map(|_| {
                let mut store = PlanStore::default();
                let id = store.insert(plan(SystemTime::now())).unwrap();
                assert!(PlanId::parse(id.as_str()).is_some(), "{}", id.as_str());
                id.as_str().to_string()
            })
            .collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn store_holds_at_most_64_plans_each_with_its_own_confirm_key() {
        let mut store = PlanStore::default();
        let mut keys = HashSet::new();
        for _ in 0..MAX_PLANS {
            let id = store.insert(plan(SystemTime::now())).unwrap();
            let key = &store.peek(&id).unwrap().confirm_key;
            assert!(key.starts_with("confirm-") && key.len() == 40, "{key}");
            assert_ne!(key.trim_start_matches("confirm-"), id.as_str());
            keys.insert(key.clone());
        }
        assert_eq!(keys.len(), MAX_PLANS);
        let err = store.insert(plan(SystemTime::now())).unwrap_err();
        assert!(matches!(err, InsertError::Full));
        assert_eq!(err.to_string(), TOO_MANY_PLANS);
        assert_eq!(store.len(), MAX_PLANS);
        // Room again once one is gone.
        let some = store.plans.keys().next().unwrap().clone();
        store.take(&some);
        assert!(store.insert(plan(SystemTime::now())).is_ok());
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
