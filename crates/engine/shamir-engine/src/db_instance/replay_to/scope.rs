//! In-memory route validation for staged WAL, before any store is opened.

use shamir_collections::{TFxMap, TFxSet};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::table::{table_store_names, table_token_for, TableConfig};
use crate::tx::recovery::is_supported_posting_key;

/// Authoritative table configurations for one staged repository, supplied by
/// the caller from the same catalogue as normal boot. Keep each physical
/// `store_id` unchanged; `None` is the legacy name-based mapping.
///
/// `TableConfig` does not carry persisted index identity. Validate that
/// identity after opening the table, before relying on it for replay.
#[derive(Debug)]
pub struct ReplayScope {
    configs: Vec<TableConfig>,
}

/// A replay route could not be established without opening a store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayScopeError {
    #[error("table {table:?} maps to invalid store name {store:?}: expected 1..=255 UTF-8 bytes")]
    InvalidStoreName { table: String, store: String },
    #[error("table {table:?} maps to a store already in replay scope: {store:?}")]
    DuplicateStore { table: String, store: String },
    #[error("table {table:?} hashes to reserved table token zero")]
    ReservedTableToken { table: String },
    #[error("tables {first:?} and {second:?} share replay token {token}")]
    DuplicateTableToken {
        token: u64,
        first: String,
        second: String,
    },
    #[error("WAL transaction {txn_id} requires a nonempty table scope")]
    EmptyTableScope { txn_id: u64 },
    #[error("WAL transaction {txn_id} has no replay route for table token {token}")]
    MissingTableRoute { txn_id: u64, token: u64 },
    #[error("WAL transaction {txn_id} has no provable index route for table token {token}, idx_id {idx_id}")]
    MissingIndexRoute {
        txn_id: u64,
        token: u64,
        idx_id: u32,
    },
}

impl ReplayScope {
    /// Carry the caller's normal-boot catalogue configurations without changing them.
    pub fn new(configs: Vec<TableConfig>) -> Self {
        Self { configs }
    }

    /// Validate the catalogue routes and every staged operation without I/O.
    /// Staging separately validates the entry-level repo interner scope marker.
    pub(super) fn validate(&self, entries: &[WalEntryV2]) -> Result<(), ReplayScopeError> {
        let mut tables = TFxMap::default();
        let mut stores = TFxSet::default();
        for config in &self.configs {
            let token = table_token_for(&config.name);
            if token == 0 {
                return Err(ReplayScopeError::ReservedTableToken {
                    table: config.name.clone(),
                });
            }
            if let Some(first) = tables.insert(token, config.name.as_str()) {
                return Err(ReplayScopeError::DuplicateTableToken {
                    token,
                    first: first.to_owned(),
                    second: config.name.clone(),
                });
            }

            let names = table_store_names(&config.name, config.store_id.as_ref());
            for store in [names.data, names.info, names.history] {
                // Same UTF-8 byte limit as FjallRepo, including legacy names.
                if store.is_empty() || store.len() > usize::from(u8::MAX) {
                    return Err(ReplayScopeError::InvalidStoreName {
                        table: config.name.clone(),
                        store,
                    });
                }
                if !stores.insert(store.clone()) {
                    return Err(ReplayScopeError::DuplicateStore {
                        table: config.name.clone(),
                        store,
                    });
                }
            }
        }

        for entry in entries {
            for op in &entry.ops {
                validate_op(&tables, entry.txn_id, op)?;
            }
        }
        Ok(())
    }

    /// Hand off the original configs after scope validation, for offline open.
    pub(super) fn into_configs(self) -> Vec<TableConfig> {
        self.configs
    }
}

fn validate_op(
    tables: &TFxMap<u64, &str>,
    txn_id: u64,
    op: &WalOpV2,
) -> Result<(), ReplayScopeError> {
    // Validate the counter's table even though cold recovery skips the delta.
    let (token, broadcast) = match *op {
        WalOpV2::Put {
            table_id_interned, ..
        }
        | WalOpV2::Delete {
            table_id_interned, ..
        }
        | WalOpV2::CounterDelta {
            table_id_interned, ..
        } => (table_id_interned, false),
        WalOpV2::IndexPut {
            table_id_interned,
            idx_id,
            ref key,
            ..
        }
        | WalOpV2::IndexDel {
            table_id_interned,
            idx_id,
            ref key,
            ..
        } => {
            let broadcast = table_id_interned == 0;
            if broadcast && tables.is_empty() {
                return Err(ReplayScopeError::EmptyTableScope { txn_id });
            }
            // WalOpV2 reserves idx_id at zero; the literal key goes to info_store.
            // Preflight checks only the key layout. Table open loads the definitions
            // needed to prove membership in a particular index.
            if idx_id != 0 || !is_supported_posting_key(key) {
                return Err(ReplayScopeError::MissingIndexRoute {
                    txn_id,
                    token: table_id_interned,
                    idx_id,
                });
            }
            (table_id_interned, broadcast)
        }
        WalOpV2::InternerOverlayMerge { .. } => return Ok(()),
    };
    if tables.is_empty() {
        return Err(ReplayScopeError::EmptyTableScope { txn_id });
    }
    if !broadcast && !tables.contains_key(&token) {
        return Err(ReplayScopeError::MissingTableRoute { txn_id, token });
    }
    Ok(())
}
