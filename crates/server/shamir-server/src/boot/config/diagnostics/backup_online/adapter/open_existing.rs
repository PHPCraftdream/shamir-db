//! Existing-store open that does not initialize metadata or update startup state.

use std::path::Path;
use std::sync::Arc;

use fjall::{Database, KeyspaceCreateOptions};
use parking_lot::Mutex;

use super::{MetaError, ServerMetaStore, KEY_SECRETS, META_KEYSPACE};

/// Open an existing store without initializing missing metadata or updating startup state.
pub(crate) fn open_existing(path: impl AsRef<Path>) -> Result<ServerMetaStore, MetaError> {
    let db = Database::builder(path.as_ref()).open()?;
    let meta = db.keyspace(META_KEYSPACE, KeyspaceCreateOptions::default)?;
    let store = ServerMetaStore {
        db: Arc::new(db),
        meta,
        write_lock: Mutex::new(()),
    };
    store
        .meta
        .get(KEY_SECRETS.as_bytes())?
        .ok_or(MetaError::NotInitialised(KEY_SECRETS))?;
    Ok(store)
}

/// Test-only: force a durable LSM version switch through the store's own handle.
#[cfg(test)]
pub(crate) fn rotate_meta_memtable_for_test(store: &ServerMetaStore) -> Result<(), MetaError> {
    store
        .meta
        .rotate_memtable_and_wait()
        .map_err(MetaError::Fjall)
}
