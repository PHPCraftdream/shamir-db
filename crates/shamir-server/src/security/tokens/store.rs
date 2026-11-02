use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use shamir_connect::server::token::{TokenId, TokenRecord};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const TOKEN_KEYSPACE: &str = "api_tokens_v1";

/// Durable token-store errors.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// Underlying fjall failure.
    #[error("token storage unavailable")]
    Fjall(#[from] fjall::Error),
    /// Tokio blocking task failed to join.
    #[error("token storage task failed")]
    Join,
    /// Named MessagePack encode/decode failure.
    #[error("token record encoding failed")]
    Encoding(#[from] rmp_serde::encode::Error),
    /// Named MessagePack decoding failure.
    #[error("token record decoding failed")]
    Decoding(#[from] rmp_serde::decode::Error),
    /// Injected failure requested by the test seam, consumed by the next durable mutation.
    #[error("injected token-store durable-write failure")]
    InjectedWriteFailure,
}

struct StoreInner {
    db: Database,
    records: Keyspace,
    path: PathBuf,
    corrupt_records: AtomicU64,
    fail_next_durable_write: AtomicBool,
    // Admin-op guard (create/revoke/GC/scan on the blocking pool), contention is nil: never held
    // across an await and never on the verify path, which reads the lock-free mirror.
    operation_lock: Mutex<()>,
}

/// Shareable fjall token persistence backend.
#[derive(Clone)]
pub struct TokenStore(Arc<StoreInner>);

impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore")
            .field("path", &self.0.path)
            .field("keyspace", &TOKEN_KEYSPACE)
            .finish()
    }
}

impl TokenStore {
    /// Open or create `<data_dir>/api_tokens` on a blocking thread.
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self, TokenStoreError> {
        let path = data_dir.as_ref().join("api_tokens");
        tokio::task::spawn_blocking(move || Self::open_blocking(path))
            .await
            .map_err(|_| TokenStoreError::Join)?
    }

    fn open_blocking(path: PathBuf) -> Result<Self, TokenStoreError> {
        let db = Database::builder(&path).open()?;
        let records = db.keyspace(TOKEN_KEYSPACE, KeyspaceCreateOptions::default)?;
        Ok(Self(Arc::new(StoreInner {
            db,
            records,
            path,
            corrupt_records: AtomicU64::new(0),
            fail_next_durable_write: AtomicBool::new(false),
            operation_lock: Mutex::new(()),
        })))
    }

    /// Scan records, warning with token ID only and skipping corrupt/invalid values.
    pub async fn scan(&self) -> Result<Vec<TokenRecord>, TokenStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.scan_blocking())
            .await
            .map_err(|_| TokenStoreError::Join)?
    }

    pub(crate) fn scan_blocking(&self) -> Result<Vec<TokenRecord>, TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut records = Vec::new();
        for item in self.0.records.iter() {
            let (key, value) = item.into_inner().map_err(|error| {
                tracing::warn!(error = %error, "API token scan item failed");
                TokenStoreError::Join
            })?;
            let id = if key.len() == 16 {
                hex::encode(&key)
            } else {
                "invalid-key".to_string()
            };
            let record: TokenRecord = match rmp_serde::from_slice(value.as_ref()) {
                Ok(record) => record,
                Err(_) => {
                    self.0.corrupt_records.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(token_id = %id, "skipping corrupt API token record");
                    continue;
                }
            };
            if key.len() != 16
                || record.id.as_slice() != key.as_ref()
                || record.validate_shape().is_err()
            {
                self.0.corrupt_records.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(token_id = %id, "skipping invalid API token record");
                continue;
            }
            records.push(record);
        }
        Ok(records)
    }

    /// Read one record on a blocking thread.
    pub async fn get(&self, id: TokenId) -> Result<Option<TokenRecord>, TokenStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.get_blocking(&id))
            .await
            .map_err(|_| TokenStoreError::Join)?
    }

    pub(crate) fn get_blocking(
        &self,
        id: &TokenId,
    ) -> Result<Option<TokenRecord>, TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = self.0.records.get(id.0)?;
        match bytes {
            Some(bytes) => {
                let record: TokenRecord = rmp_serde::from_slice(&bytes)?;
                if record.id != id.0 || record.validate_shape().is_err() {
                    self.0.corrupt_records.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(token_id = %hex::encode(id.0), "skipping invalid API token record");
                    Ok(None)
                } else {
                    Ok(Some(record))
                }
            }
            None => Ok(None),
        }
    }

    /// Test-only seam shared by clones; production starts with failure injection disabled.
    #[cfg(test)]
    pub(crate) fn fail_next_durable_write(&self) {
        self.0
            .fail_next_durable_write
            .store(true, Ordering::Release);
    }

    fn check_write_fault(&self) -> Result<(), TokenStoreError> {
        if self.0.fail_next_durable_write.swap(false, Ordering::AcqRel) {
            Err(TokenStoreError::InjectedWriteFailure)
        } else {
            Ok(())
        }
    }

    /// Insert only when the ID is absent; checks and writes under one store lock.
    pub(crate) fn put_if_absent_blocking(
        &self,
        record: &TokenRecord,
    ) -> Result<bool, TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.0.records.get(record.id)?.is_some() {
            return Ok(false);
        }
        self.write_record(&self.0.db, &self.0.records, record)?;
        Ok(true)
    }

    /// Store a record durably before resolving.
    pub async fn put(&self, record: TokenRecord) -> Result<(), TokenStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.put_blocking(&record))
            .await
            .map_err(|_| TokenStoreError::Join)?
    }

    fn put_blocking(&self, record: &TokenRecord) -> Result<(), TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.write_record(&self.0.db, &self.0.records, record)
    }

    fn write_record(
        &self,
        db: &Database,
        records: &Keyspace,
        record: &TokenRecord,
    ) -> Result<(), TokenStoreError> {
        self.check_write_fault()?;
        let bytes = rmp_serde::to_vec_named(record)?;
        let mut batch = db.batch().durability(Some(PersistMode::SyncAll));
        batch.insert(records, record.id, bytes.as_slice());
        batch.commit()?;
        Ok(())
    }

    /// Delete a record durably before resolving.
    pub async fn delete(&self, id: TokenId) -> Result<(), TokenStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.delete_blocking(&id))
            .await
            .map_err(|_| TokenStoreError::Join)?
    }

    pub(crate) fn delete_blocking(&self, id: &TokenId) -> Result<(), TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check_write_fault()?;
        Self::delete_locked(&self.0.db, &self.0.records, id)
    }

    fn delete_locked(
        db: &Database,
        records: &Keyspace,
        id: &TokenId,
    ) -> Result<(), TokenStoreError> {
        let mut batch = db.batch().durability(Some(PersistMode::SyncAll));
        batch.remove(records, id.0);
        batch.commit()?;
        Ok(())
    }

    /// Count corrupt values skipped by full scans since open.
    pub fn corrupt_records_hint(&self) -> u64 {
        self.0.corrupt_records.load(Ordering::Relaxed)
    }

    pub(crate) fn with_locked_records<T>(
        &self,
        operation: impl FnOnce(&Database, &Keyspace) -> Result<T, TokenStoreError>,
    ) -> Result<T, TokenStoreError> {
        let _guard = self
            .0
            .operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation(&self.0.db, &self.0.records)
    }

    /// Check the next-mutation fault while the caller holds the operation lock.
    pub(crate) fn fail_locked_write(&self) -> Result<(), TokenStoreError> {
        self.check_write_fault()
    }

    /// Scan valid records while the caller holds the store operation lock.
    pub(crate) fn scan_locked(&self) -> Result<Vec<TokenRecord>, TokenStoreError> {
        let mut records = Vec::new();
        for item in self.0.records.iter() {
            let (key, value) = item.into_inner().map_err(|error| {
                tracing::warn!(error = %error, "API token scan item failed");
                TokenStoreError::Join
            })?;
            let record = match rmp_serde::from_slice::<TokenRecord>(value.as_ref()) {
                Ok(record) => record,
                Err(_) => {
                    self.0.corrupt_records.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(token_id = %hex::encode(&key), "skipping corrupt API token record");
                    continue;
                }
            };
            if key.len() == 16
                && record.id.as_slice() == key.as_ref()
                && record.validate_shape().is_ok()
            {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub(crate) fn write_locked(
        &self,
        db: &Database,
        records: &Keyspace,
        record: &TokenRecord,
    ) -> Result<(), TokenStoreError> {
        self.write_record(db, records, record)
    }
}
