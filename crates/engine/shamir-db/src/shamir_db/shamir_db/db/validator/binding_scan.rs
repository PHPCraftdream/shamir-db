//! AUTHORITATIVE binding-holder derivation (round 3, review findings 1
//! and 2): the set of databases that bind a validator is derived from the
//! tables' OWN persisted bindings at admission/refusal time.
//!
//! The reverse cache is NOT consulted: a configured table whose binding
//! envelope or info store is absent counts as unbound (a table that was never
//! bound legitimately has none, and a reverse reference to an earlier
//! incarnation of the same name must not decide for a new table). Known
//! limit: an envelope lost by storage damage is therefore not detected here.
//! Reads use an existing-only lookup that creates no store — scoped exactly
//! like the operations guide scopes the Fjall caveat: FACADE DDL only, because
//! an external deletion of the keyspace can still race the check-then-create
//! inside that lookup. Existing metadata reads use only the mapped `__info__`
//! store; no `TableManager` is constructed, no `__data__`/`__history__` store
//! is touched, no index recovery runs, no row data is loaded. This read-only
//! scan never drains or syncs the backend; rare DDL binding writes do so
//! through the storage wrapper. The internal store is read by its own handle.
//!
//! Round 6 (F2b), widened in round 7 (G1): a table whose binding record is IN
//! DOUBT — a failed binding write the manager could not write back and flush —
//! holds EVERY validator identity, because it may still validate with a binding
//! the backing record lost. The flag lives on the table's already-open manager:
//! a never-opened (cold) manager has none, so such a table contributes nothing
//! here, and a restart clears the flag by definition (it resets the flag; it
//! does not repair the records). Such a table contributes its DATABASE to the
//! reservation set and nothing to the table refs, so no refusal message
//! discloses it. Its database is also added to `in_doubt_dbs`, which drives
//! `BindingHolders::refuse_in_doubt` — the shared refusal that validator
//! replace, rename and drop all call, because a reservation alone is NOT
//! evidence: a doubtful table's database is usually free, and a vacant fence
//! slot reserves successfully.
//!
//! Exclusion domain / TOCTOU: the `ReplicationControlGuard` parameter
//! proves the caller holds replication control for the whole derive →
//! reserve → commit sequence, and `bind_validator_as`,
//! `unbind_validator` and the database/repo/table rename paths take the
//! same lock, so the binding set cannot change between derivation and
//! the reservations it feeds. No DashMap guard is held across an
//! `.await` (the instances are snapshotted first) and every result is
//! deterministically sorted.

use std::collections::BTreeSet;

use shamir_types::types::record_id::RecordId;

use crate::{DbError, DbResult};

use crate::engine::db_instance::db_instance::DbInstance;
use crate::shamir_db::shamir_db::ReplicationControlGuard;

use crate::shamir_db::shamir_db::{ShamirDb, SYSTEM_DB_NAME};

/// The authoritative binding-holder view of one validator: the databases
/// that reserve the migration fence (`dbs`, sorted, deduplicated) and the
/// sorted `db/repo/table` refs of every holding table (`tables`) for
/// refusal messages. The internal system store contributes `tables` refs
/// only — it is never reserved.
pub(in crate::shamir_db) struct BindingHolders {
    dbs: Vec<String>,
    tables: Vec<String>,
    /// Databases holding an IN-DOUBT table (round 6). Their fence is
    /// reserved like a real holder's, their table refs stay out of `tables()`
    /// so no refusal message discloses them, and the field is the input of
    /// `refuse_in_doubt` — the shared in-doubt refusal every admitted caller
    /// (replace, rename, drop) consumes (round 7, G1).
    in_doubt_dbs: Vec<String>,
}

impl BindingHolders {
    /// Databases whose migration fence must be reserved (sorted, deduped,
    /// internal system store excluded by handle).
    pub(in crate::shamir_db) fn dbs(&self) -> &[String] {
        &self.dbs
    }

    /// Sorted holding table refs (user + internal store).
    pub(in crate::shamir_db) fn tables(&self) -> &[String] {
        &self.tables
    }

    /// True when nothing anywhere holds a binding — a real envelope match or
    /// an in-doubt table both count.
    pub(in crate::shamir_db) fn is_empty(&self) -> bool {
        self.tables.is_empty() && self.in_doubt_dbs.is_empty()
    }

    /// Round 7 (G1): pure decision on this evidence set — at least one scanned
    /// table has an IN-DOUBT binding record. No I/O, no table path.
    pub(in crate::shamir_db) fn has_in_doubt(&self) -> bool {
        !self.in_doubt_dbs.is_empty()
    }

    /// The ONE in-doubt refusal, built from [`Self::has_in_doubt`] (round 7,
    /// G1): every admitted caller that must not act on a doubtful binding state
    /// — validator replace, rename (wire and migration preflight) and drop —
    /// returns this same typed error, so drop's `is_empty()` evidence and the
    /// new refusal cannot drift apart. The text is fixed and path-free (see
    /// `TableManager::binding_in_doubt_error`): neither the table nor the
    /// database name reaches it, and it is the same text this table's own
    /// bind/unbind refuses with.
    pub(in crate::shamir_db) fn refuse_in_doubt(&self) -> DbResult<()> {
        if self.has_in_doubt() {
            return Err(crate::engine::table::TableManager::binding_in_doubt_error());
        }
        Ok(())
    }
}

impl ShamirDb {
    /// Derive the binding holders of `validator_id` from the tables' own
    /// persisted bindings. Scan errors propagate (fail closed — an
    /// unreadable table must neither admit a bound validator nor drop a
    /// bound one).
    pub(in crate::shamir_db) async fn validator_binding_holders(
        &self,
        validator_id: &RecordId,
        _control: &ReplicationControlGuard<'_>,
    ) -> DbResult<BindingHolders> {
        // Snapshot (name, instance) pairs and DROP the DashMap shard guard
        // before any `.await` — the metadata store opens below await.
        let instances: Vec<(String, DbInstance)> = self
            .dbs
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        let mut dbs = BTreeSet::new();
        let mut tables = Vec::new();
        let mut in_doubt = BTreeSet::new();
        for (db_name, instance) in instances {
            for repo_name in instance.list_repos() {
                // Clone the repo handle out of the map; no map guard is
                // held across the metadata reads below. `list_repos` reads
                // the same map, so an absent handle means a concurrent
                // drop: fail closed rather than silently skip a holder.
                let Some(repo) = instance.get_repo(&repo_name) else {
                    return Err(DbError::NotFound(format!(
                        "Repository '{repo_name}' not found"
                    )));
                };
                for table_name in instance.list_tables(&repo_name)? {
                    // Round 6 (F2b), widened in round 7: an in-doubt table
                    // holds EVERY identity. Checked BEFORE the envelope read:
                    // the envelope is exactly what cannot be trusted here.
                    if repo.table_validator_bindings_in_doubt(&table_name) {
                        in_doubt.insert(db_name.clone());
                        dbs.insert(db_name.clone());
                        continue;
                    }
                    let bound = repo.table_validator_bindings_metadata(&table_name).await?;
                    let Some(bound) = bound else {
                        continue;
                    };
                    if bound
                        .bindings
                        .iter()
                        .any(|binding| &binding.validator_id == validator_id)
                    {
                        tables.push(Self::table_ref_str(&db_name, &repo_name, &table_name));
                        dbs.insert(db_name.clone());
                    }
                }
            }
        }
        // The internal system store: scanned through its own handle, added to
        // the DROP refusal evidence but NEVER to the reservation set. This loop
        // deliberately IGNORES the in-doubt flag: a system table may not
        // reserve a database, so an in-doubt internal binding must not raise
        // the database-level doubt refusal either — it is simply invisible
        // here (known limit: an internal binding removed in backing before a
        // failed UNBIND is reconciled is not DROP evidence).
        let system_repo = self.system_store.system_repo()?;
        for table_name in system_repo.list_table_names() {
            let bound = system_repo
                .table_validator_bindings_metadata(&table_name)
                .await?;
            let Some(bound) = bound else {
                continue;
            };
            if bound
                .bindings
                .iter()
                .any(|binding| &binding.validator_id == validator_id)
            {
                tables.push(Self::table_ref_str(
                    SYSTEM_DB_NAME,
                    system_repo.name(),
                    &table_name,
                ));
            }
        }
        tables.sort();
        Ok(BindingHolders {
            dbs: dbs.into_iter().collect(),
            tables,
            in_doubt_dbs: in_doubt.into_iter().collect(),
        })
    }
}

#[cfg(test)]
impl BindingHolders {
    /// Test-only construction (round 7): the three evidence inputs of the pure
    /// in-doubt decision, with no database, store or table involved.
    pub(in crate::shamir_db) fn for_test(
        dbs: Vec<String>,
        tables: Vec<String>,
        in_doubt_dbs: Vec<String>,
    ) -> Self {
        Self {
            dbs,
            tables,
            in_doubt_dbs,
        }
    }
}
