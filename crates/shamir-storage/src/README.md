# Storage Backend Abstraction Layer

This module provides a unified interface over 7 embedded database engines (plus a cached wrapper), allowing S.H.A.M.I.R. to work with any supported backend seamlessly.

## Architecture

`error`, `types` (Store / Repo trait surface), `storage_in_memory`, and
`storage_cached` are always compiled. Each on-disk backend is gated by its
own cargo feature (default feature `all-backends` turns them all on; embedded
builds can opt out).

```
shamir-storage/src/
├── lib.rs
├── README.md
├── types.rs                  # Store and Repo traits  (always compiled)
├── error.rs                  # DbError, DbResult types (always compiled)
├── storage_in_memory.rs      # In-memory backend       (always compiled)
├── storage_cached.rs         # Cached wrapper          (always compiled)
├── storage_sled.rs           # Sled backend       (feature = "sled")
├── storage_redb.rs           # Redb backend       (feature = "redb")
├── storage_fjall.rs          # Fjall backend      (feature = "fjall")
├── storage_nebari.rs         # Nebari backend     (feature = "nebari")
├── storage_persy.rs          # Persy backend      (feature = "persy")
└── storage_canopy.rs         # Canopy backend     (feature = "canopy")
```

## Core Traits

### `Store` Trait
Low-level key-value store operating on raw bytes. Keys are `RecordKey =
Bytes` (opaque to storage; higher layers use `RecordId` 16-byte
identifiers serialized via `RecordId::to_bytes`).

```rust
pub type RecordKey = Bytes;
type RecordStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

#[async_trait]
pub trait Store: Send + Sync {
    // Basic operations
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey>;
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool>;
    async fn get(&self, key: RecordKey) -> DbResult<Bytes>;
    async fn remove(&self, key: RecordKey) -> DbResult<bool>;

    // Async streaming (PHP-style generators with batching + prefetch)
    fn iter_stream(&self, batch_size: usize) -> RecordStream;

    // Prefix-filtered streaming (used by index scans)
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream;
}
```

### `Repo` Trait
Manages multiple stores (tables):

```rust
#[async_trait]
pub trait Repo: Send + Sync {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>>;
    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool>;
    async fn stores_list(&self) -> DbResult<Vec<String>>;
}
```

## Supported Backends

| Backend | Type | Best For | Status |
|---------|------|----------|--------|
| **InMemory** | DashMap | Testing, caching | ✅ Stable |
| **Cached** | Wrapper | Read-heavy workloads | ✅ Stable |
| **Sled** | B-Tree | General purpose | ✅ Stable |
| **Redb** | MVCC B-Tree | Concurrency | ✅ Stable |
| **Fjall** | LSM-Tree | Write-heavy | ✅ Stable |
| **Nebari** | B-Tree | BlueDB successor | ✅ Stable |
| **Persy** | B-Tree + Index | Transactions | ✅ Stable |
| **Canopy** | B-Tree | Compression | ✅ Stable |

## Usage Example

```rust
use shamir_storage::storage_sled::SledRepo;
use shamir_storage::types::Repo;

// Open repository
let repo = SledRepo::new("./my_db")?;

// Create/open store (table)
let store = repo.store_get("users").await?;

// Use store
let key = store.insert(Bytes::from_static(b"Alice")).await?;
let data = store.get(key).await?;

// Stream records (memory-efficient!)
let mut stream = store.iter_stream(100);
while let Some(batch) = stream.next().await {
    for (id, value) in batch? {
        println!("{:?}", id);
    }
}

// List all stores
let stores = repo.stores_list().await?;
println!("Stores: {:?}", stores);

// Delete store
repo.store_delete("users").await?;
```

## Backend-Specific Details

### InMemoryStore
- **Type**: DashMap-based in-memory storage
- **Pros**: Zero latency, thread-safe, perfect for tests
- **Cons**: Data lost on restart, limited by RAM
- **Use**: Testing, caching layers, temporary data

### CachedStore
- **Type**: Wrapper with caching capabilities
- **Pros**: Two write modes (Sync/Async), full mirror cache
- **Cons**: Higher memory usage, eventual consistency in Async mode
- **Write Modes**:
  - `WriteMode::Sync`: Write-through - waits for disk (safer, for indexes)
  - `WriteMode::Async`: Write-behind - background writes (faster, for data)

```rust
// Create cached store with sync mode (for indexes)
let cached = CachedStore::new_sync(inner).await?;

// Create cached store with async mode (for data)
let cached = CachedStore::new_async(inner).await?;

// Check pending writes in async mode
let pending = cached.pending_writes();
cached.flush().await?; // Wait for all pending writes
```

### Sled
- **Type**: Pure Rust B-tree database
- **Pros**: Battle-tested, stable, reliable
- **Cons**: No built-in compression
- **Cursor Bug**: Fixed in `iter_stream` - needed `skip_first` flag

### Redb
- **Type**: Modern MVCC B-tree
- **Pros**: ACID transactions, concurrent readers
- **Cons**: Newer, less battle-tested
- **Range API**: Supports `(Bound, Bound)` for precise ranges

### Fjall
- **Type**: LSM-tree database
- **Pros**: High write throughput, keyspaces
- **Cons**: Append-only, needs compaction
- **Iter API**: Manual cursor skip required

### Nebari
- **Type**: B-tree (BlueDB successor)
- **Pros**: Reliable, proven architecture
- **Cons**: Complex scan API
- **Scan Evaluation**: Uses `ScanEvaluation` enum for flow control

### Persy
- **Type**: B-tree with indexes
- **Pros**: ACID, indexes, segments
- **Cons**: Complex RecordId → PersyId mapping
- **Indexing**: Maintains internal index for ID mapping

### Canopy
- **Type**: B-tree with LZ4 compression
- **Pros**: Transparent compression
- **Cons**: Iteration returns `Result<Iter>`
- **Range**: Supports range queries

## Async Streaming Implementation

All backends implement `iter_stream()` using async generators:

```rust
fn iter_stream(&self, batch_size: usize)
    -> Pin<Box<dyn Stream<Item = Result<Vec<_>, DbError>> + Send>>
{
    Box::pin(stream! {
        let mut cursor = None;

        loop {
            // Fetch batch from storage (in spawn_blocking)
            let (batch, next_cursor) = self.fetch_batch(cursor, batch_size).await?;

            if batch.is_empty() {
                break; // No more records
            }

            yield Ok(batch);
            cursor = next_cursor;
        }
    })
}
```

### Key Features
- **PHP-style generators**: Clean syntax using `async_stream::stream!`
- **Memory-efficient**: Constant memory usage regardless of dataset size
- **Concurrent**: Uses `spawn_blocking` for CPU-intensive work
- **Lazy**: Only fetches when consumer calls `.next().await`

### Cursor Management
Each backend manages cursors differently:
- **Sled**: `range(cursor..)` includes cursor, need `skip_first`
- **Redb**: `range((Excluded(cursor), Unbounded))`
- **Fjall**: Manual iterator skip
- **Nebari**: Scan with skip flag
- **Persy**: Collect all, then slice (no range support)
- **Canopy**: `range(cursor..)`

## Testing

All backends have comprehensive test suites:

```bash
# Test specific backend
cargo test test_sled
cargo test test_redb
cargo test test_fjall
cargo test test_nebari
cargo test test_persy
cargo test test_canopy

# Test streaming
cargo test iter_stream

# Run all storage tests
cargo test --lib storage
```

## Performance Considerations

### Choosing a Backend

| Use Case | Recommended Backend |
|----------|---------------------|
| **Testing** | InMemory |
| **Read-heavy** | Sled, Redb, Cached (Sync) |
| **Write-heavy** | Fjall, Cached (Async) |
| **Transactions** | Persy |
| **Compression** | Canopy |
| **Maximum concurrency** | Redb (MVCC) |
| **Hot data caching** | CachedStore wrapper |

### Memory Usage

- **Interning**: Reduces string memory by ~70%
- **Streaming**: Constant memory regardless of dataset size
- **Batch Size**: Tune for your workload (100-1000 typical)

## Implementation Notes

### Thread Safety
All backends implement `Send + Sync`:
- Uses `Arc` for shared ownership
- Uses `spawn_blocking` for blocking operations
- Uses `DashMap` for concurrent interning

### Error Handling
All operations return `DbResult<T>`:
- `DbError::Storage` - Backend-specific errors
- `DbError::NotFound` - Key doesn't exist
- `DbError::Codec` - Serialization errors

### Future Enhancements
- [ ] Bulk operations (batch insert/delete)
- [ ] Transactions (Persy-only)
- [ ] Compression tuning (Canopy)
- [ ] Performance benchmarks
