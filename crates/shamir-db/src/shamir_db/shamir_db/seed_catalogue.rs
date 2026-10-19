use futures::StreamExt;

#[cfg(any(test, feature = "test-util"))]
use shamir_types::types::value::QueryValue;

use crate::{DbError, DbResult, ShamirDb};

impl ShamirDb {
    /// Test-only subscription-catalogue injection seam (S5c-X round 9): upsert
    /// one subscription row through the SAME handle the real admin writers use
    /// — `system_store().subscriptions_table()` + `set_via_implicit_tx`, no db
    /// name (the catalogue lives on the SYSTEM repo, not a user db; a batch
    /// `Insert` addressed to db `"system"` fails with "Database 'system' not
    /// found"). `record` must carry the `name` field (the row key).
    #[cfg(any(test, feature = "test-util"))]
    pub async fn inject_subscription_row_for_test(&self, record: QueryValue) -> DbResult<()> {
        let name = record["name"]
            .as_str()
            .ok_or_else(|| DbError::Validation("subscription record needs a name".into()))?
            .to_owned();
        let store = self.system_store();
        let table = store.subscriptions_table().await?;
        let mut key_fields = crate::types::common::new_map();
        key_fields.insert("name".to_string(), QueryValue::Str(name));
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: QueryValue::Map(key_fields),
            value: record,
        };
        store.set_via_implicit_tx(&table, &op).await?;
        store.persist_table_interner(&table, None).await?;
        self.flush_seed_catalogue().await
    }

    /// Offline-only live-record cap across all system tables.
    pub const SEED_CATALOGUE_MAX_RECORDS: usize = 4096;
    /// Offline-only live-byte cap across all system tables.
    pub const SEED_CATALOGUE_MAX_BYTES: usize = 8 * 1024 * 1024;

    /// Before the normal init loaders allocate their complete catalogues, scan
    /// live MVCC payloads one at a time (the raw __data__ store is inert).
    /// Caps: `SEED_CATALOGUE_MAX_RECORDS` records and `SEED_CATALOGUE_MAX_BYTES`
    /// across all system tables, including functions/validators loaded at boot.
    pub async fn check_seed_catalogue(&self) -> DbResult<()> {
        self.check_seed_catalogue_with_limits(
            Self::SEED_CATALOGUE_MAX_RECORDS,
            Self::SEED_CATALOGUE_MAX_BYTES,
        )
        .await
    }

    /// Same scan with explicit caps, so boundaries are testable without
    /// writing thousands of catalogue rows.
    pub async fn check_seed_catalogue_with_limits(
        &self,
        max_records: usize,
        max_bytes: usize,
    ) -> DbResult<()> {
        let repo = self.system_store.system_repo()?;
        let version = repo.consistent_version().await?;
        let mut bytes = 0usize;
        let mut records = 0usize;
        for name in repo.list_table_names() {
            let table = repo.get_table(&name).await?;
            let stream = table
                .mvcc_snapshot_stream(1, version)
                .ok_or_else(|| DbError::Storage("seed catalogue requires MVCC".into()))?;
            futures::pin_mut!(stream);
            while let Some(batch) = stream.next().await {
                for (key, value) in batch? {
                    bytes = bytes
                        .checked_add(key.as_ref().len())
                        .and_then(|n| n.checked_add(value.len()))
                        .ok_or_else(|| DbError::Storage("seed catalogue overflow".into()))?;
                    records += 1;
                    if bytes > max_bytes || records > max_records {
                        return Err(DbError::Storage("seed catalogue budget exceeded".into()));
                    }
                }
            }
        }
        if repo.consistent_version().await? != version {
            return Err(DbError::Storage(
                "seed catalogue changed during scan".into(),
            ));
        }
        Ok(())
    }

    /// Strict metadata durability for an offline seed; no clean checkpoint is
    /// required merely to flush the authoritative table/repository catalogue.
    pub async fn flush_seed_catalogue(&self) -> DbResult<()> {
        let repo = self.system_store.system_repo()?;
        for name in repo.list_table_names() {
            repo.get_table(&name).await?;
        }
        repo.flush_buffers().await?;
        repo.flush_all_history().await?;
        repo.flush_seed_metadata().await?;
        Ok(())
    }
}
