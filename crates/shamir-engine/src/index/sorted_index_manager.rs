//! Sorted (B-tree-by-value) index manager.
//!
//! Parallel to the hash-based `IndexManager`. Where hash indexes
//! answer **equality** lookups (`field == value`), sorted indexes
//! answer **range / order / min** queries by encoding the indexed
//! value into bytes that sort the same way the value does (see
//! `shamir_types::core::sort_codec`) and storing one info-store
//! record per `(value, record_id)` pair.
//!
//! Layout per entry in info_store:
//!
//! ```text
//!   physical_key  = SORTED_TAG (1 byte)
//!                 ||  name_interned (8 bytes BE)
//!                 ||  encoded_value (variable)
//!                 ||  record_id (16 bytes)
//!   physical_value = empty Bytes
//! ```
//!
//! `SORTED_TAG` is chosen to be distinct from the hash-index tag so
//! the two indexes never collide in the same info_store. Within one
//! `name_interned`, all entries share that prefix, so a prefix scan
//! returns every record matching this index in **value order** —
//! that's the whole point.
//!
//! What's supported in this first cut:
//! - Single-field index over a scalar column (Int / Float / String /
//!   Bool / U64).
//! - Range queries: between / gt / gte / lt / lte.
//! - `order by field asc + limit K` (forward scan, stop after K).
//! - `min(field)` (first record from prefix scan).
//!
//! Not yet:
//! - `max(field)`, `order by desc` — needs reverse iteration on the
//!   Store trait (next).
//! - Composite sorted index over multiple columns.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_types::core::sort_codec;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// Distinguishes sorted-index physical keys from any other key kind
/// that lives in the same info_store. Must NOT collide with
/// `IndexRecordKey::TAG` (see index_record_key.rs) or any system
/// RecordId byte pattern. RecordId::system uses a 4-byte zero prefix
/// followed by name bytes — first byte is 0x00. Hash-index keys
/// start with the unique flag (0 or 1). So 0x80 is a safe pick.
const SORTED_TAG: u8 = 0x80;

/// Definition of a sorted index — minimal, since we only support
/// single-field for now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortedIndexDefinition {
    /// Interned id of the index name.
    pub name_interned: u64,
    /// Single field path, expressed as interner keys (matches the
    /// regular `IndexInfoItem::path`).
    pub field_path: Vec<u64>,
}

impl SortedIndexDefinition {
    pub fn new(name_interned: u64, field_path: Vec<u64>) -> Self {
        Self { name_interned, field_path }
    }
}

/// Manages a set of sorted indexes for one table. The set itself is
/// kept in memory (`DashMap`) and persisted to a single system
/// record key under `RecordId::system("sorted_indexes")` so we can
/// reload on restart.
pub struct SortedIndexManager {
    info_store: Arc<dyn Store>,
    /// `name_interned → definition`
    indexes: Arc<DashMap<u64, SortedIndexDefinition>>,
}

impl Clone for SortedIndexManager {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
        }
    }
}

impl SortedIndexManager {
    /// Construct empty; caller must `load()` to hydrate.
    pub async fn new(info_store: Arc<dyn Store>) -> DbResult<Self> {
        let m = Self {
            info_store,
            indexes: Arc::new(DashMap::new()),
        };
        m.load().await?;
        Ok(m)
    }

    /// True if at least one sorted index exists.
    pub fn has_indexes(&self) -> bool {
        !self.indexes.is_empty()
    }

    /// Iterate over all sorted-index definitions.
    pub fn iter_indexes(&self) -> Vec<SortedIndexDefinition> {
        self.indexes.iter().map(|e| e.value().clone()).collect()
    }

    /// Look up a definition whose `field_path` matches.
    pub fn find_by_field(&self, field_path: &[u64]) -> Option<SortedIndexDefinition> {
        self.indexes
            .iter()
            .find(|e| e.value().field_path == field_path)
            .map(|e| e.value().clone())
    }

    /// Register a new sorted index. Persists the updated definitions
    /// blob, but does NOT backfill — the caller scans the table and
    /// calls `insert_entry` for each existing record.
    pub async fn register(&self, def: SortedIndexDefinition) -> DbResult<()> {
        self.indexes.insert(def.name_interned, def);
        self.persist_defs().await
    }

    /// Drop a sorted index definition AND every entry written under
    /// it. O(I) where I is the size of the index.
    pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
        let existed = self.indexes.remove(&name_interned).is_some();
        if !existed {
            return Ok(false);
        }
        // Sweep entries.
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, 256);
        futures::pin_mut!(stream);
        let mut to_drop: Vec<Bytes> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                to_drop.push(k);
            }
        }
        for k in to_drop {
            self.info_store.remove(k).await?;
        }
        self.persist_defs().await?;
        Ok(true)
    }

    /// Add an index entry for a record. Called from
    /// `TableManager::insert` and `set` (create branch).
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        record: &InnerValue,
    ) -> DbResult<()> {
        if self.indexes.is_empty() {
            return Ok(());
        }
        // Snapshot definitions so we don't hold the DashMap shard.
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                self.info_store.set(key, Bytes::new()).await?;
            }
        }
        Ok(())
    }

    /// Update entries when a record changes.
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> DbResult<()> {
        if self.indexes.is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        for def in &defs {
            let old_enc = extract_and_encode(old, &def.field_path)?;
            let new_enc = extract_and_encode(new, &def.field_path)?;
            if old_enc == new_enc {
                continue;
            }
            if let Some(ref ov) = old_enc {
                let key = self.build_entry_key(def.name_interned, ov, record_id);
                let _ = self.info_store.remove(key).await?;
            }
            if let Some(ref nv) = new_enc {
                let key = self.build_entry_key(def.name_interned, nv, record_id);
                self.info_store.set(key, Bytes::new()).await?;
            }
        }
        Ok(())
    }

    /// Drop entries for a deleted record.
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        record: &InnerValue,
    ) -> DbResult<()> {
        if self.indexes.is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                let _ = self.info_store.remove(key).await?;
            }
        }
        Ok(())
    }

    /// Range lookup: return all record IDs whose indexed value is in
    /// `[start, end]` (both inclusive). `start` / `end` are the
    /// already-encoded value bytes (call sites use
    /// `sort_codec::encode_*` to produce them).
    pub async fn lookup_range(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<BTreeSet<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix.clone(), 256);
        futures::pin_mut!(stream);

        // Bounds in the same physical-key space.
        let lower_bound = start_encoded.map(|enc| {
            let mut k = prefix.to_vec();
            k.extend_from_slice(enc);
            k
        });
        let upper_bound = end_encoded.map(|enc| {
            let mut k = prefix.to_vec();
            k.extend_from_slice(enc);
            // Append max-suffix so all record_id tiebreakers for the
            // upper-bound value are included.
            k.extend_from_slice(&[0xFFu8; 16]);
            k
        });

        let mut out: BTreeSet<RecordId> = BTreeSet::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                let kb: &[u8] = k.as_ref();
                if let Some(ref lo) = lower_bound {
                    if kb.as_ref() < lo.as_slice() {
                        continue;
                    }
                }
                if let Some(ref hi) = upper_bound {
                    if kb.as_ref() > hi.as_slice() {
                        // Sorted scan — once we cross the upper bound
                        // we're done.
                        return Ok(out);
                    }
                }
                if let Some(id) = decode_record_id_suffix(kb) {
                    out.insert(id);
                }
            }
        }
        Ok(out)
    }

    /// Min lookup — the first record under the sorted prefix.
    pub async fn lookup_min(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// First K record ids under the sorted prefix, in value-asc order.
    pub async fn lookup_first_k(
        &self,
        name_interned: u64,
        k: usize,
    ) -> DbResult<Vec<RecordId>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, k.min(256));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    // ----- internals --------------------------------------------------------

    /// Prefix common to every entry of one sorted index.
    fn entry_prefix(&self, name_interned: u64) -> Bytes {
        let mut buf = Vec::with_capacity(9);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        Bytes::from(buf)
    }

    /// Full entry key for one (value, record_id) pair.
    fn build_entry_key(
        &self,
        name_interned: u64,
        encoded_value: &[u8],
        record_id: &RecordId,
    ) -> Bytes {
        let mut buf = Vec::with_capacity(1 + 8 + encoded_value.len() + 16);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        buf.extend_from_slice(encoded_value);
        buf.extend_from_slice(&record_id.to_bytes());
        Bytes::from(buf)
    }

    async fn persist_defs(&self) -> DbResult<()> {
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let bytes = bincode::serialize(&defs).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!(
                "sorted-index defs encode: {e}"
            ))
        })?;
        let sys_id = shamir_types::types::record_id::RecordId::system("sorted_indexes");
        self.info_store
            .set(sys_id.to_bytes(), Bytes::from(bytes))
            .await?;
        Ok(())
    }

    async fn load(&self) -> DbResult<()> {
        let sys_id = shamir_types::types::record_id::RecordId::system("sorted_indexes");
        let bytes = match self.info_store.get(sys_id.to_bytes()).await {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = bincode::deserialize(bytes.as_ref())
            .map_err(|e| {
                shamir_storage::error::DbError::Codec(format!(
                    "sorted-index defs decode: {e}"
                ))
            })?;
        for d in defs {
            self.indexes.insert(d.name_interned, d);
        }
        Ok(())
    }
}

/// Extract the value at `field_path` from a record and encode it via
/// `sort_codec`. Returns `None` if the field is missing or has a type
/// we don't index (we intentionally skip such records — they won't
/// surface in sorted lookups).
fn extract_and_encode(
    record: &InnerValue,
    field_path: &[u64],
) -> DbResult<Option<Vec<u8>>> {
    let Some(val) = resolve_path(record, field_path) else {
        return Ok(None);
    };
    let mut buf = Vec::new();
    match val {
        InnerValue::Null => sort_codec::encode_null(&mut buf),
        InnerValue::Bool(b) => sort_codec::encode_bool(&mut buf, b),
        InnerValue::Int(i) => sort_codec::encode_i64(&mut buf, i),
        InnerValue::F64(f) => {
            if sort_codec::encode_f64(&mut buf, f).is_err() {
                return Ok(None);
            }
        }
        InnerValue::Str(s) => sort_codec::encode_str(&mut buf, &s),
        InnerValue::Bin(b) => sort_codec::encode_bytes(&mut buf, &b),
        _ => return Ok(None),
    }
    Ok(Some(buf))
}

fn resolve_path(record: &InnerValue, field_path: &[u64]) -> Option<InnerValue> {
    let mut cur = record.clone();
    for &p in field_path {
        let key = shamir_types::core::interner::InternerKey::new(p);
        cur = match cur {
            InnerValue::Map(map) => map.get(&key)?.clone(),
            _ => return None,
        };
    }
    Some(cur)
}

fn decode_record_id_suffix(key_bytes: &[u8]) -> Option<RecordId> {
    if key_bytes.len() < 16 {
        return None;
    }
    let tail = &key_bytes[key_bytes.len() - 16..];
    let mut arr = [0u8; 16];
    arr.copy_from_slice(tail);
    Some(RecordId(arr))
}
