//! In-process application-schema maintenance fence (release-1 R1c step 2b-3a,
//! design §4.3/§4.5).
//!
//! One in-memory state per stable database identity. A migration reserves
//! before the durable CAS and holds the live generation after it succeeds.
//! A verified resume atomically replaces an in-doubt marker with a
//! reservation and restores the marker if it exits before attempting commit.
//! Once commit may publish, drop leaves the attempted generation in doubt.
//! Cancellation may leave no matching journal entry; in that case restart
//! and let journal replay establish ownership. A live read is not proof of abort.
//! A live runner releases on durable success or leaves its generation in
//! doubt on failure. Lifecycle mutations reserve only vacant identities.

use std::sync::atomic::Ordering;

use dashmap::mapref::entry::Entry;
use shamir_query_types::batch::tree_visitor::BatchTreeVisitor;

use crate::query::batch::{BatchError, BatchOp, BatchRequest};
use crate::shamir_db::shamir_db::ShamirDb;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::DbResult;

use super::super::admin_dispatch::explicit_migration_target;

/// Stable §5.3 code for a fence held by a live runner of this process.
const IN_PROGRESS: &str = "schema_migration_in_progress";

/// Stable §5.3 code for a fence that is still durably OWNED but has NO live
/// runner in this process (R1c step 2b-3b: the runner crashed or the server
/// restarted before resume) — seeded at database open from the journal.
const IN_DOUBT: &str = "schema_migration_in_doubt";

/// Stable code of the lifecycle refusal (R1c-4 FINAL follow-up 3, decision
/// 2): any database lifecycle op (RENAME/DROP) targeting a database with a
/// LIVE or RESERVED in-process gate OR an in-doubt marker reports
/// `schema_migration_in_doubt` (the one lifecycle wire code for both
/// states). Narrow accessor — the constant itself stays file-private.
pub(in crate::shamir_db) const fn schema_migration_lifecycle_code() -> &'static str {
    IN_DOUBT
}

/// Shared message shape of the lifecycle refusal (no per-callsite text
/// drift; the direct-call guards wrap it in `DbError::Validation`, the wire
/// handlers pair it with [`schema_migration_lifecycle_code`]). Truthful
/// about both gated states: a LIVE/RESERVED fence (active runner) and an
/// IN-DOUBT marker (unresolved journal) refuse alike.
pub(in crate::shamir_db) fn schema_migration_lifecycle_refusal_message(
    op: &str,
    db: &str,
) -> String {
    format!(
        "cannot {op} database '{db}': an application-schema migration is \
         active or its journal is unresolved for it; if a migration runner \
         is active, wait for it to finish, otherwise recover the in-doubt \
         migration (resume with the identical stored body; reopen the \
         database only after repairing the journal on disk)"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::shamir_db) enum FenceState {
    Reserved { previous_in_doubt: Option<u64> },
    Live(u64),
    InDoubt(u64),
    Lifecycle,
}

impl FenceState {
    fn refusal_code(self) -> Option<&'static str> {
        match self {
            Self::Reserved { .. } | Self::Live(_) => Some(IN_PROGRESS),
            Self::InDoubt(_) => Some(IN_DOUBT),
            Self::Lifecycle => None,
        }
    }
}

impl ShamirDb {
    /// §4.5: lower the server's node mode into the engine. Called once at
    /// server launch; `true` marks this engine a follower/read replica, and
    /// application-schema apply then fails closed engine-side.
    pub fn set_node_read_only(&self, read_only: bool) {
        self.node_read_only
            .store(read_only, std::sync::atomic::Ordering::Relaxed);
    }

    /// Engine-side primary-only admission check for apply/plan callers.
    pub(in crate::shamir_db) fn node_read_only(&self) -> bool {
        self.node_read_only.load(Ordering::Relaxed)
    }

    pub(in crate::shamir_db) fn database_identity_for_fence(
        &self,
        db: &str,
    ) -> Option<DatabaseIdentity> {
        self.database_identities.get(db).map(|entry| *entry)
    }

    fn fence_state(&self, id: DatabaseIdentity) -> Option<FenceState> {
        self.schema_migration_fences.get(&id).map(|entry| *entry)
    }

    pub(in crate::shamir_db) fn schema_migration_refusal_code(
        &self,
        id: DatabaseIdentity,
    ) -> Option<&'static str> {
        self.fence_state(id)?.refusal_code()
    }

    /// Reserve a resolved identity for an explicit-target mutation. Classify
    /// a refusal from the same entry that rejected the reservation.
    pub(in crate::shamir_db) fn reserve_admin_migration_target(
        &self,
        id: DatabaseIdentity,
    ) -> Result<SchemaMigrationLifecycleGuard<'_>, Option<&'static str>> {
        match self.schema_migration_fences.entry(id) {
            Entry::Vacant(slot) => {
                self.publish_fence_slot();
                slot.insert(FenceState::Lifecycle);
                Ok(SchemaMigrationLifecycleGuard { shamir: self, id })
            }
            Entry::Occupied(slot) => Err(slot.get().refusal_code()),
        }
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn reserve_schema_migration_lifecycle(
        &self,
        id: DatabaseIdentity,
    ) -> Option<SchemaMigrationLifecycleGuard<'_>> {
        self.reserve_admin_migration_target(id).ok()
    }

    pub(in crate::shamir_db) fn reserve_schema_migration_run(
        &self,
        id: DatabaseIdentity,
        expected_resume_generation: Option<u64>,
    ) -> Result<SchemaMigrationGateGuard<'_>, &'static str> {
        let previous_in_doubt = match self.schema_migration_fences.entry(id) {
            Entry::Vacant(slot) => {
                self.publish_fence_slot();
                slot.insert(FenceState::Reserved {
                    previous_in_doubt: None,
                });
                None
            }
            Entry::Occupied(mut slot) => match *slot.get() {
                FenceState::InDoubt(generation)
                    if expected_resume_generation
                        .is_some_and(|expected| expected >= generation) =>
                {
                    let previous_in_doubt = Some(generation);
                    slot.insert(FenceState::Reserved { previous_in_doubt });
                    previous_in_doubt
                }
                state => return Err(state.refusal_code().unwrap_or(IN_PROGRESS)),
            },
        };
        Ok(SchemaMigrationGateGuard {
            shamir: self,
            id,
            state: FenceState::Reserved { previous_in_doubt },
            possibly_published: None,
            succeeded: false,
        })
    }

    pub(in crate::shamir_db) fn reserve_applied_fence_release(
        &self,
        id: DatabaseIdentity,
        generation: u64,
    ) -> Option<SchemaMigrationGateGuard<'_>> {
        let Entry::Occupied(mut slot) = self.schema_migration_fences.entry(id) else {
            return None;
        };
        if *slot.get() != FenceState::InDoubt(generation) {
            return None;
        }
        let state = FenceState::Reserved {
            previous_in_doubt: Some(generation),
        };
        slot.insert(state);
        Some(SchemaMigrationGateGuard {
            shamir: self,
            id,
            state,
            possibly_published: None,
            succeeded: false,
        })
    }

    fn insert_fence_if_vacant(&self, id: DatabaseIdentity, state: FenceState) -> bool {
        let Entry::Vacant(slot) = self.schema_migration_fences.entry(id) else {
            return false;
        };
        self.publish_fence_slot();
        slot.insert(state);
        true
    }

    fn publish_fence_slot(&self) {
        // Increment under the shard lock before publishing; decrement after removal.
        // Zero cannot hide a published entry, even with transitions on different shards.
        self.schema_migration_fence_count
            .fetch_add(1, Ordering::AcqRel);
    }

    fn transition_fence(
        &self,
        id: DatabaseIdentity,
        from: FenceState,
        to: Option<FenceState>,
    ) -> bool {
        let Entry::Occupied(mut slot) = self.schema_migration_fences.entry(id) else {
            return false;
        };
        if *slot.get() != from {
            return false;
        }
        if let Some(state) = to {
            slot.insert(state);
        } else {
            slot.remove();
            self.schema_migration_fence_count
                .fetch_sub(1, Ordering::AcqRel);
        }
        true
    }

    fn in_doubt_generation(&self, db: &str) -> Option<u64> {
        match self.fence_state(self.database_identity_for_fence(db)?)? {
            FenceState::InDoubt(generation) => Some(generation),
            FenceState::Reserved { previous_in_doubt } => previous_in_doubt,
            FenceState::Live(_) | FenceState::Lifecycle => None,
        }
    }

    pub(in crate::shamir_db) fn schema_migration_in_doubt(&self, db: &str) -> bool {
        self.in_doubt_generation(db).is_some()
    }

    /// Preliminary migration-only refusal; external DDL conflicts are
    /// classified by the authoritative reservation, not by this observer.
    pub(in crate::shamir_db) fn schema_migration_lifecycle_blocked(&self, db: &str) -> bool {
        self.database_identity_for_fence(db)
            .and_then(|id| self.schema_migration_refusal_code(id))
            .is_some()
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn schema_migration_gate_generation(&self, db: &str) -> Option<u64> {
        match self.fence_state(self.database_identity_for_fence(db)?)? {
            FenceState::Reserved { .. } => Some(u64::MAX),
            FenceState::Live(generation) => Some(generation),
            FenceState::InDoubt(_) | FenceState::Lifecycle => None,
        }
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn schema_migration_gate_in_doubt_generation(
        &self,
        db: &str,
    ) -> Option<u64> {
        self.in_doubt_generation(db)
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn reserve_schema_migration_gate(
        &self,
        db: &str,
    ) -> Option<SchemaMigrationGateGuard<'_>> {
        self.reserve_schema_migration_run(self.database_identity_for_fence(db)?, None)
            .ok()
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn hold_schema_migration_gate(&self, db: &str, generation: u64) {
        let id = self
            .database_identity_for_fence(db)
            .expect("test database identity");
        if let Some(state @ FenceState::InDoubt(_)) = self.fence_state(id) {
            self.transition_fence(id, state, Some(FenceState::Live(generation)));
        } else {
            assert!(self.insert_fence_if_vacant(id, FenceState::Live(generation)));
        }
    }

    #[cfg(test)]
    pub(in crate::shamir_db) fn release_schema_migration_gate(&self, db: &str, generation: u64) {
        if let Some(id) = self.database_identity_for_fence(db) {
            self.transition_fence(id, FenceState::Live(generation), None);
        }
    }

    /// Validate all routed histories, including orphans. Only loaded database
    /// identities receive an in-memory marker; damage fails the whole open.
    pub(in crate::shamir_db) async fn seed_schema_migration_in_doubt(&self) -> DbResult<()> {
        let store = self.system_store();
        if !store.schema_migration_journal_durable() {
            return Ok(());
        }
        let routed = Box::pin(store.schema_migration_journal_routed_identities()).await?;
        let loaded: shamir_collections::TFxSet<_> = self
            .database_identities
            .iter()
            .map(|entry| *entry.value())
            .collect();
        for id in routed {
            if let Some(generation) = Box::pin(store.owned_schema_migration_fence(id)).await? {
                if loaded.contains(&id) {
                    self.insert_fence_if_vacant(id, FenceState::InDoubt(generation));
                }
            }
        }
        Ok(())
    }
}

pub(in crate::shamir_db) struct SchemaMigrationLifecycleGuard<'a> {
    shamir: &'a ShamirDb,
    id: DatabaseIdentity,
}

impl Drop for SchemaMigrationLifecycleGuard<'_> {
    fn drop(&mut self) {
        self.shamir
            .transition_fence(self.id, FenceState::Lifecycle, None);
    }
}

pub(in crate::shamir_db) struct SchemaMigrationGateGuard<'a> {
    shamir: &'a ShamirDb,
    id: DatabaseIdentity,
    state: FenceState,
    possibly_published: Option<u64>,
    succeeded: bool,
}

impl SchemaMigrationGateGuard<'_> {
    pub(super) fn observe_acquire_commit(&mut self, generation: Option<u64>) {
        self.possibly_published = generation;
    }

    pub(in crate::shamir_db) fn hold(&mut self, generation: u64) {
        let live = FenceState::Live(generation);
        assert!(
            matches!(self.state, FenceState::Reserved { .. })
                && self
                    .shamir
                    .transition_fence(self.id, self.state, Some(live)),
            "migration gate lost its reservation before hold"
        );
        self.state = live;
    }

    pub(in crate::shamir_db) fn mark_succeeded(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for SchemaMigrationGateGuard<'_> {
    fn drop(&mut self) {
        let next = match self.state {
            FenceState::Reserved { previous_in_doubt } => self
                .possibly_published
                .or(previous_in_doubt)
                .map(FenceState::InDoubt),
            FenceState::Live(generation) if !self.succeeded => {
                Some(FenceState::InDoubt(generation))
            }
            FenceState::Live(_) | FenceState::InDoubt(_) | FenceState::Lifecycle => None,
        };
        self.shamir.transition_fence(self.id, self.state, next);
    }
}

/// Synchronous batch-entry gate (design §4.3 "safe initial release default"):
/// while the database's maintenance fence is held in-process, refuse the
/// whole batch if a leaf targeting the serving database writes or calls a
/// procedure. Dispatch handles explicit database mutators after target
/// authorization. Reads and `ApplicationSchema` operations pass this check.
///
/// `#[inline(never)]` + sync: the whole tree walk lives in this plain stack
/// frame so the async entry points only carry the one call (same
/// stack-safety shape as `tx_read_quarantine_refusal_for_request`).
#[inline(never)]
pub(in crate::shamir_db) fn maintenance_fence_refusal(
    shamir: &ShamirDb,
    db_name: &str,
    request: &BatchRequest,
) -> Option<BatchError> {
    if shamir.schema_migration_fence_count.load(Ordering::Acquire) == 0 {
        return None;
    }
    let id = shamir.database_identity_for_fence(db_name)?;
    let code = shamir.schema_migration_refusal_code(id)?;
    for visit in BatchTreeVisitor::new(&request.queries) {
        let op = &visit.entry.op;
        // Visit leaves only. Explicit target mutations (including same-DB
        // targets) authorize and reserve in dispatch. CREATE only claims a
        // namespace slot: keep the serving check for same-name CREATE, but
        // never check a foreign CREATE against the existing target's fence.
        if matches!(
            op,
            BatchOp::ApplicationSchema(_) | BatchOp::Batch(_) | BatchOp::ForEach(_)
        ) {
            continue;
        }
        if explicit_migration_target(op)
            .is_some_and(|target| !matches!(op, BatchOp::CreateDb(_)) || target != db_name)
        {
            continue;
        }
        if op.is_write() || matches!(op, BatchOp::Call(_)) {
            return Some(BatchError::QueryError {
                alias: visit.alias.to_string(),
                message: format!(
                    "query '{}': an application-schema migration holds the maintenance \
                     fence for database '{db_name}'; writes, DDL and stored-procedure \
                     calls are refused until it completes",
                    visit.alias
                ),
                code: Some(code.to_string()),
            });
        }
    }
    None
}
