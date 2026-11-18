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

use shamir_query_types::batch::tree_visitor::BatchTreeVisitor;

use crate::query::batch::{BatchError, BatchOp, BatchRequest};
use crate::shamir_db::shamir_db::ShamirDb;

/// Stable §5.3 code used for every maintenance-fence refusal.
const IN_PROGRESS: &str = "schema_migration_in_progress";

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

    /// Mark `db` as fenced in-process under `generation`. Only the winner of
    /// the durable fence CAS (step-2a `acquire_schema_migration_fence`)
    /// reaches this, so the blind insert cannot oversubscribe the gate.
    pub(in crate::shamir_db) fn hold_schema_migration_gate(&self, db: &str, generation: u64) {
        self.schema_migration_gate
            .insert(db.to_string(), generation);
    }

    /// Release the in-process gate, but only if the recorded generation still
    /// belongs to the releasing migration (a newer owner must not be evicted).
    pub(in crate::shamir_db) fn release_schema_migration_gate(&self, db: &str, generation: u64) {
        self.schema_migration_gate
            .remove_if(db, |_, held| *held == generation);
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
    // Gate not held for this database → nothing to refuse.
    shamir.schema_migration_gate_generation(db_name)?;
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
                code: Some(IN_PROGRESS.to_string()),
            });
        }
    }
    None
}
