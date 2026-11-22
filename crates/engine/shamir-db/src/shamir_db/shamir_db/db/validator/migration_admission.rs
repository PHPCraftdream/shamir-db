//! Migration-owned validator rename admission (brief 84, item 3 / review
//! finding 4): the guard infrastructure that lets an application-schema
//! migration preflight a body containing `RenameValidator` BEFORE the first
//! journal write, so a foreign admission conflict surfaces as the foreign
//! target's code and never strands the serving database behind its own
//! fence.
//!
//! Ordering contract: the APPLY runner acquires replication control, calls
//! [`ShamirDb::begin_validator_rename_migration`] with the DISTINCT source
//! names of every `RenameValidator` in the body and the serving database's
//! own identity, and holds the returned [`ValidatorRenameMigration`] RAII
//! for the whole run. Replication control is dropped BEFORE the runner
//! executes; the foreign fence reservations and the module-level gate live
//! on in the guard and are released together on drop.
//!
//! Cancellation safety: the gate bool is published LAST — all awaits
//! (holder derivation, fence reservation) happen while replication control
//! is still held and BEFORE the CAS, and there is no `.await` between the
//! successful CAS and the returned guard. A future dropped mid-derivation
//! can therefore never strand the gate set: before the CAS the bool was
//! never taken, after the CAS the guard already exists.
//!
//! Exclusion semantics:
//! * The `validator_rename_migration` gate (CAS false → true) admits at
//!   most ONE validator-rename migration at a time. Other migration-family
//!   runners already serialise per database; this closes the cross-database
//!   gap.
//! * While the gate is set, ordinary validator namespace mutation
//!   (create incl. fresh, replace, rename, drop) and NEW binds must refuse
//!   via [`ShamirDb::refuse_concurrent_validator_namespace_mutation`] (the
//!   facade wiring belongs to the validator-management item). Unbind may
//!   continue: it only shrinks the holder set, and a reserved foreign fence
//!   already refuses wire DDL against that database.
//! * A rename body whose validator is replace/drop/HMAC-shaped is
//!   inadmissible in a migration anyway (security/hmac.rs), so only renames
//!   reach this guard.
//!
//! Missing sources are skipped, not errors: a valid resume can have ALREADY
//! renamed `from` → `to` in an earlier attempt, and an earlier in-body
//! CREATE/RENAME can have introduced the source. An absent source carries
//! no new binding holders while the namespace and bind facade are frozen by
//! this very gate, so reserving nothing for it is sound. Only future
//! effects (the rename still to run) need target reservations; integration
//! may conservatively inspect the destination too, but this guard's
//! `permits` view covers the reserved set.
//!
//! Soundness: the holder set is derived AUTHORITATIVELY under the caller's
//! `ReplicationControlGuard` (the same serialisation bind/unbind and the
//! facade mutations take) via `validator_binding_holders` — the tables' own
//! persisted bindings, never the best-effort reverse index. Source names
//! resolve to validator ids from the live registry first, then the durable
//! catalogue row (a migration can run after a restart before the registry
//! is repopulated). Foreign targets are reserved through the SAME shared
//! reservation helper as ordinary admission
//! (`reserve_validator_target_identities_locked`), so identity capture,
//! re-verification and the `Fence` error mapping cannot drift.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::shamir_db::execute::admin_db_schema_migration::SchemaMigrationLifecycleGuard;
use crate::shamir_db::shamir_db::db::validator::ValidatorRegistrationError;
use crate::shamir_db::shamir_db::{ReplicationControlGuard, ShamirDb};
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;

/// RAII hold of one validator-rename migration's foreign reservations.
/// Dropping it releases every foreign fence (each
/// [`SchemaMigrationLifecycleGuard`] transitions its slot back) and only
/// then clears the module gate, so a successor cannot pass the CAS while
/// foreign slots are still held.
pub(in crate::shamir_db) struct ValidatorRenameMigration<'a> {
    gate: Arc<AtomicBool>,
    serving: Option<DatabaseIdentity>,
    /// The foreign identities actually reserved (sorted order; the serving
    /// identity is exempt and never listed).
    foreign: Vec<DatabaseIdentity>,
    fences: Vec<SchemaMigrationLifecycleGuard<'a>>,
}

impl<'a> ValidatorRenameMigration<'a> {
    /// The capability view integration wires against: the exact foreign
    /// target identities whose fences this guard holds, plus the serving
    /// identity it was constructed for (`None` when the serving database has
    /// no cached identity, which exempts nothing).
    pub(in crate::shamir_db) fn permits(&self) -> (&[DatabaseIdentity], Option<DatabaseIdentity>) {
        (&self.foreign, self.serving)
    }
}

impl Drop for ValidatorRenameMigration<'_> {
    fn drop(&mut self) {
        // Release the foreign fences BEFORE clearing the gate.
        let fences = std::mem::take(&mut self.fences);
        drop(fences);
        self.gate.store(false, Ordering::Release);
    }
}

impl ShamirDb {
    /// True while a validator-rename migration holds its reservations.
    pub(in crate::shamir_db) fn validator_rename_migration_in_progress(&self) -> bool {
        self.validator_rename_migration.load(Ordering::Acquire)
    }

    /// Facade admission check (handoff to the validator-management item):
    /// ordinary validator create (incl. fresh), replace, rename, drop and
    /// NEW bind call this after their replication-control acquisition and
    /// refuse while a validator-rename migration is in progress. Unbind
    /// deliberately does NOT call it (it only shrinks the holder set).
    pub(in crate::shamir_db) fn refuse_concurrent_validator_namespace_mutation(
        &self,
        op: &str,
    ) -> DbResult<()> {
        if self.validator_rename_migration_in_progress() {
            return Err(DbError::Validation(format!(
                "cannot {op} validator: an application-schema migration \
                 renaming validators is in progress"
            )));
        }
        Ok(())
    }

    /// Preflight a migration body containing `RenameValidator` operations:
    /// under the caller's replication control, resolve each distinct source
    /// name to its validator id (absent names are skipped — see the module
    /// comment), derive the authoritative binding-holder databases and
    /// reserve every FOREIGN holder's migration fence through the shared
    /// ordinary-admission reservation helper. The serving database is
    /// exempt; its fence the runner already holds.
    ///
    /// The gate is published only on success, with no `.await` between the
    /// CAS and the returned guard (cancellation safety, module comment).
    pub(in crate::shamir_db) async fn begin_validator_rename_migration(
        &self,
        source_names: &[String],
        serving: Option<DatabaseIdentity>,
        control: &ReplicationControlGuard<'_>,
    ) -> Result<ValidatorRenameMigration<'_>, ValidatorRegistrationError> {
        // 1) Derive + reserve under the caller's replication control. Every
        //    failure here (and any cancellation of this future) leaves the
        //    gate untouched.
        let holder_dbs = self.rename_source_holder_dbs(source_names, control).await?;
        let mut foreign = Vec::with_capacity(holder_dbs.len());
        for db in &holder_dbs {
            if let Some(id) = self.database_identity_for_fence(db) {
                if Some(id) != serving {
                    foreign.push(id);
                }
            }
        }
        let fences =
            self.reserve_validator_target_identities_locked(&holder_dbs, serving, control)?;
        // 2) Publish the gate — no `.await` from here to the return value.
        match self.validator_rename_migration.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(ValidatorRenameMigration {
                gate: Arc::clone(&self.validator_rename_migration),
                serving,
                foreign,
                fences,
            }),
            // Losing the race unwinds the just-taken fences with the Vec.
            Err(_) => Err(ValidatorRegistrationError::Db(DbError::Validation(
                "a validator-rename application-schema migration is \
                 already in progress"
                    .to_string(),
            ))),
        }
    }

    /// Derive the union (sorted, deduplicated) of the binding-holder
    /// databases across all source names that still resolve. A source that
    /// resolves neither in the live registry nor in the durable catalogue
    /// contributes nothing (module comment: already-renamed resume or a
    /// source introduced by an earlier in-body CREATE/RENAME, sound while
    /// the namespace is frozen).
    async fn rename_source_holder_dbs(
        &self,
        source_names: &[String],
        control: &ReplicationControlGuard<'_>,
    ) -> DbResult<Vec<String>> {
        let mut targets: BTreeSet<String> = BTreeSet::new();
        for name in source_names {
            // Propagate I/O and malformed-id failures (fail closed — the
            // original post-intent failure class); skip ONLY a source that
            // genuinely does not exist (already-renamed resume or a source
            // introduced by an earlier in-body CREATE/RENAME; sound while
            // the namespace and bind facade are frozen by this gate).
            let Some(id) = self.migration_validator_id(name).await? else {
                continue;
            };
            let holders = self.validator_binding_holders(&id, control).await?;
            // Round 8: a doubtful binding record refuses the migration rename
            // HERE, in the preflight, before any foreign reservation and before
            // the runner acquires the serving fence durably and journals the
            // intent. Refusing only at execution would leave the serving
            // database fenced in doubt by a rename the evidence never allowed.
            // Same shared decision and fixed text as replace, rename and drop.
            holders.refuse_in_doubt()?;
            targets.extend(holders.dbs().iter().cloned());
        }
        Ok(targets.into_iter().collect())
    }

    /// Resolve a migration body's `RenameValidator` source name to the
    /// validator id tables actually invoke: the live registry first, then
    /// the durable catalogue row (post-restart, pre-registry-restore).
    /// `Ok(None)` only for a genuinely absent source.
    async fn migration_validator_id(&self, name: &str) -> DbResult<Option<RecordId>> {
        if let Some(id) = self.validators.id_for_name(name) {
            return Ok(Some(id));
        }
        let Some(record) = self.system_store.load_validator(name).await? else {
            return Ok(None);
        };
        match record
            .get("_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<RecordId>().ok())
        {
            Some(id) => Ok(Some(id)),
            None => Err(DbError::Validation(format!(
                "validator '{name}' has an invalid catalogue id"
            ))),
        }
    }
}
