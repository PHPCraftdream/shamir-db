//! In-process application-schema maintenance fence (release-1 R1c step 2b-3a,
//! design §4.3/§4.5).
//!
//! While a migration owns the durable fence generation for a database, the
//! engine also holds an IN-PROCESS per-database gate here: a lock-free
//! `DashMap<db name, held generation>` on [`ShamirDb`] (same ideology as
//! `repo_create_locks` — no mutex on this path). The batch execution entries
//! consult it through the synchronous [`maintenance_fence_refusal`] helper:
//! from the moment the gate is held, any batch containing a write, a DDL
//! operation, or a stored-procedure call (`CallOp` — guest code may write
//! through the direct intrinsics) is refused with the stable
//! `schema_migration_in_progress` code; reads and the `ApplicationSchema`
//! operations themselves pass (apply enforces its own admission against the
//! durable journal). The migration's own DDL handlers are invoked directly
//! and do not traverse the batch entries, so they are unaffected.
//!
//! R1c step 2b-3b: the fence survives restart. At database open,
//! `init_inner` consults the durable journal once per database and seeds an
//! IN-DOUBT marker (`schema_migration_gate_in_doubt`) when the control
//! record still OWNS the fence and the journal is unresolved — i.e. a
//! runner crashed or the server restarted before resume. While only the
//! marker is present, the same write/DDL/`CallOp` refusals carry
//! `schema_migration_in_doubt`; a resumed runner's `hold` clears the marker
//! and installs the live gate (`in_progress`), and its RAII release removes
//! it as before. The marker is in-memory only: it never widens the durable
//! contract.
//!
//! Release is guaranteed on every apply exit path by an RAII guard in
//! `apply.rs`; the durable fence generation itself is released through the
//! step-2a `SystemStore` API only after the `applied` event is durable.
//!
//! The engine-side primary-only guard for apply also lives here: the server
//! lowers its node mode once at launch (`set_node_read_only`); a read-only
//! (follower/replica) engine refuses application-schema apply with
//! `schema_migration_follower_read_only` even if a future embedder bypasses
//! the server gate.

use std::sync::atomic::Ordering;

use dashmap::mapref::entry::Entry;
use shamir_query_types::batch::tree_visitor::BatchTreeVisitor;

use crate::query::batch::{BatchError, BatchOp, BatchRequest};
use crate::shamir_db::shamir_db::ShamirDb;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::{DbError, DbResult};

/// Stable §5.3 code for a fence held by a live runner of this process.
const IN_PROGRESS: &str = "schema_migration_in_progress";

/// Generation placeholder of a gate that is RESERVED (admission passed, the
/// durable fence CAS is still in flight): the gate is visible to every other
/// applier and to the batch entries from the reservation on, so no applier can
/// observe a durably started migration with no in-process gate.
const GATE_PENDING: u64 = u64::MAX;

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

/// Stable §5.3 code for an unreadable journal/catalogue. When the open-time
/// seed itself failed for a database, the marker stands for a BOOT FAILURE
/// and status/plan/apply report this code, not `schema_migration_in_doubt`.
const DURABILITY_UNAVAILABLE: &str = "schema_migration_durability_unavailable";

impl ShamirDb {
    /// §4.5: lower the server's node mode into the engine. Called once at
    /// server launch; `true` marks this engine a follower/read replica, and
    /// application-schema apply then fails closed engine-side. Lock-free
    /// (`AtomicBool`, written once at boot, read per apply admission).
    pub fn set_node_read_only(&self, read_only: bool) {
        self.node_read_only
            .store(read_only, std::sync::atomic::Ordering::Relaxed);
    }

    /// Engine-side primary-only admission check for apply/plan callers.
    pub(in crate::shamir_db) fn node_read_only(&self) -> bool {
        self.node_read_only.load(Ordering::Relaxed)
    }

    /// The generation of the fence currently held in-process for `db`, if any.
    pub(in crate::shamir_db) fn schema_migration_gate_generation(&self, db: &str) -> Option<u64> {
        self.schema_migration_gate.get(db).map(|entry| *entry)
    }

    /// Reserve the in-process gate for `db` BEFORE the durable fence CAS:
    /// atomic insert-if-absent, so exactly one applier of this process gets
    /// the reservation and every other one is refused with
    /// `schema_migration_in_progress` (a durable `started` row is therefore
    /// never visible without its in-process gate, and a concurrent applier
    /// can never take over a live runner as if it had crashed). The returned
    /// guard cancels the reservation if dropped before
    /// [`SchemaMigrationGateGuard::hold`].
    pub(in crate::shamir_db) fn reserve_schema_migration_gate(
        &self,
        db: &str,
    ) -> Option<SchemaMigrationGateGuard<'_>> {
        match self.schema_migration_gate.entry(db.to_string()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(slot) => {
                slot.insert(GATE_PENDING);
                Some(SchemaMigrationGateGuard {
                    shamir: self,
                    db: db.to_string(),
                    generation: None,
                    succeeded: false,
                })
            }
        }
    }

    /// Mark `db` as fenced in-process under `generation`. Only the winner of
    /// the durable fence CAS (step-2a `acquire_schema_migration_fence`, fresh
    /// or recovery) reaches this, so the blind insert cannot oversubscribe
    /// the gate. It also clears any open-time in-doubt marker: a resumed
    /// runner converts the in-doubt fence into a live one.
    pub(in crate::shamir_db) fn hold_schema_migration_gate(&self, db: &str, generation: u64) {
        self.schema_migration_gate_in_doubt.remove(db);
        self.schema_migration_gate
            .insert(db.to_string(), generation);
    }

    /// The generation of the fence the open-time rebuild recorded as durably
    /// OWNED with an unresolved journal entry — i.e. in doubt, no runner.
    /// Absent once a resumed runner has held the gate; `None` while the
    /// marker stands for an UNKNOWABLE state (boot failure) — such databases
    /// report `schema_migration_durability_unavailable`, never a guessed
    /// generation. Test-only reader.
    #[cfg(test)]
    pub(in crate::shamir_db) fn schema_migration_gate_in_doubt_generation(
        &self,
        db: &str,
    ) -> Option<u64> {
        self.schema_migration_gate_in_doubt
            .get(db)
            .and_then(|entry| *entry)
    }

    /// Whether ANY in-doubt marker is present for `db`, including the
    /// boot-failure variant whose generation is unknown (`None`). The
    /// presence-only reader of the batch fence; unlike the test-only
    /// generation getter it does not collapse the unknown-generation marker
    /// into "free".
    pub(in crate::shamir_db) fn schema_migration_in_doubt(&self, db: &str) -> bool {
        self.schema_migration_gate_in_doubt.contains_key(db)
    }

    /// Lifecycle-guard predicate (R1c-4 FINAL follow-up 3, decision 2):
    /// one source of truth for the database lifecycle checks (rename/drop):
    /// TRUE for any live fence state — reserved/held gate (`GATE_PENDING`
    /// included, from the reservation on) or open-time in-doubt marker.
    /// Seed publication stays marker-only by decision. Defined only here.
    pub(in crate::shamir_db) fn schema_migration_lifecycle_blocked(&self, db: &str) -> bool {
        self.schema_migration_gate_generation(db).is_some() || self.schema_migration_in_doubt(db)
    }

    /// Convert a LIVE gate into an IN-DOUBT marker: the run exited while the
    /// durable fence is still OWNED with an unresolved journal (in-doubt or
    /// postcondition-failure exit). The marker is installed BEFORE the live
    /// entry is removed (same generation guard as a plain release), so the
    /// transition never exposes a false free interval to a concurrent
    /// lifecycle check; the marker carries the run's generation so a later
    /// resume's `hold` (higher generation) converts it back to a live gate;
    /// writes stay refused with `schema_migration_in_doubt` until then.
    pub(in crate::shamir_db) fn mark_schema_migration_gate_in_doubt(
        &self,
        db: &str,
        generation: u64,
    ) {
        // Marker first: no free interval between live and in-doubt states.
        self.schema_migration_gate_in_doubt
            .insert(db.to_string(), Some(generation));
        self.release_schema_migration_gate(db, generation);
    }

    /// One-time open-time seed of the in-doubt marker: every database whose
    /// control record still OWNS the durable fence while its journal is
    /// unresolved is marked in doubt (in-memory only, generation known).
    /// A database whose fence/journal state is UNKNOWABLE at boot (the
    /// full ordinary-journal validation in
    /// `owned_schema_migration_fence` FAILED — ownership itself cannot be
    /// established)
    /// is marked with the unknown-generation variant (`None`): admission
    /// fails closed with `schema_migration_durability_unavailable`. A single
    /// payload-free `warn!` covers all failures. The TYPED journal error
    /// (`DbError::SchemaMigrationJournalUnreadable` — an unattributable row)
    /// is PROPAGATED: startup fails closed before any per-database
    /// quarantine (the routing validation above normally catches it first;
    /// this is the typed backstop). Every other error (e.g.
    /// `Locked`, backend failures) is swallowed into the unknown-generation
    /// marker as before and never re-labeled as journal corruption. The
    /// marker is in-memory only and is NOT cleared by repairing the journal
    /// on disk alone — the affected databases must be reopened for a fresh
    /// seed. A no-op without a durable journal. Called by `init_inner` only;
    /// never on the batch path.
    pub(in crate::shamir_db) async fn seed_schema_migration_in_doubt(
        &self,
        identities: &[(String, DatabaseIdentity)],
    ) -> DbResult<()> {
        let store = self.system_store();
        if !store.schema_migration_journal_durable() {
            return Ok(());
        }
        // ONE structural routing validation of the durable journal BEFORE the
        // per-database quarantine loop: an unattributable row (undecodable
        // physical row, missing/malformed record_key, key-vs-envelope or
        // readable-payload identity contradiction) fails the WHOLE startup
        // closed with the typed `schema_migration_durability_unavailable`,
        // even with zero databases. Non-journal failures (e.g. `Locked`)
        // propagate unchanged, never masked.
        // Boxed: keeps the scan future out of the caller's init future.
        Box::pin(store.validate_schema_migration_journal_routing()).await?;
        let mut unreadable = false;
        for (name, identity) in identities {
            let owned = Box::pin(store.owned_schema_migration_fence(*identity)).await;
            match owned {
                Ok(Some(generation)) => {
                    self.schema_migration_gate_in_doubt
                        .insert(name.clone(), Some(generation));
                }
                // An unattributable journal row: startup itself fails closed,
                // typed — never masked as a per-database boot failure.
                Err(DbError::SchemaMigrationJournalUnreadable(_)) => {
                    return Err(DbError::SchemaMigrationJournalUnreadable(
                        "unattributable schema migration journal row at boot".into(),
                    ));
                }
                // The control record read failed: mark unknown, never guess.
                Err(_) => {
                    unreadable = true;
                    self.schema_migration_gate_in_doubt
                        .insert(name.clone(), None);
                }
                Ok(None) => {}
            }
        }
        if unreadable {
            log::warn!(
                "application-schema maintenance fence: the durable migration state could not \
                 be read for at least one database at boot; affected databases fail closed \
                 until repaired and REOPENED"
            );
        }
        Ok(())
    }

    /// Release the in-process gate, but only if the recorded generation still
    /// belongs to the releasing migration (a newer owner must not be evicted).
    pub(in crate::shamir_db) fn release_schema_migration_gate(&self, db: &str, generation: u64) {
        self.schema_migration_gate
            .remove_if(db, |_, held| *held == generation);
    }
}

/// RAII owner of the in-process gate of one apply run, from the reservation
/// before the durable fence CAS to the end of the run. Dropped before
/// [`Self::hold`] it cancels the reservation (the CAS failed or the apply
/// future was dropped); after `hold` it releases the live gate on success and
/// converts it to the in-doubt marker otherwise. Every transition is
/// synchronous, so no await can separate the reservation from its owner.
pub(in crate::shamir_db) struct SchemaMigrationGateGuard<'a> {
    shamir: &'a ShamirDb,
    db: String,
    generation: Option<u64>,
    succeeded: bool,
}

impl SchemaMigrationGateGuard<'_> {
    /// The durable fence is owned under `generation`: replace the reservation
    /// with the live gate (also clearing an open-time in-doubt marker).
    pub(in crate::shamir_db) fn hold(&mut self, generation: u64) {
        self.shamir.hold_schema_migration_gate(&self.db, generation);
        self.generation = Some(generation);
    }

    /// The run ended with `applied` durable: the drop releases the gate
    /// instead of leaving an in-doubt marker.
    pub(in crate::shamir_db) fn mark_succeeded(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for SchemaMigrationGateGuard<'_> {
    fn drop(&mut self) {
        match self.generation {
            None => {
                self.shamir
                    .schema_migration_gate
                    .remove_if(&self.db, |_, held| *held == GATE_PENDING);
            }
            Some(generation) if self.succeeded => {
                self.shamir
                    .release_schema_migration_gate(&self.db, generation);
            }
            Some(generation) => {
                self.shamir
                    .mark_schema_migration_gate_in_doubt(&self.db, generation);
            }
        }
    }
}

/// Synchronous batch-entry gate (design §4.3 "safe initial release default"):
/// while the database's maintenance fence is held in-process, refuse the
/// whole batch if ANY operation in the tree — nested `Batch`/`ForEach` bodies
/// included — is a write, a DDL/admin op, or a `CallOp`. Reads and the
/// `ApplicationSchema` ops (status/plan/apply, which run their own admission
/// against the durable journal) pass.
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
    // Nothing is held for this database: neither a live runner's gate nor an
    // open-time in-doubt marker → nothing to refuse.
    let live = shamir.schema_migration_gate_generation(db_name);
    let in_doubt = shamir.schema_migration_in_doubt(db_name);
    if live.is_none() && !in_doubt {
        return None;
    }
    // A live runner of this process is authoritative over the open-time
    // in-doubt marker (hold clears it anyway); the code distinguishes the
    // two states for callers.
    let code = if live.is_some() {
        IN_PROGRESS
    } else {
        IN_DOUBT
    };
    for visit in BatchTreeVisitor::new(&request.queries) {
        let op = &visit.entry.op;
        // The application-schema operations keep working during a fence:
        // status reports the in-flight state, plan/apply refuse themselves
        // with the exact §5.3 code.
        if matches!(op, BatchOp::ApplicationSchema(_)) {
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

/// Fail-closed admission for the application-schema handlers (status/plan/
/// apply), called from the common dispatcher AFTER authorization and BEFORE
/// any journal byte is read: when the open-time in-doubt marker stands for a
/// BOOT FAILURE (state unknowable at open — the control-record read failed),
/// refuse with `schema_migration_durability_unavailable`
/// instead of guessing. A marker with a
/// KNOWN generation keeps the existing behavior (handlers read the journal
/// themselves and report `schema_migration_in_doubt` with the reason and
/// operation index). Repairing the journal on disk alone does not clear the
/// in-memory marker: the database must be reopened.
#[inline(never)]
pub(in crate::shamir_db) fn schema_migration_gate_boot_unavailable_refusal(
    shamir: &ShamirDb,
    db_name: &str,
) -> Option<BatchError> {
    shamir
        .schema_migration_gate_in_doubt
        .get(db_name)
        .filter(|entry| entry.value().is_none())
        .map(|_| BatchError::QueryError {
            alias: String::new(),
            message: format!(
                "the application-schema migration state for database \
                     '{db_name}' could not be read at boot; status, plan and \
                     apply fail closed until the journal or catalogue is \
                     repaired and the database is reopened"
            ),
            code: Some(DURABILITY_UNAVAILABLE.to_string()),
        })
}
