# Storage Backend Abstraction Layer

This crate provides the unified `Store` / `Repo` trait surface that every
ShamirDB table sits on, plus the concrete implementations behind it.

There is **one** on-disk storage engine today — **Fjall** (an LSM-tree,
journal-based, keyspace-oriented KV store) — and three always-compiled,
in-RAM implementations: `InMemoryStore`, `CachedStore` (a full-mirror
read cache + sync/async write wrapper), and `MemBufferStore` (a
`moka`-backed write-back buffer). The `Cached`/`MemBuffer` wrappers are
*decorators*: they stack on top of any other backend (`Cached →
MemBuffer → Fjall` is the default disk stack; see `shamir-engine`'s
`BoxRepoFactory`).

## Architecture

`error`, `types` (the `Store` / `Repo` trait surface), `storage_in_memory`,
`storage_cached`, and `storage_membuffer` are always compiled — they have
no extra dependencies and are required by tests across the workspace. The
single on-disk backend, `storage_fjall`, is gated behind the `fjall` cargo
feature. The default feature set (`all-backends`) pulls it in so today's
consumers see no change; embedded / minimal builds opt out:

```toml
shamir-storage = { version = "0.1", default-features = false, features = ["fjall"] }
```

> **Note on `all-backends`.** This meta-feature historically expanded to a
> list of per-engine toggles (Sled, Redb, Nebari, Persy, Canopy, …). Those
> backends were removed from the tree and are no longer feature-gated
> options; `all-backends` now expands to `["fjall"]` alone. See
> `Cargo.toml`'s `[features]` table for the authoritative list.

```
shamir-storage/src/
├── lib.rs                       # crate root + module wiring  (always compiled)
├── README.md                    # this file
├── types.rs                     # Store / Repo traits, KvOp, RecordKey  (always compiled)
├── error.rs                     # DbError, DbResult                      (always compiled)
├── key_bytes.rs                 # KeyBytes — small-buffer RecordKey repr (always compiled)
├── storage_in_memory.rs         # InMemoryStore / InMemoryRepo            (always compiled)
├── storage_cached.rs            # CachedStore — full-mirror cache wrapper (always compiled)
├── storage_membuffer.rs         # MemBufferStore — moka write-back buffer (always compiled)
├── storage_fjall.rs             # FjallStore / FjallRepo — on-disk LSM    (feature = "fjall")
├── membuffer_clear_race_hook.rs # test-only deterministic seam for MemBufferStore  (cfg(test))
└── tests/                       # per-backend test modules                 (cfg(test))
```

`RecordKey` is `KeyBytes` (see `types.rs:9`), not raw `Bytes` — `KeyBytes`
is the representation-transparent small-buffer key whose serde encoding is
byte-identical to the WAL's `bytes::Bytes` encoding (see
`key_bytes.rs` / the record-key-128 migration plan). Higher layers address
rows by 16-byte `RecordId`s serialized via `RecordId::to_bytes`.

## Core Traits

The full, authoritative definitions live in `types.rs`; the summaries
below mirror it. Methods are marked **(required)** or **(default)** to
match whether the trait declares them as a required method or provides a
default implementation that backends may override.

### `Store` Trait

Asynchronous key-value store over raw bytes (`types.rs:31-304`).

```rust
pub type RecordKey = KeyBytes;

#[async_trait]
pub trait Store: Send + Sync {
    // --- point ops (required) ---
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey>;            // new record, generated key
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool>;    // upsert; true = created
    async fn get(&self, key: RecordKey) -> DbResult<Bytes>;                 // Err(NotFound) on miss
    async fn remove(&self, key: RecordKey) -> DbResult<bool>;               // true = existed

    // --- streaming scans (required) ---
    fn iter_stream(&self, batch_size: usize) -> RecordStream;               // full scan, ascending key order
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream;  // prefix-filtered

    // --- vectored / batched ops (default impls; backends override where it pays) ---
    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>>;
    async fn set_no_flag(&self, key: RecordKey, value: Bytes) -> DbResult<()>;       // skip the "was created" lookup
    async fn remove_no_flag(&self, key: RecordKey) -> DbResult<()>;                 // skip the "existed" lookup
    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>>;
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>>;
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>>;
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()>;                         // atomic mixed-op batch
    fn iter_range_stream(&self, start: Option<Bytes>, end: Option<Bytes>, batch_size: usize) -> RecordStream;
    fn iter_range_stream_reverse(&self, start: Option<Bytes>, end: Option<Bytes>, batch_size: usize) -> RecordStream;

    // --- durability / lifecycle (default no-ops; buffering backends override) ---
    async fn flush(&self) -> DbResult<()>;                                            // fsync buffered writes
    async fn apply_buffer_config(&self, _cfg: &MemBufferConfig) -> DbResult<()>;      // hot-reload MemBuffer sizing
    async fn raw_backend(&self) -> Option<Arc<dyn Store>>;                           // unwrap one wrapper layer
}
```

Notes on the default impls (`types.rs:148-303`):

- **`get_many`** — default loops `get`, mapping `NotFound → None`. Disk
  backends override with a single transactional read to collapse N×
  `spawn_blocking` setups into one (Fjall overrides, `storage_fjall.rs`).
- **`set_no_flag` / `remove_no_flag`** (task #613) — for callers that
  don't need the `bool` flag. Fjall's `set`/`remove` embed a
  `contains_key` lookup *only* to derive that flag (its `insert`/`remove`
  return no prior value), so the no-flag variants genuinely skip the
  extra LSM lookup — a real halving of point-write cost (see
  `storage_fjall.rs`).
- **`transact`** — default applies `KvOp`s sequentially and is **NOT**
  atomic. Backends with a native write-transaction API override: Fjall
  uses `Database::batch()` → an atomic cross-keyspace `OwnedWriteBatch`
  commit (`storage_fjall.rs`). The in-memory backend keeps the default
  sequential semantics.
- **`flush`** — default is a no-op (fine for `InMemoryStore`). Buffering
  backends override: `FjallStore` calls `Database::persist(PersistMode::SyncAll)`;
  `CachedStore`'s `Async` mode drains its pending-write worker.
- **`raw_backend`** — returns `None` on a raw backend, `Some(inner)` on a
  wrapper. `fully_unwrap_store` (`types.rs:373`) walks the chain to the
  first non-wrapper backend.

`KvOp` (`types.rs:20-24`) is the mixed-op enum for `transact`: `Set(key,
value)` or `Remove(key)`.

### `Repo` Trait

Manages multiple named `Store` instances (tables / keyspaces)
(`types.rs:382-423`).

```rust
#[async_trait]
pub trait Repo: Send + Sync {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>>;  // required; creates if absent
    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool>;          // required
    async fn stores_list(&self) -> DbResult<Vec<String>>;                                   // required

    // (default) Copy every k/v from store `from` into store `to`, batched.
    // Used by RENAME TABLE; backends with a native copy/rename may override.
    async fn copy_store(&self, from: &str, to: &str) -> DbResult<()>;
}
```

## Supported Backends

| Backend | Type | Compiled | Role |
|---------|------|----------|------|
| **InMemory** | `scc::TreeIndex` (lock-free sorted B+ tree) | always | Testing, ephemeral / cache-tier deployments. `scan_prefix_stream` does an `O(log N + matches)` range walk. |
| **Cached** | Wrapper (full-mirror cache over any `Store`) | always | Small hot datasets whose working set fits in RAM. Loads every record from `inner` on construction; reads are then pure-memory. Two write modes (`Sync` / `Async`). |
| **MemBuffer** | Wrapper (`moka` W-TinyLFU write-back buffer over any `Store`) | always | Bounded RAM write-back cache with background flush to `inner`. The default wrapper every disk backend is constructed inside (via `shamir-engine`'s `BoxRepoFactory`). |
| **Fjall** | LSM-tree (journal-based, keyspaces) | `feature = "fjall"` (on by default) | The only on-disk backend. Cross-keyspace atomic write batches; prefix/range scans via the LSM index. |

The four `Cargo.toml`/`lib.rs` annotations above are the complete set —
there is no `storage_sled` / `storage_redb` / `storage_nebari` /
`storage_persy` / `storage_canopy` module, file, or cargo feature in this
crate. Earlier revisions of this README listed those engines; they are not
present in the tree and the comparative claims made about them (Sled's
`skip_first` cursor quirk, Redb's `Bound` API, Nebari's `ScanEvaluation`,
Persy's `PersyId` mapping, Canopy's LZ4 compression) described engines that
were never part of this codebase's current shape.

## Usage Example

```rust
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::types::Repo;

// Open repository (Fjall creates the directory + journal on first open)
let repo = FjallRepo::new("./my_db")?;

// Create/open store (a Fjall keyspace)
let store = repo.store_get("users").await?;

// Use store
let key = store.insert(Bytes::from_static(b"Alice")).await?;
let data = store.get(key).await?;

// Stream records in ascending key order, batch_size at a time
let mut stream = store.iter_stream(100);
while let Some(batch) = stream.next().await {
    for (id, value) in batch? {
        println!("{:?}", id);
    }
}

// List / delete stores
let stores = repo.stores_list().await?;
repo.store_delete("users").await?;
```

Application code does not normally construct `FjallRepo` by hand — it goes
through `shamir-engine`'s `BoxRepoFactory`, which composes the wrapper
layers. `BoxRepoFactory::fjall(path)` returns `MemBuffer(Fjall(...))` by
default; `BoxRepoFactory::fjall_raw(path)` returns the unwrapped `Fjall`;
`BoxRepoFactory::cached(inner, mode)` stacks a `CachedStore` on top. See
`shamir-engine::repo::repo_types`.

## Backend-Specific Details

### InMemoryStore (`storage_in_memory.rs`)

- **Backing structure**: `scc::TreeIndex<RecordKey, Bytes>` — a lock-free
  concurrent sorted B+ tree. Replaces an earlier `DashMap` shape whose
  `scan_prefix_stream` was an `O(N)` full-iter + filter.
- **Scans**: `O(log N + matches)` via `TreeIndex::range` from the prefix
  start; `iter_range_stream` likewise seeks to the lower bound. Each batch
  grabs a fresh epoch-buffers `Guard`, collects up to `batch_size`, drops
  the guard before yielding.
- **Concurrency**: reads use epoch-based reclamation (`peek_with`,
  `iter`/`range`); writes take per-node locks scoped to the touched B+
  path.
- **Atomicity**: `transact` inherits the default *sequential* semantics —
  partial state may be observable to concurrent readers. This backend is a
  supported deployment target for embedded / ephemeral / cache-tier use;
  workloads needing cross-op atomicity use Fjall.
- **No durability**: data is lost on restart.

### CachedStore (`storage_cached.rs`)

Full-mirror read cache over any `Store`. On construction it streams every
record from `inner` into an `scc::TreeIndex`; subsequent reads are
pure-memory, falling back to `inner` on a miss (lazy fill).

- **Write modes** (`WriteMode`):
  - `Sync` — write-through: awaits `inner` then updates the cache. Safer;
    use for indexes and other critical data.
  - `Async` — write-behind: updates the cache immediately and enqueues the
    `inner` write onto a single long-lived background worker. Faster, but
    `inner` may lag; pending writes are observable via `pending_writes()`
    and drained by `flush().await`.
- **Ordered background writes** (task #616 pt.2): `Async` mode routes every
  `set`/`remove` through one `mpsc`-drained worker that applies jobs
  strictly FIFO, so two writes to the same key land in submission order
  (independently-spawned tasks gave no ordering guarantee).
- **Incremental scans** (audit finding 1.3): `iter_stream` /
  `scan_prefix_stream` are cursor-resumed (`Bound::Excluded(last_key)`,
  one bounded `range` per batch under a short-lived `Guard`) rather than
  eagerly materializing the whole cache before the first yield.
- **`transact` cache action** (audit finding 1.4): a committed `Set`
  *populates* the cache with the fresh value instead of invalidating, so
  read-after-write hits RAM.

```rust
let cached = CachedStore::new_sync(inner).await?;   // write-through (indexes)
let cached = CachedStore::new_async(inner).await?;  // write-behind (data)

let pending = cached.pending_writes();
cached.flush().await?;                              // drain Async-mode pending writes
```

### MemBufferStore (`storage_membuffer.rs`)

Concurrent **write-back** buffer over any `Store`, with `moka` as the
in-RAM cache (`moka::future::Cache<RecordKey, Slot>`, W-TinyLFU eviction).
This is the wrapper `BoxRepoFactory` installs around every disk backend by
default.

- **Write path**: `insert` / `set` / `remove` return as soon as the
  `moka` cache + a `DashMap`-backed `dirty` set are updated — `inner` is
  not touched yet. A background flusher (spun up in `MemBufferStore::new`)
  drains `dirty` to `inner` in batches on a notify-or-timeout loop; call
  `flush().await` for synchronous durability.
- **Read path**: lock-free `moka` reads (per-thread event buffers, the
  Caffeine pattern). A `dirty_count: AtomicUsize` cardinality mirror gates
  the `dirty` probe so the common case (empty dirty set) skips it
  entirely (task #539 — replaced an earlier boolean sentinel that could
  not be made linearizable).
- **Why `moka`** (see the module doc): the previous `Mutex<LruCache>` +
  `Mutex<HashSet>` serialised every op on one mutex and went flat under
  read scaling. `moka` handles eviction (W-TinyLFU + LRU window), TTL
  (`time_to_live`), and byte capacity (`weigher` + `max_capacity`)
  internally, so this layer's own eviction/TTL/cap loops are unnecessary.
- **No data loss on eviction**: `dirty` holds *values* (not just keys), so
  a `moka` eviction can never silently drop a not-yet-flushed write.
- **Hot-reload** (`apply_config` / `apply_buffer_config`): sizing/TTL
  changes drain `dirty` first, then atomically swap in a fresh `moka`
  cache via `ArcSwap` (DDL-driven, rare).

`MemBufferConfig` (`max_bytes`, `max_entries`, `ttl_ms`,
`flush_interval_ms`, `flush_batch_size`) serializes into the engine's
info-store at the DDL boundary. The flusher errors are surfaced via a
telemetry counter (audit §2.2) rather than swallowed.

### Fjall (`storage_fjall.rs`, `feature = "fjall"`)

The only on-disk backend. Fjall is an LSM-tree store: each `Repo` is a
`fjall::Database` (a directory), each `Store` is a `fjall::Keyspace` (its
own physical LSM-tree). Writes are journaled (append-only `*.jnl` files)
and later compacted into LSM segments.

- **Durability model**: by default a write flushes to OS buffers but **not**
  to disk (matches RocksDB's default). `flush()` overrides the trait
  no-op to call `Database::persist(PersistMode::SyncAll)`, which fsyncs
  the journal. The dedicated write worker (task #536) serializes
  `insert` / `transact` commits onto one OS thread per store — Fjall's
  per-Database journal-writer mutex already serializes writes, so this
  loses no parallelism while removing the per-op `spawn_blocking` hop.
  `set` / `remove` stay on `spawn_blocking` because their embedded
  `contains_key` read benched a net regression when serialized.
- **Atomicity**: `transact` commits via `Database::batch()` — an
  `OwnedWriteBatch` applied atomically across keyspaces (`storage_fjall.rs`).
- **Zero-copy reads**: the `bytes_1` cargo feature (on, see `Cargo.toml`)
  makes Fjall's `Slice` a `bytes::Bytes` under the hood, so `get`/scan
  conversions are refcount moves, not memcpy + alloc (audit finding §1.1).
- **Scans** (`iter_stream`, `scan_prefix_stream`): `O(log N + batch)` via
  `Keyspace::range`, cursor-resumed with `Bound::Excluded(last_key)` each
  batch. `iter_range_stream_reverse` uses `range(...).rev()` (Fjall's range
  iterator is `DoubleEndedIterator`) — a native reverse cursor, no
  in-memory collect.

## Async Streaming Implementation

`iter_stream` / `scan_prefix_stream` yield `Vec<(RecordKey, Bytes)>`
batches in **ascending lexicographic byte order**, each batch resuming
strictly past the previous batch's last key. Fjall's implementation
(`storage_fjall.rs`) is the reference shape:

```rust
fn iter_stream(&self, batch_size: usize) -> RecordStream {
    let keyspace = self.keyspace.clone();
    Box::pin(stream! {
        let mut last_key: Option<Vec<u8>> = None;
        loop {
            let keyspace_clone = keyspace.clone();
            let start_key = last_key.clone();
            let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> =
                task::spawn_blocking(move || {
                    use std::ops::Bound;
                    let lower = match &start_key {
                        Some(c) => Bound::Excluded(c.clone()),  // resume past last key
                        None    => Bound::Unbounded,
                    };
                    let mut items = Vec::with_capacity(256);
                    let mut last_batch_key = None;
                    for guard in keyspace_clone.range((lower, Bound::Unbounded)).take(batch_size) {
                        let (key, val) = guard.into_inner()?;
                        last_batch_key = Some(key.to_vec());
                        items.push((RecordKey::from(Bytes::from(key)), Bytes::from(val)));
                    }
                    Ok((items, last_batch_key))
                }).await.map_err(|e| DbError::Internal(e.to_string()))?;
            let (batch, next_key) = batch?;
            if batch.is_empty() { break; }
            last_key = next_key;
            yield Ok(batch);
        }
    })
}
```

### Key Features
- **PHP-style generators**: clean syntax via `async_stream::stream!`.
- **Memory-efficient**: constant memory per batch regardless of dataset
  size; a consumer that stops early (e.g. an upstream `LIMIT N`) pays only
  for the batches it drained.
- **Blocking work off the runtime**: each batch's `range` walk runs under
  `spawn_blocking`.
- **Lazy**: batches are fetched only when the consumer calls `.next().await`.

### Cursor Management
With only one on-disk backend, the cursor story is uniform:

- **Fjall**: `Keyspace::range((Bound::Excluded(last_key), Unbounded))`
  re-seeks to just past the previous batch's last key and `.take(batch_size)`
  bounds the per-batch cost. `scan_prefix_stream` additionally terminates
  when a key no longer `starts_with(prefix)`.
- **InMemoryStore / CachedStore**: same `Bound::Excluded(last_key)` resume
  pattern, over `scc::TreeIndex::range` under a short-lived epoch `Guard`
  (the `Guard` must not outlive its scope, so the cursor is re-seeked each
  batch rather than held across `.await` points).

(The earlier per-backend cursor table — Sled's `skip_first`, Redb's
`(Bound, Bound)`, Nebari's scan-skip, Persy's collect-then-slice, Canopy's
`range(cursor..)` — described backends that are not in this tree.)

## Testing

Tests run through the workspace's centralised test entry point (see
`CLAUDE.md` → "Centralised test entry point"), **not** raw `cargo test`
(a perimeter guard requires the marker the wrapper sets). To run this
crate's tests:

```bash
./scripts/test.sh -p shamir-storage           # lib tests
./scripts/test.sh -p shamir-storage --full    # lib + integration/e2e
./scripts/test.sh @storage                    # shamir-storage + shamir-wal
```

(Or the `cargo t` / `cargo tl` aliases from `.cargo/config.toml`.)

Per-backend coverage lives in `crates/shamir-storage/src/tests/`:
`storage_in_memory_tests.rs`, `storage_cached_tests.rs`,
`storage_membuffer_tests.rs`, `storage_fjall_tests.rs` (the last gated
behind `feature = "fjall"`), plus `types_tests.rs` and the `key_bytes/`
serde/ord/hash suites. There is one on-disk backend to exercise, so there
is no per-engine `cargo test test_<backend>` list anymore.

## Performance Considerations

### Choosing a Backend

| Use case | Recommendation |
|----------|----------------|
| **Unit tests / ephemeral data** | `InMemoryStore` |
| **Persistence (production)** | Fjall — the only on-disk backend |
| **Small hot dataset, reads must avoid disk latency** | `CachedStore` wrapper over Fjall (or over `MemBuffer(Fjall)`) |
| **Bounded RAM write-back cache with background flush** | `MemBufferStore` wrapper (the default disk stack already is `MemBuffer(Fjall)`) |

The wrapper layers compose: `BoxRepoFactory::cached(fjall(path), mode)`
yields `Cached → Fjall`; the default disk stack is `MemBuffer → Fjall`.

### Memory Usage
- **Streaming**: constant memory per batch regardless of dataset size.
- **CachedStore**: full mirror — memory scales with the *entire* `inner`
  dataset, not just the hot set. Best for small tables.
- **MemBufferStore**: bounded by `max_bytes` / `max_entries` (moka
  W-TinyLFU eviction); `dirty` additionally holds not-yet-flushed values.
- **Batch size**: tune per workload (typical 100–1000).

## Implementation Notes

### Thread Safety
All backends are `Send + Sync`:
- `Arc` for shared ownership; blocking Fjall ops run under `spawn_blocking`.
- `InMemoryStore` / `CachedStore` use `scc::TreeIndex` (lock-free reads via
  epoch-based reclamation).
- `MemBufferStore` uses `moka` (lock-free reads) + `DashMap` for `dirty` +
  `ArcSwap` for the hot-swappable cache instance.

### Error Handling
All operations return `DbResult<T>` over `DbError` (`error.rs`):
- `DbError::Storage` — backend-specific storage error.
- `DbError::NotFound` — key not present (point `get`).
- `DbError::Internal` — runtime / join / channel failures.
- `DbError::KeyExists` — duplicate key on an insert that requires novelty.

### Future Enhancements
Items previously listed here referenced backends and features no longer in
the tree ("Transactions (Persy-only)", "Compression tuning (Canopy)"). The
current state:

- **Transactions** — *already present*. Cross-keyspace atomic writes are
  exposed via `Store::transact` and backed by Fjall's `OwnedWriteBatch`
  at this layer; higher-level serializable/MVCC transaction semantics live
  in `shamir-tx` / the engine's `MvccStore`. There is no Persy-specific
  transaction story to add.
- **Compression tuning** — Fjall ships LZ4 compression by default; tuning
  knobs (journal compression, block size, blob separation) flow through
  `FjallRepo::new` / `Database::builder` rather than a Canopy-style layer.
- **Bulk operations** — `insert_many` / `set_many` / `remove_many` /
  `get_many` are already on the `Store` trait (default impls, overridden by
  Fjall).
