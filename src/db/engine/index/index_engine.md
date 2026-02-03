# Index Engine Architecture

## Overview

Index engine provides fast lookups for table records without full table scans. Indices are maintained asynchronously using a journal-based approach.

## Storage Structure

### Index Data Storage

```
__idx__{table_name} -> index store for a table

Key:   [path: u64, u64, ...][type: u8][hash1: u64][hash2: u64]
Value: Vec<RecordId> (bincode serialized)
```

#### Key Components

**Path component:**
```
"params.length.right" -> [42, 78, 156]

- Each path component is converted to u64 via interner
- Variable length: N components = N * 8 bytes
- Enables prefix-based queries (e.g., "params.*")
```

**Type discriminator:**
```
0x00 - Null
0x01 - Int
0x02 - UInt
0x03 - Float
0x04 - Bool
0x05 - Str
0x06 - Bin
0x07 - Array  (not indexed)
0x08 - Map    (hashed)
0x09 - Set    (not indexed)
0x0A - Decimal
0x0B - BigInt
```

**Double hash:**
```
hash1 = xxhash64(value)
hash2 = fnvhash64(value)

Collision probability: ~2^-128 (negligible)
```

#### Value Structure

```
Vec<RecordId> - all records with this exact (path, type, hash1, hash2)

Example:
  Key: [42, 78, 0x01, 0x3a2f..., 0x7c1e...]
  Value: [id1, id2, id3, ...]  (bincode serialized)
```

### Journal Storage

```
__journal__{table_name} -> operation log for a table

Key:   seq_no (u64, big-endian bytes)
Value: IndexOp (bincode serialized)
```

#### IndexOp Structure

```rust
pub struct IndexOp {
    pub seq_no: u64,              // Sequence number
    pub timestamp: u64,           // Timestamp
    pub op_type: OpType,          // Insert/Update/Delete
    pub record_id: [u8; 16],      // RecordId

    pub changes: Vec<IndexChange>, // All changed index entries
}

pub enum OpType {
    Insert = 0,
    Update = 1,
    Delete = 2,
}

pub struct IndexChange {
    pub path: Vec<u64>,      // Interned path components
    pub value_type: u8,      // Type discriminator
    pub hash1: u64,          // First hash
    pub hash2: u64,          // Second hash
}
```

### Metadata Storage

```
__index_meta__ -> index configuration

RecordId::system("index_config:{table_name}") -> IndexConfig (bincode)
```

```rust
pub struct IndexConfig {
    pub indexes: Vec<IndexDef>,
}

pub struct IndexDef {
    pub path: Vec<u64>,           // Interned path
    pub path_str: String,         // Human-readable path
    pub unique: bool,             // Is unique index?
    pub created_at: u64,          // Creation timestamp
}
```

```
__indexer_pos__ -> indexer positions

RecordId::system("indexer_pos:{table_name}") -> u64 (last processed seq_no)
```

## Index Management

### Creating an Index

```
table.create_index("user.profile.age")?.await;

Steps:
1. Add to IndexConfig (persist)
2. Create index in memory
3. **Build initial index:**
   - Scan all records in table
   - Extract value at "user.profile.age"
   - Compute (type, hash1, hash2)
   - Add to index store
4. Start incremental updates via journal
```

### Dropping an Index

```
table.drop_index("user.profile.age")?.await;

Steps:
1. Remove from IndexConfig
2. Delete all index entries with this path prefix
3. Stop tracking changes
```

### Listing Indexes

```
table.list_indexes()?.await;
-> Vec<IndexDef>
```

## Asynchronous Update Mechanism

### Global Indexer Thread

**Single thread for all tables:**

```
┌─────────────┐     append      ┌──────────────┐
│  Table 1    │ ───────────────> │ Journal 1    │
├─────────────┤                  ├──────────────┤
│  Table 2    │ ───────────────> │ Journal 2    │
├─────────────┤                  ├──────────────┤
│  Table N    │ ───────────────> │ Journal N    │
└─────────────┘                  └──────────────┘
                                          │
                                          ↓
                                  ┌──────────────┐
                                  │ Global       │
                                  │ Indexer      │
                                  │ (single      │
                                  │  thread)     │
                                  └──────────────┘
```

**Processing order:**
```
1. Round-robin through all tables
2. Read next operation from journal
3. Update indexes for that table
4. Save position
5. Move to next table
```

**Guarantees:**
- Sequential processing **per table**
- Interleaved processing **across tables**
- No concurrent updates to same index

### Update Flow

**INSERT operation:**
```
Main thread:
  1. table.insert(value)
  2. Extract indexed paths from value
  3. For each indexed path:
      - Compute (type, hash1, hash2)
      - Add to IndexOp.changes
  4. journal.append(IndexOp)  ← persisted
  5. return to caller  ← non-blocking!

Indexer thread (later):
  1. journal.read(seq_no)
  2. For each change in IndexOp.changes:
      - key = [path][type][hash1][hash2]
      - value = index_store.get(key) or Vec::new()
      - value.push(record_id)
      - index_store.set(key, value)
  3. save_position(seq_no)
```

**UPDATE operation:**
```
Main thread:
  1. table.update(id, new_value)
  2. Extract old indexed paths
  3. Extract new indexed paths
  4. journal.append(IndexOp {
       changes: [
         (old_path, old_hash1, old_hash2),  // remove
         (new_path, new_hash1, new_hash2),  // add
       ]
     })

Indexer thread:
  1. Remove from old hash indexes
  2. Add to new hash indexes
```

**DELETE operation:**
```
Main thread:
  1. table.delete(id)
  2. Extract indexed paths
  3. journal.append(IndexOp)

Indexer thread:
  1. For each change:
      - value = index_store.get(key)
      - value.remove(record_id)
      - if value.is_empty():
          index_store.remove(key)
```

## Index Key Encoding

### Path Encoding

```
"user.profile.age" → [42, 78, 156]

Using interner:
  "user" → 42
  "profile" → 78
  "age" → 156

Key prefix: [42, 78, 156]  (24 bytes)
```

### Value Encoding

**Simple types (Int, Str, etc.):**
```
InnerValue::Int(30)
  → type: 0x01
  → hash1: xxhash64(&30)
  → hash2: fnvhash64(&30)

Full key: [42, 78, 156, 0x01, h1, h2]  (33 bytes)
```

**Complex types (Map, Array):**
```
InnerValue::Map{"theme": "dark"}
  → type: 0x08
  → serialized: bincode(value)
  → hash1: xxhash64(&serialized)
  → hash2: fnvhash64(&serialized)

Full key: [42, 156, 0x08, h1, h2]
```

### Full Key Format

```
┌─────────────────┬──────┬──────────┬──────────┐
│ Path (N * 8B)   │ Type │ Hash1    │ Hash2    │
│ [u64, u64, ...] │ u8   │ u64      │ u64      │
└─────────────────┴──────┴──────────┴──────────┘

Size: (N * 8) + 1 + 8 + 8 bytes
Example (3 components): 33 bytes

Note: Tail size is always 17 bytes (type + hash1 + hash2)
Path length is computed: path_bytes = key.len() - 17
Depth = path_bytes / 8
```

### Encoding/Decoding

**Encoding (write):**
```rust
let mut key = Vec::with_capacity(path.len() * 8 + 17);

// Path components
for &component in path {
    key.extend_from_slice(&component.to_be_bytes());
}

// Fixed tail (always 17 bytes)
key.push(type_disc);
key.extend_from_slice(&hash1.to_be_bytes());
key.extend_from_slice(&hash2.to_be_bytes());

// Total: path.len() * 8 + 17 bytes
```

**Decoding (read):**
```rust
let key = index_key; // Bytes

// Read tail (last 17 bytes)
let tail_offset = key.len() - 17;
let type_disc = key[tail_offset];
let hash1 = u64::from_be_bytes(key[tail_offset + 1..tail_offset + 9].try_into()?);
let hash2 = u64::from_be_bytes(key[tail_offset + 9..tail_offset + 17].try_into()?);

// Read path (everything before tail)
let path_bytes = &key[0..tail_offset];
let path: Vec<u64> = path_bytes
    .chunks(8)
    .map(|chunk| u64::from_be_bytes(chunk.try_into().unwrap()))
    .collect();

// Validation
assert!(path_bytes.len() % 8 == 0, "Invalid path length");
```

## Query Execution

### Exact Match Query

```
table.query().eq("user.age", 30).find()?;

Execution:
1. Resolve path: "user.age" → [42, 78] (via interner)
2. Hash value: 30 → (0x01, h1, h2)
3. Lookup: index_store.get([42, 78, 0x01, h1, h2])
4. Deserialize: Vec<RecordId>
5. Fetch: table.get_batch(record_ids)
6. Return: Vec<(RecordId, UserValue)>
```

### Complex Query

```
table.query()
  .eq("user.age", 30)
  .eq("user.city", "Moscow")
  .find()?;

Execution:
1. index1: age=30 → ids1 = [id1, id2, id3]
2. index2: city="Moscow" → ids2 = [id2, id3, id4]
3. Intersect: ids = ids1 ∩ ids2 = [id2, id3]
4. Fetch: table.get_batch(ids)
5. Return: results
```

### Missing Index Handling

```
Option A: Error
  "Index required for path 'user.age'"

Option B: Fallback to full scan
  "Index not found, scanning table..."

Recommended: Start with A, add B later
```

## Concurrency & Consistency

### Race Condition Prevention

**Single-threaded indexer per index key:**
```
Only one operation processes [path][type][hash1][hash2] at a time
→ No lost updates
→ No concurrent modifications to same Vec<RecordId>
```

### Eventual Consistency

```
Timeline:
  T0: insert() → journal → return
  T1: query() → may not see new record (index not yet updated)
  T2: indexer processes journal
  T3: query() → sees new record

Trade-off:
- Fast inserts (non-blocking)
- Queries may be slightly stale
- Acceptable for most use cases
```

### Crash Recovery

```
Indexer crash:
1. On restart: read __indexer_pos__ for each table
2. Continue from last_position + 1
3. Process pending journal entries

Journal corruption:
1. Detect (bincode deserialize fails)
2. Fallback: rebuild index from scratch (scan all records)
```

## Performance Considerations

### Index Size Estimates

```
Single index entry:
  Key: 33 bytes (3-component path)
  Value: 8 bytes per RecordId

1000 records with age=30:
  Key: 33 bytes
  Value: 8 * 1000 = 8000 bytes
  Total: ~8 KB
```

### Journal Size

```
IndexOp size:
  Header: ~32 bytes (seq_no, timestamp, etc.)
  Per change: ~32 bytes (path, hashes)
  Typical: 3-5 changes per operation

Daily estimate (10k operations):
  10k * 128 bytes = 1.28 MB/day

Cleanup strategy:
  - Delete processed entries after checkpoint
  - Or periodic rotation
```

### Optimization Opportunities

**1. Batch processing:**
```
Process multiple journal entries per table in one batch
→ Better disk I/O utilization
→ Reduced metadata updates
```

**2. Lazy index building:**
```
create_index() starts in background
Query returns "index not ready" until built
→ Non-blocking index creation
```

**3. Selective indexing:**
```
Only index hot paths
  - Filter columns (WHERE)
  - Join keys
  - Frequently queried fields
```

## Limitations & Future Work

### Current Limitations

1. **No range queries** - all values are hashed
2. **Array elements not indexed** - only whole arrays
3. **Single indexer thread** - may become bottleneck
4. **No partial index support** - can't index `WHERE status = 'active'`

### Future Enhancements

1. **Range indexes:**
   ```
   Store value instead of hash for Int/Float
   Enable: WHERE age > 18 AND age < 65
   ```

2. **Multi-threaded indexer:**
   ```
   Partition by table or index
   Concurrent processing with mutex per key
   ```

3. **Full-text search:**
   ```
   Inverted index for string fields
   Tokenization + stemming
   ```

4. **Covering indexes:**
   ```
   Store additional fields in index
   Avoid table lookup for some queries
   ```

## Summary

**Key design decisions:**

1. **Double hash** - virtually eliminates collisions
2. **Async journal-based updates** - non-blocking inserts
3. **Single global indexer** - simple, no concurrent index updates
4. **Path as interned u64 array** - compact, enables prefix queries
5. **Per-table journals** - sequential processing per table
6. **Separate index store** - clean separation from data

**Trade-offs:**
- Fast inserts vs slightly stale queries
- Simple implementation vs advanced features
- Single-threaded indexer vs complexity
