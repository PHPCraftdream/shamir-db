//! Weak registry for live facade changelog receiver gates.
use super::receiver::{ChangelogDeliveryGate, GateState};
use crate::{DbResult, ShamirDb};
use dashmap::DashMap;
use shamir_collections::THasher;
use std::sync::{Arc, Weak};

impl ShamirDb {
    pub(crate) async fn revoke_quarantined_receivers(&self) {
        let mut pending = Vec::new();
        for ((db, repo), gates) in self.changelog_receivers.snapshot() {
            if self.resync_quarantine.reason_for(&db, &repo).is_some()
                || self.resync_quarantine.reason_for(&db, "*").is_some()
            {
                for gate in gates {
                    gate.close();
                    pending.push(gate);
                }
            }
        }
        for gate in pending {
            gate.wait_idle().await;
        }
    }

    /// Recompute and install the cache. Resolution failures propagate; an
    /// existing cache remains installed and therefore fail-closed.
    pub(crate) async fn refresh_resync_quarantine_cache(&self) -> DbResult<()> {
        let entries = self.resync_quarantine_entries().await?;
        self.resync_quarantine.install(entries);
        self.revoke_quarantined_receivers().await;
        Ok(())
    }
}

/// Live receiver gates keyed by `(db, repo)`.
type ReceiverGates = DashMap<(String, String), Vec<Weak<GateState>>, THasher>;

#[derive(Clone)]
pub(crate) struct ChangelogReceiverRegistry {
    entries: Arc<ReceiverGates>,
}
impl ChangelogReceiverRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::with_hasher(THasher::default())),
        }
    }
    pub(crate) fn register(&self, db: &str, repo: &str, gate: ChangelogDeliveryGate) {
        let mut entry = self
            .entries
            .entry((db.to_string(), repo.to_string()))
            .or_default();
        entry.retain(|weak| weak.strong_count() != 0);
        entry.push(Arc::downgrade(&gate.0));
    }
    pub(crate) fn unregister(&self, db: &str, repo: &str, gate: &ChangelogDeliveryGate) {
        let key = (db.to_string(), repo.to_string());
        if let Some(mut entries) = self.entries.get_mut(&key) {
            entries
                .retain(|weak| weak.strong_count() != 0 && !weak.ptr_eq(&Arc::downgrade(&gate.0)));
        }
        // remove_if rechecks emptiness while holding the shard entry, so a
        // concurrent register cannot have its newly appended weak gate lost.
        self.entries
            .remove_if(&key, |_, entries| entries.is_empty());
    }
    pub(crate) fn snapshot(&self) -> Vec<((String, String), Vec<ChangelogDeliveryGate>)> {
        let mut result = Vec::new();
        self.entries.retain(|key, gates| {
            gates.retain(|weak| weak.strong_count() != 0);
            if gates.is_empty() {
                return false;
            }
            result.push((
                key.clone(),
                gates
                    .iter()
                    .filter_map(Weak::upgrade)
                    .map(ChangelogDeliveryGate)
                    .collect(),
            ));
            true
        });
        result
    }
}
