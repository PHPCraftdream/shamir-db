//! AUTHORITATIVE binding-holder derivation (round 3, review findings 1
//! and 2): the set of databases that bind a validator is derived from the
//! tables' OWN persisted bindings at admission/refusal time.
//!
//! The reverse cache is NOT consulted: a configured table whose binding
//! envelope or info store is absent counts as unbound (a table that was never
//! bound legitimately has none, and a reverse reference to an earlier
//! incarnation of the same name must not decide for a new table). Known
//! limit: an envelope lost by storage damage is therefore not detected here.
//! Reads use an existing-only lookup that creates no store. Existing metadata
//! reads use only the mapped `__info__` store; no `TableManager` is
//! constructed, no `__data__`/`__history__` store is touched, no index
//! recovery runs, no row data is loaded. This read-only scan never drains or
//! syncs the backend; rare DDL binding writes do so through the storage
//! wrapper. The internal store is read by its own handle.
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

    /// True when no table anywhere holds a binding.
    pub(in crate::shamir_db) fn is_empty(&self) -> bool {
        self.tables.is_empty()
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
        // The internal system store: scanned through its own handle, added
        // to the DROP refusal evidence but NEVER to the reservation set.
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
        })
    }
}
