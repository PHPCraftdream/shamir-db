# Implementation Plan: Async Indexer with Optimized Unique Constraints

## Overview

This document describes the implementation plan for:
1. **Asynchronous index updates** via background thread
2. **Separated storage for unique indexes** for faster validation

## Goals

1. **Non-blocking writes**: Table insert/update/delete should not wait for index updates
2. **Fast unique constraint checking**: Skip validation entirely when no unique indexes exist
3. **Scalable architecture**: Single global indexer for all tables
4. **Memory-efficient**: Use streaming, not full table loads

---

## Architecture

### Component Overview

```
Table (insert/update/delete)
    |
    | 1. Check unique constraints (synchronous, fast)
    | 2. Write to data_store
    | 3. Send message to Indexer (non-blocking)
    v
mpsc::unbounded_channel
    |
    v
GlobalIndexer (background task)
    |
    | Reads messages
    | Processes sequentially per table
    | Updates index_store
    v
__idx__{table_name} (index storage)
```

### Storage Structure

```
// Unique indexes (separate for fast access)
RecordId::system("unique_indexes") -> Option<Vec<IndexDef>>

// Regular index configuration
RecordId::system("index_target") -> IndexTarget

// Index data storage
__idx__{table_name} -> index store
  Key: [path: u64, u64, ...][type: u8][hash1: u64][hash2: u64]
  Value: Vec<RecordId> (bincode)
```

---

## Step 1: Preparation (1-2 hours)

### 1.1 Create Communication Module

**File:** `src/db/engine/indexer/comm.rs` (new)

```rust
//! Indexer communication types

use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use crate::db::engine::index::OpType;

/// Message sent to global indexer
pub struct IndexMessage {
    pub table_name: String,
    pub op_type: OpType,
    pub record_id: RecordId,
    pub value: Option<UserValue>,
    // TODO: For Update, we need old_value too (Step 6)
}

impl IndexMessage {
    pub fn insert(table_name: String, id: RecordId, value: UserValue) -> Self {
        Self {
            table_name,
            op_type: OpType::Insert,
            record_id: id,
            value: Some(value),
        }
    }

    pub fn delete(table_name: String, id: RecordId) -> Self {
        Self {
            table_name,
            op_type: OpType::Delete,
            record_id: id,
            value: None,
        }
    }
}
```

### 1.2 Modify Table Structure

**File:** `src/db/engine/table.rs`

```rust
pub struct Table<R: Repo> {
    // ... existing fields ...

    /// Regular index configuration (for future queries)
    index_target: Arc<RwLock<IndexTarget>>,

    /// Unique indexes (separate for fast validation)
    unique_indexes: Arc<RwLock<Option<Vec<IndexDef>>>>,

    /// Channel sender for indexer messages
    indexer_tx: mpsc::UnboundedSender<IndexMessage>,
}
```

### 1.3 Update Table Constructor

```rust
impl<R: Repo> Table<R> {
    pub async fn new(repo: Arc<R>, table_name: String) -> DbResult<Self> {
        // ... existing code ...

        // Load index_target
        let index_target = Self::load_index_target(&info_store).await?
            .unwrap_or(IndexTarget::Disabled);

        // Load unique_indexes separately
        let unique_indexes = Self::load_unique_indexes(&info_store).await?;

        // TODO: Get sender from Repo (Step 5)
        let indexer_tx = todo!();

        Ok(Self {
            // ... existing fields ...
            index_target: Arc::new(RwLock::new(index_target)),
            unique_indexes: Arc::new(RwLock::new(unique_indexes)),
            indexer_tx,
        })
    }
}
```

### 1.4 Update Index Management Methods

```rust
impl<R: Repo> Table<R> {
    pub async fn add_index(&self, path: &[&str]) -> DbResult<()> {
        let interner = self.get_interner().await?;
        let interned_path: Vec<u64> = path.iter()
            .map(|&s| interner.touch_ind(s).val())
            .collect();

        // Update index_target only
        let mut target = self.index_target.write().await;
        target.add_index(interned_path.clone(), false);
        self.save_index_target(&target).await?;

        Ok(())
    }

    pub async fn add_unique_index(&self, path: &[&str]) -> DbResult<()> {
        let interner = self.get_interner().await?;
        let interned_path: Vec<u64> = path.iter()
            .map(|&s| interner.touch_ind(s).val())
            .collect();

        // 1. Validate existing data
        self.validate_unique_index(&interned_path, interner).await?;

        // 2. Update unique_indexes
        let mut unique = self.unique_indexes.write().await;
        match &mut *unique {
            Some(indexes) => {
                indexes.retain(|idx| idx.path != interned_path);
                indexes.push(IndexDef::unique(interned_path.clone()));
            }
            None => {
                *unique = Some(vec![IndexDef::unique(interned_path.clone())]);
            }
        }

        // 3. Update index_target (for future queries)
        let mut target = self.index_target.write().await;
        target.add_index(interned_path, true);
        self.save_index_target(&target).await?;

        // 4. Save unique_indexes separately
        self.save_unique_indexes(&unique).await?;

        Ok(())
    }

    pub async fn remove_index(&self, path: &[&str]) -> DbResult<bool> {
        let interner = self.get_interner().await?;
        let interned_path: Vec<u64> = path.iter()
            .map(|&s| interner.touch_ind(s).val())
            .collect();

        let mut removed = false;

        // Remove from unique_indexes if unique
        {
            let mut unique = self.unique_indexes.write().await;
            if let Some(indexes) = &mut *unique {
                let initial_len = indexes.len();
                indexes.retain(|idx| idx.path != interned_path);
                if indexes.len() < initial_len {
                    removed = true;
                    if indexes.is_empty() {
                        *unique = None;
                    }
                }
            }
        }

        // Remove from index_target
        {
            let mut target = self.index_target.write().await;
            if target.remove_index(&interned_path) {
                removed = true;
            }
            self.save_index_target(&target).await?;
        }

        // Save unique_indexes if changed
        if removed {
            let unique = self.unique_indexes.read().await;
            self.save_unique_indexes(&unique).await?;
        }

        Ok(removed)
    }
}
```

### 1.5 Add Storage Methods

```rust
impl<R: Repo> Table<R> {
    fn unique_indexes_key() -> RecordId {
        RecordId::system("unique_indexes")
    }

    pub async fn load_unique_indexes(
        info_store: &Arc<dyn Store>
    ) -> DbResult<Option<Vec<IndexDef>>> {
        let key_bytes = Bytes::copy_from_slice(Self::unique_indexes_key().as_bytes());
        match info_store.get(key_bytes).await {
            Ok(bytes) => {
                let indexes: Vec<IndexDef> = bincode::deserialize(&bytes)
                    .map_err(|e| DbError::Codec(format!("Failed to deserialize unique indexes: {}", e)))?;
                Ok(Some(indexes))
            }
            Err(DbError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn save_unique_indexes(&self, unique: &Option<Vec<IndexDef>>) -> DbResult<()> {
        let key_bytes = Bytes::copy_from_slice(Self::unique_indexes_key().as_bytes());

        match unique {
            Some(indexes) => {
                let bytes = bincode::serialize(indexes)
                    .map_err(|e| DbError::Codec(format!("Failed to serialize unique indexes: {}", e)))?;
                self.info_store.set(key_bytes, Bytes::from(bytes)).await?;
            }
            None => {
                self.info_store.remove(key_bytes).await?;
            }
        }

        Ok(())
    }
}
```

### 1.6 Optimize Unique Constraint Checking

```rust
impl<R: Repo> Table<R> {
    async fn check_unique_constraints(&self, value: &UserValue, interner: &Interner) -> DbResult<()> {
        // FAST PATH: No unique indexes
        let unique = self.unique_indexes.read().await;
        let unique_indexes = match &*unique {
            Some(indexes) => indexes.as_slice(),
            None => return Ok(()),  // <-- Skip entirely!
        };

        // Check each unique index
        for index_def in unique_indexes {
            if let Some(extracted) = Self::extract_value(value, &index_def.path, interner)? {
                self.check_value_unique_exclude(&index_def.path, &extracted, interner, None).await?;
            }
        }

        Ok(())
    }
}
```

### 1.7 Add Tests

```rust
#[tokio::test]
async fn test_separated_unique_indexes_storage() {
    // Create table
    let (table, _dir) = create_test_table().await.unwrap();

    // Add unique index
    table.add_unique_index(&["email"]).await.unwrap();

    // Verify it's in unique_indexes
    let unique = table.unique_indexes.read().await;
    assert!(unique.is_some());
    assert_eq!(unique.as_ref().unwrap().len(), 1);

    // Verify it's also in index_target
    let target = table.get_index_target().await;
    assert!(target.has_unique_index(&vec![1])); // interned ID
}

#[tokio::test]
async fn test_fast_path_no_unique_indexes() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Add regular index (not unique)
    table.add_index(&["name"]).await.unwrap();

    // unique_indexes should be None
    let unique = table.unique_indexes.read().await;
    assert!(unique.is_none());

    // Insert should not check uniqueness
    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("test".to_string()));
    table.insert(&UserValue::Map(data)).await.unwrap();

    // Second insert with same name should succeed (not unique)
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("test".to_string()));
    table.insert(&UserValue::Map(data2)).await.unwrap();

    assert_eq!(table.count().await.unwrap(), 2);
}
```

---

## Step 2: Hash Computation (1 hour)

### 2.1 Create Hash Module

**File:** `src/db/engine/indexer/hash.rs` (new)

```rust
//! Hash computation for index values

use crate::types::value::InnerValue;
use std::hash::{Hash, Hasher};
use twox_hash::XxHash64;
use fnv::FnvHasher;

/// Type discriminator for InnerValue
pub const TYPE_NULL: u8 = 0x00;
pub const TYPE_INT: u8 = 0x01;
pub const TYPE_UINT: u8 = 0x02;
pub const TYPE_FLOAT: u8 = 0x03;
pub const TYPE_BOOL: u8 = 0x04;
pub const TYPE_STR: u8 = 0x05;
pub const TYPE_BIN: u8 = 0x06;
pub const TYPE_ARRAY: u8 = 0x07;  // Not indexed
pub const TYPE_MAP: u8 = 0x08;
pub const TYPE_SET: u8 = 0x09;   // Not indexed
pub const TYPE_DECIMAL: u8 = 0x0A;
pub const TYPE_BIGINT: u8 = 0x0B;

/// Compute hash for index value
/// Returns Some((type, hash1, hash2)) or None if not indexable
pub fn compute_hash(value: &InnerValue) -> Option<(u8, u64, u64)> {
    match value {
        InnerValue::Null => Some((TYPE_NULL, 0, 0)),

        InnerValue::Int(n) => {
            let h1 = hash_int(*n);
            let h2 = hash_int(*n);
            Some((TYPE_INT, h1, h2))
        }

        InnerValue::UInt(n) => {
            let h1 = hash_uint(*n);
            let h2 = hash_uint(*n);
            Some((TYPE_UINT, h1, h2))
        }

        InnerValue::Float(f) => {
            let h1 = hash_float(*f);
            let h2 = hash_float(*f);
            Some((TYPE_FLOAT, h1, h2))
        }

        InnerValue::Bool(b) => {
            let h1 = hash_bool(*b);
            let h2 = hash_bool(*b);
            Some((TYPE_BOOL, h1, h2))
        }

        InnerValue::Str(s) => {
            let h1 = hash_bytes(s.as_bytes());
            let h2 = hash_bytes(s.as_bytes());
            Some((TYPE_STR, h1, h2))
        }

        InnerValue::Bin(bytes) => {
            let h1 = hash_bytes(bytes);
            let h2 = hash_bytes(bytes);
            Some((TYPE_BIN, h1, h2))
        }

        InnerValue::Decimal(d) => {
            let serialized = bincode::serialize(d).ok()?;
            let h1 = hash_bytes(&serialized);
            let h2 = hash_bytes(&serialized);
            Some((TYPE_DECIMAL, h1, h2))
        }

        InnerValue::BigInt(n) => {
            let serialized = bincode::serialize(n).ok()?;
            let h1 = hash_bytes(&serialized);
            let h2 = hash_bytes(&serialized);
            Some((TYPE_BIGINT, h1, h2))
        }

        InnerValue::Map(map) => {
            let serialized = bincode::serialize(map).ok()?;
            let h1 = hash_bytes(&serialized);
            let h2 = hash_bytes(&serialized);
            Some((TYPE_MAP, h1, h2))
        }

        InnerValue::Array(_) | InnerValue::Set(_) => None,  // Not indexed
    }
}

fn hash_int(n: i64) -> u64 {
    let mut h1 = XxHash64::default();
    n.hash(&mut h1);
    h1.finish()
}

fn hash_uint(n: u64) -> u64 {
    let mut h1 = XxHash64::default();
    n.hash(&mut h1);
    h1.finish()
}

fn hash_float(f: f64) -> u64 {
    let mut h1 = XxHash64::default();
    f.to_bits().hash(&mut h1);
    h1.finish()
}

fn hash_bool(b: bool) -> u64 {
    let mut h1 = XxHash64::default();
    b.hash(&mut h1);
    h1.finish()
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h1 = XxHash64::default();
    bytes.hash(&mut h1);
    h1.finish()
}
```

### 2.2 Add Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_int() {
        let val = InnerValue::Int(42);
        let (type_, h1, h2) = compute_hash(&val).unwrap();
        assert_eq!(type_, TYPE_INT);
        assert!(h1 > 0);
        assert!(h2 > 0);
    }

    #[test]
    fn test_hash_deterministic() {
        let val = InnerValue::Str("test".to_string());
        let (t1, h1a, h2a) = compute_hash(&val).unwrap();
        let (t2, h1b, h2b) = compute_hash(&val).unwrap();
        assert_eq!(t1, t2);
        assert_eq!(h1a, h1b);
        assert_eq!(h2a, h2b);
    }

    #[test]
    fn test_hash_map() {
        let mut map = HashMap::new();
        map.insert(1, InnerValue::Int(10));
        let val = InnerValue::Map(map);
        let result = compute_hash(&val);
        assert!(result.is_some());
    }

    #[test]
    fn test_hash_array_not_indexed() {
        let val = InnerValue::Array(vec![]);
        let result = compute_hash(&val);
        assert!(result.is_none());
    }
}
```

---

## Step 3: Key Encoding (30 min)

### 3.1 Create Encoding Module

**File:** `src/db/engine/indexer/encoding.rs` (new)

```rust
//! Index key encoding/decoding

use bytes::Bytes;

/// Encode index key from components
pub fn encode_index_key(
    path: &[u64],
    value_type: u8,
    hash1: u64,
    hash2: u64,
) -> Result<Bytes, String> {
    let mut key = Vec::with_capacity(path.len() * 8 + 17);

    // Path components (big-endian)
    for &component in path {
        key.extend_from_slice(&component.to_be_bytes());
    }

    // Type + hashes (fixed 17 bytes)
    key.push(value_type);
    key.extend_from_slice(&hash1.to_be_bytes());
    key.extend_from_slice(&hash2.to_be_bytes());

    Ok(Bytes::from(key))
}

/// Decode index key
pub fn decode_index_key(key: &[u8]) -> Result<(Vec<u64>, u8, u64, u64), String> {
    if key.len() < 17 {
        return Err("Key too short".to_string());
    }

    let tail_offset = key.len() - 17;

    // Read tail
    let value_type = key[tail_offset];
    let hash1 = u64::from_be_bytes(
        key[tail_offset + 1..tail_offset + 9]
            .try_into()
            .map_err(|_| "Invalid hash1".to_string())?
    );
    let hash2 = u64::from_be_bytes(
        key[tail_offset + 9..tail_offset + 17]
            .try_into()
            .map_err(|_| "Invalid hash2".to_string())?
    );

    // Read path
    let path_bytes = &key[0..tail_offset];
    if path_bytes.len() % 8 != 0 {
        return Err("Invalid path length".to_string());
    }

    let path: Vec<u64> = path_bytes
        .chunks(8)
        .map(|chunk| {
            u64::from_be_bytes(
                chunk.try_into().expect("Invalid chunk")
            )
        })
        .collect();

    Ok((path, value_type, hash1, hash2))
}
```

### 3.2 Add Tests

```rust
#[test]
fn test_encode_decode_roundtrip() {
    let path = vec![1, 2, 3];
    let encoded = encode_index_key(&path, 0x01, 100, 200).unwrap();
    let (decoded_path, type_, h1, h2) = decode_index_key(&encoded).unwrap();

    assert_eq!(decoded_path, path);
    assert_eq!(type_, 0x01);
    assert_eq!(h1, 100);
    assert_eq!(h2, 200);
}
```

---

## Step 4: Index Store Operations (1 hour)

### 4.1 Create Store Module

**File:** `src/db/engine/indexer/store.rs` (new)

```rust
//! Index store operations

use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;
use crate::db::error::{DbError, DbResult};
use bytes::Bytes;
use std::sync::Arc;

/// Add record ID to index
pub async fn add_to_index(
    store: &Arc<dyn Store>,
    key: Bytes,
    record_id: RecordId,
) -> DbResult<()> {
    // Get existing IDs
    let existing = match store.get(key.clone()).await {
        Ok(bytes) => {
            bincode::deserialize(&bytes)
                .unwrap_or_default()
        }
        Err(DbError::NotFound(_)) => Vec::new(),
        Err(e) => return Err(e),
    };

    // Add new ID
    let mut ids: Vec<RecordId> = existing;
    ids.push(record_id);

    // Serialize and save
    let serialized = bincode::serialize(&ids)
        .map_err(|e| DbError::Codec(format!("Failed to serialize: {}", e)))?;
    store.set(key, Bytes::from(serialized)).await?;

    Ok(())
}

/// Remove record ID from index
pub async fn remove_from_index(
    store: &Arc<dyn Store>,
    key: Bytes,
    record_id: RecordId,
) -> DbResult<bool> {
    // Get existing IDs
    let existing = match store.get(key.clone()).await {
        Ok(bytes) => {
            bincode::deserialize(&bytes)
                .unwrap_or_default()
        }
        Err(DbError::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };

    // Remove ID
    let mut ids: Vec<RecordId> = existing;
    let initial_len = ids.len();
    ids.retain(|id| *id != record_id);

    if ids.len() == initial_len {
        return Ok(false);  // Not found
    }

    // Save or delete
    if ids.is_empty() {
        store.remove(key).await?;
    } else {
        let serialized = bincode::serialize(&ids)
            .map_err(|e| DbError::Codec(format!("Failed to serialize: {}", e)))?;
        store.set(key, Bytes::from(serialized)).await?;
    }

    Ok(true)
}

/// Find all record IDs for a key
pub async fn find_in_index(
    store: &Arc<dyn Store>,
    key: Bytes,
) -> DbResult<Vec<RecordId>> {
    match store.get(key).await {
        Ok(bytes) => {
            bincode::deserialize(&bytes)
                .map_err(|e| DbError::Codec(format!("Failed to deserialize: {}", e)))
        }
        Err(DbError::NotFound(_)) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
```

---

## Step 5: Global Indexer (2 hours)

### 5.1 Create Indexer Module

**File:** `src/db/engine/indexer/mod.rs` (new)

```rust
//! Global asynchronous index updater

use crate::db::engine::index::IndexTarget;
use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;
use crate::db::error::{DbError, DbResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, RwLock};

pub mod comm;
pub mod hash;
pub mod encoding;
pub mod store;

use comm::IndexMessage;

/// Table index state
pub struct TableIndexState {
    pub index_store: Arc<dyn Store>,
    pub index_target: IndexTarget,
}

/// Global indexer
pub struct GlobalIndexer {
    receiver: mpsc::UnboundedReceiver<IndexMessage>,
    tables: Arc<RwLock<HashMap<String, TableIndexState>>>,
    shutdown: AtomicBool,
}

impl GlobalIndexer {
    pub fn new(
        receiver: mpsc::UnboundedReceiver<IndexMessage>,
    ) -> Self {
        Self {
            receiver,
            tables: Arc::new(RwLock::new(HashMap::new())),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Register a table for indexing
    pub async fn register_table(
        &self,
        table_name: String,
        index_store: Arc<dyn Store>,
        index_target: IndexTarget,
    ) {
        let mut tables = self.tables.write().await;
        tables.insert(table_name, TableIndexState {
            index_store,
            index_target,
        });
    }

    /// Run indexer loop
    pub async fn run(self: Arc<Self>) {
        log::info!("GlobalIndexer started");

        loop {
            tokio::select! {
                // Process next message
                msg = self.receiver.recv() => {
                    match msg {
                        Some(msg) => {
                            if let Err(e) = self.process_message(msg).await {
                                log::error!("Indexer error: {}", e);
                            }
                        }
                        None => {
                            log::info!("Indexer channel closed");
                            break;
                        }
                    }
                }
            }

            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }
        }

        log::info!("GlobalIndexer shutdown complete");
    }

    async fn process_message(&self, msg: IndexMessage) -> DbResult<()> {
        let tables = self.tables.read().await;
        let state = tables.get(&msg.table_name)
            .ok_or_else(|| DbError::Internal(format!("Table not found: {}", msg.table_name)))?;

        match &state.index_target {
            IndexTarget::Disabled => Ok(()),
            IndexTarget::All => self.process_all(state, &msg).await,
            IndexTarget::Selective(indexes) => self.process_selective(state, indexes, &msg).await,
        }
    }

    async fn process_selective(
        &self,
        state: &TableIndexState,
        indexes: &[crate::db::engine::index::IndexDef],
        msg: &IndexMessage,
    ) -> DbResult<()> {
        if let Some(value) = &msg.value {
            if let UserValue::Map(map) = value {
                for index_def in indexes {
                    // TODO: Extract value by path and update index
                    // This requires path extraction logic
                }
            }
        }
        Ok(())
    }

    async fn process_all(&self, _state: &TableIndexState, _msg: &IndexMessage) -> DbResult<()> {
        // TODO: Implement full indexing
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
```

---

## Step 6: Integration with Table (1 hour)

### 6.1 Send Messages from Operations

```rust
impl<R: Repo> Table<R> {
    async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        // 1. Check unique constraints (fast path)
        let interner = self.get_interner().await?;
        self.check_unique_constraints(value, interner).await?;

        // 2. Transform and save
        let transform = transform::user_to_inner(value, interner);
        if let Some(ref new_keys) = transform.new_keys {
            self.save_new_keys(new_keys).await?;
        }

        let inner_bytes = transform.inner_value.to_bytes();
        let key_bytes = self.data_store.insert(inner_bytes).await?;
        self.increment_record_count(1).await?;

        let arr: [u8; 16] = key_bytes.as_ref().try_into()
            .map_err(|_| DbError::Internal("Failed to convert key bytes to RecordId".to_string()))?;
        let id = RecordId(arr);

        // 3. Send message to indexer (non-blocking)
        let _ = self.indexer_tx.send(IndexMessage::insert(
            self.table_name.clone(),
            id,
            value.clone(),
        ));

        Ok(id)
    }

    async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // 1. Delete from data store
        let key_bytes = Bytes::copy_from_slice(id.as_bytes());
        let removed = self.data_store.remove(key_bytes.clone()).await?;

        if removed {
            self.increment_record_count(-1).await?;

            // 2. Send message to indexer
            let _ = self.indexer_tx.send(IndexMessage::delete(
                self.table_name.clone(),
                id,
            ));
        }

        Ok(removed)
    }
}
```

---

## Step 7: Update Operations (Future Enhancement)

### The Problem with Update

For Update, we need to:
1. Remove old value from index
2. Add new value to index

But we don't have the old value!

### Solutions

**Option A:** Read old value before update
```rust
async fn update(&self, id: RecordId, value: &UserValue) -> DbResult<bool> {
    // Read old value
    let old_value = self.get(id).await.ok();

    // Check unique constraints
    self.check_unique_constraints_exclude(value, interner, Some(id)).await?;

    // Update data store
    // ...

    // Send both old and new to indexer
    let _ = self.indexer_tx.send(IndexMessage::update(
        self.table_name.clone(),
        id,
        old_value,
        Some(value.clone()),
    ));
}
```

**Option B:** Scan index for record_id (slower)
```rust
// In indexer:
async fn process_update(&self, msg: IndexMessage) -> DbResult<()> {
    // Find all keys for this record_id
    let keys = scan_index_for_record_id(&state.index_store, msg.record_id).await?;

    // Remove old entries
    for key in keys {
        remove_from_index(&state.index_store, key, msg.record_id).await?;
    }

    // Add new entries
    if let Some(value) = msg.value {
        add_indexes_for_value(&state, value, msg.record_id).await?;
    }
}
```

**Recommendation:** Start with Option A (simpler), optimize later if needed.

---

## Summary

### Timeline

- **Step 1:** 1-2 hours (separated storage, fast path)
- **Step 2:** 1 hour (hash computation)
- **Step 3:** 30 min (key encoding)
- **Step 4:** 1 hour (index store)
- **Step 5:** 2 hours (global indexer)
- **Step 6:** 1 hour (integration)

**Total: 6.5 - 7.5 hours**

### Priority

1. **High Priority:** Step 1 (separated unique indexes) - immediate performance benefit
2. **Medium Priority:** Steps 2-4 (hash, encoding, store) - foundation
3. **Lower Priority:** Steps 5-6 (async indexer) - can be added incrementally

### Testing Strategy

1. Unit tests for each module
2. Integration tests for indexer flow
3. Performance tests for unique constraint checking
4. Stress tests for concurrent operations
