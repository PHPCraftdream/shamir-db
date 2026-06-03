# S.H.A.M.I.R. Database — Crate Audit Report

All findings below are adversarially verified true-positives. Grouped by crate, then severity. Format: `file:line` — title — why it matters — fix.

---

## shamir-types

### High
- **`crates/shamir-types/src/core/interner/interner.rs:224`** — entries_after stalls past a leaked-id gap → silent persistence data loss — a mid-range `None` slot makes the delta-capture `break` permanently, so every interned key above the gap is never persisted and is missing after restart (de-intern returns None → decode errors). — Change the `Some(None) => break` arm to `continue` (matching `entries_in_id_range`), or assign ids only on the Vacant branch so no permanent gaps exist.
- **`crates/shamir-types/src/access.rs:30`** — Principal identity from non-cryptographic fxhash — an attacker who can influence usernames can find a collision and be treated as Owner of another principal's resources, forging the identity layer. — Derive principal ids from BLAKE3/SHA-256 (truncated) or assign monotonic ids from a user catalogue; never hash usernames into identity.
- **`crates/shamir-types/src/types/value.rs:172`** — Attacker-controlled `size_hint` drives unbounded preallocation — a ~5-byte MessagePack array/map header declaring 2^32 elements forces a multi-GB `Vec::with_capacity` and aborts the process (decode-bomb DoS). — Clamp the hint: `with_capacity(size_hint.min(SANE_CAP))` and grow on demand.

### Medium
- **`crates/shamir-types/src/types/value.rs:172`** — Untrusted size_hint pre-allocation (panics scope) — same root cause as above; capacity-overflow panic / alloc abort on hostile input. — Drop or clamp the capacity hint.
- **`crates/shamir-types/src/codecs/interned/messagepack.rs:111`** — Unbounded recursion decoding untrusted MessagePack — `rmpv::read_value` + `rmpv_value_to_inner` recurse per nesting level with no cap; deep input overflows the stack and aborts the process (remote DoS). JSON path is mitigated by serde_json's 128-level guard. — Add a depth cap (e.g. 64/128) on the msgpack parse/conversion.
- **`crates/shamir-types/src/core/interner/interner.rs:119`** — touch_ind clones the whole reverse Vec per first-touch (O(n²) to fill) — bulk interning of fresh field names deep-copies every `UserKey` String on each insert. — Use a chunked/append-only segmented structure or `ArcSwap<Vec<Arc<UserKey>>>` so appends copy only pointers.
- **`crates/shamir-types/src/core/interner/interner.rs:28`** — `parking_lot::Mutex` on write path held across the full reverse-Vec clone — violates the no-parking_lot invariant and serialises all concurrent first-touch writers behind an O(n) copy. — Reserve ids with `AtomicU64::fetch_add`; narrow the critical section to the pointer swap only.
- **`crates/shamir-types/src/codecs/basic/bincode.rs:8`** — Second public `CodecError` collides with canonical `codecs::CodecError` — two same-named, non-interchangeable error types on the same public surface is a `?`-propagation trap for downstream callers. — Rename the bincode-local enum (`BincodeError`) or fold it into the canonical `CodecError`.

### Low
- **`crates/shamir-types/src/types/value.rs:135`** — `visit_u64` truncates u64 > i64::MAX to negative i64 — generic Deserialize silently corrupts large unsigned integers from external input (interned codecs already guard this). — Mirror the codec logic: fall back to `Str(value.to_string())` for `value > i64::MAX`.
- **`crates/shamir-types/src/types/record_id.rs:120`** — Deserialize requires borrowed `&[u8]` — fails on reader/streaming/owned-only deserializers despite bytes being present. — Use a Visitor with `visit_bytes`/`visit_byte_buf` (or `serde_bytes::ByteBuf`).
- **`crates/shamir-types/src/core/interner/interner.rs:64`** — `with_state` sizes reverse Vec by persisted id — a corrupt snapshot id forces an uncatchable OOM abort. — Validate `max_id` against entry count or build the reverse map sparsely.
- **`crates/shamir-types/src/core/interner/interner.rs:24`** — Forward map uses unseeded FxHash on attacker strings — hash-flooding can degrade decode toward O(n) (CPU DoS), bounded by interning dedup. — Use a seeded hasher (`ahash::RandomState`) for this one forward map.
- **`crates/shamir-types/src/core/interner/interned_key.rs:26`** — `id()` re-decodes variable-width bytes on every Hash/Eq/Ord — micro-cost on the per-field record hot path. — Cache the decoded `u64` alongside the bytes.
- **`crates/shamir-types/src/codecs/basic/bincode.rs:24`** — Doc examples reference nonexistent `shamir_db::types::codec` path and lack code fences — misleads anyone copying them. — Rewrite inside a ```` ```rust ```` fence with the real path, or delete.

---

## shamir-storage

### High
- **`crates/shamir-storage/src/storage_fjall.rs:343`** — iter_stream: deleted cursor key terminates the stream early — if the last-yielded key is deleted mid-scan, the equality-skip consumes the whole iterator and the stream ends, silently dropping every record after the cursor (cache rebuild / export loss). — Resume with `Bound::Excluded(cursor)` (as `iter_range_stream_reverse` already does), not exact-match-then-break.
- **`crates/shamir-storage/src/storage_nebari.rs:585`** — iter_stream / scan_prefix_stream: deleted cursor truncates the scan — `skipping` flag never clears, batch comes back empty, stream ends; silent data loss. — Clear `skipping` by ordering (`key <= cursor`) or use `Bound::Excluded(cursor)`.
- **`crates/shamir-storage/src/storage_fjall.rs:381`** — scan_prefix_stream re-scans the entire keyspace per batch (O(N²)) — full `keyspace.iter()` + filter each batch; prefix scan costs O(N·K/batch). — Use `keyspace.range((Included(prefix), Excluded(prefix_end)))` with cursor advance.
- **`crates/shamir-storage/src/storage_fjall.rs:175`** — fjall/persy/nebari/canopy forward `iter_range_stream` falls back to O(N) full scan — all four expose native range APIs and use them for reverse/prefix, but forward range / ORDER BY / MIN scan the whole table. — Add native forward `iter_range_stream` overrides using the existing range-cursor pattern.

### Medium
- **`crates/shamir-storage/src/storage_sled.rs:174`** — iter_stream: inclusive range + unconditional skip_first drops a live record when cursor was deleted — one record lost per affected batch boundary under concurrent deletion. — Use exclusive lower bound `(Bound::Excluded(start), Unbounded)` and drop `skip_first`.
- **`crates/shamir-storage/src/storage_cached.rs:165`** — Async mode set/remove spawn unordered tasks → set/remove reorder on the same key — cache and inner store diverge permanently; survives flush, resurrects on reload. — Serialise per-key async writes through a single ordered drain queue.
- **`crates/shamir-storage/src/storage_membuffer.rs:246`** — Unvalidated `flush_interval_ms = 0` → busy-spin CPU DoS — a malformed/hostile config pins a worker at 100%. — Clamp `flush_interval_ms` to `max(1, value)` (and `flush_batch_size >= 1`); the logging flusher already guards this.
- **`crates/shamir-storage/src/storage_nebari.rs:559`** — nebari iter_stream restarts scan from range start every batch (O(N²)) — skip-flag re-scan instead of a seek cursor. — Pass `Bound::Excluded(last_key)` as the lower bound (scan_prefix_stream is already effectively single-pass).
- **`crates/shamir-storage/src/storage_cached.rs:175`** — Async mode spawns one tokio task per write — no batching; unbounded task fan-out under load, defeating the MemBuffer batching layer. — Coalesce through a bounded channel + single batched drainer (`set_many`/`remove_many`).
- **`crates/shamir-storage/src/storage_cached.rs:131`** — Two divergent `flush` methods (inherent vs trait) — inherent `flush` skips `inner.flush()`; a concrete-typed caller silently gets weaker durability. — Delete the inherent `flush`; keep only the `Store::flush` trait impl.

### Low
- **`crates/shamir-storage/src/storage_sled.rs:114`** — `set` does a redundant `get` before `insert` — extra B-tree traversal per write; `insert` already returns the prior value. — `let prev = tree.insert(...)?; Ok(prev.is_none())`.
- **`crates/shamir-storage/src/storage_nebari.rs:207`** — `set` does redundant `get` before `set` (same in canopy) — two B-tree ops per write. — Use `Tree::replace` (returns prior value) via an owned transaction.
- **`crates/shamir-storage/src/storage_membuffer.rs:367`** — `drain_all` doc references a removed moka eviction listener — misleads readers that evictions trigger inner writes (they do not). — Update comments: dirty set is authoritative, no listener.
- **`crates/shamir-storage/src/storage_fjall.rs:50`** — `store_delete` returns `Ok(true)` for a never-existing store — diverges from all other backends (which return `false`); also creates-then-drops an empty keyspace. — Probe existence first (non-creating lookup) and return `false` when absent.

---

## shamir-query-types

### High
- **`crates/shamir-query-types/src/batch/types.rs:133`** — Unbounded recursion on untrusted nested Filter/FilterValue/Cond → stack-overflow DoS — `BatchOp::deserialize` round-trips through `serde_json::from_value` (no recursion guard) and rmp-serde has none; a few thousand nested `$cond`/`not` levels abort the whole server post-handshake. Planner walks recurse again. — Enforce a max nesting depth during deserialization and make the planner walks iterative.
- **`crates/shamir-query-types/src/auth/types.rs:153`** — Plaintext password / password_hash in plain `String`, leaky `Debug`, no zeroize — `{:?}`/tracing of a BatchOp prints cleartext passwords; the wire doc falsely claims "zeroized after". — Wrap secrets in `Zeroizing<String>`/`Secret` newtype with redacting `Debug`; add the `zeroize` dep.

### Medium
- **`crates/shamir-query-types/src/read/limit.rs:37`** — Unchecked `page * page_size` on untrusted pagination — overflow panics in debug, wraps to a bogus `skip` in release (wrong page). — `page.saturating_sub(1).saturating_mul(page_size)`.

### Low
- **`crates/shamir-query-types/src/read/limit.rs:37`** — (correctness duplicate of above) Unchecked Page multiplication — same overflow, downstream clamps defang it. — Use `saturating_mul`.
- **`crates/shamir-query-types/src/read/limit.rs:91`** — Unchecked `skip + page_size` in `has_next` — overflow → spurious `has_next` (debug panic). — `skip.saturating_add(page_size) < total`.
- **`crates/shamir-query-types/src/batch/planner.rs:408`** — O(n²) order lookup in `sort_by_key` (linear `position` per element) — needless quadratic on per-batch planning (n ≤ 50). — Precompute an `alias → index` map once.
- **`crates/shamir-query-types/src/batch/planner.rs:390`** — `IndexSet::shift_remove` is O(n) per removal — order preservation unneeded since `ready` is re-sorted. — Use `swap_remove`.
- **`crates/shamir-query-types/src/hmac.rs:28`** — Canonical-input doc table omits the three migration ops — doc drift against the wire-stable contract. — Add rows for start/commit/rollback_migration.
- **`crates/shamir-query-types/src/admin/types.rs:269`** — `MigrationStatusOp` doc promises "list all active migrations" but the type can't express it (single required `String`, no list-all branch in handler). — Drop the clause or document a sentinel.

---

## shamir-engine

### High
- **`crates/shamir-engine/src/index/index_manager.rs:972`** — DashMap shard guard held across `.await` in `validate_unique_for_create` — the lazy dashmap iterator holds a shard read-lock across `check_unique_constraint().await` on the per-insert unique hot path; concurrent index DDL on the same shard stalls. — Collect `indexes_unique.iter()` into a `Vec` before the loop (as `SortedIndexManager` already does).
- **`crates/shamir-engine/src/query/batch/executor.rs:134`** — Row-level security filters computed but never enforced — `execute_batch_with_permissions` runs only the coarse gate then calls `execute_batch(... Actor::System)`; `row_filter` has no production caller, so any row_filter grant gets unrestricted access. — AND `row_filter` into each op's filter (validate inserts) and thread the real `Actor`.
- **`crates/shamir-engine/src/index2/vector/hnsw_adapter.rs:193`** — Unbounded vector top-k (`k`) → memory-exhaustion DoS — untrusted `k` near u32::MAX drives `overscan*2+10` and `Vec::with_capacity(k+16)` (~80 GB) on the production-default adapter. — Clamp `k` to `MAX_TOPK` at the lookup boundary; reject `k==0`/`k>cap`.

### Medium
- **`crates/shamir-engine/src/index/index_manager.rs:1010`** — DashMap shard guard across `.await` in `validate_unique_for_update` — writer stall against concurrent DDL on the unique-update hot path. — Collect to `Vec` before awaiting.
- **`crates/shamir-engine/src/index/index_manager.rs:1145`** — Same in `on_record_created_unique` — shard guard across `add_unique_entry().await`. — Collect to `Vec` before awaiting.
- **`crates/shamir-engine/src/index/index_manager.rs:1212`** — Same in `on_record_deleted_unique` — shard guard across `remove_unique_entry().await`. — Collect to `Vec` before awaiting.
- **`crates/shamir-engine/src/function/net_gateway.rs:69`** — SSRF guard checks hostname string only, not resolved IP — a wildcard-allowlisted name resolving to 127.0.0.1/169.254.169.254 passes (DNS-rebinding SSRF); CurlNetGateway has no IP pinning. — Resolve and re-apply `is_private_or_loopback_ip` to every resolved address; pin the connection.
- **`crates/shamir-engine/src/function/builtin.rs:37`** — Argon2id memory/time params unbounded — caller-supplied `memory_kb` near u32::MAX exhausts memory / pins a blocking thread (DoS). — Enforce sane maxima before constructing `Argon2Params`.
- **`crates/shamir-engine/src/index/index_info.rs:141`** — Read-path planning clones every `IndexDefinition` per query — deep-copies all path `Vec<u64>` of all indexes just to match one field, on every WHERE/UPDATE/DELETE. — Add a borrowing iterator / `find_by_field` that compares paths without cloning.
- **`crates/shamir-engine/src/table/write_exec.rs:757`** — `lookup_records_via_index` fetches matched records one-by-one — N storage round-trips where `get_many` does one (read path already uses it). — Collect ids and call `table().get_many(&ids)`; skip `None` (stale) entries.

### Low
- **`crates/shamir-engine/src/function/wasm.rs:908`** — CPU-bound WASM guest occupies a tokio worker (no spawn_blocking/epoch yield) — documented tradeoff, bounded by fuel. — Run under `spawn_blocking` or enable epoch interruption.
- **`crates/shamir-engine/src/index2/fts_ranked_backend.rs:186`** — `plan_update` emits `BumpFtsStats` unconditionally → doc_count drift on empty text — skewed BM25 until reopen; repeated churn can underflow doc_count to u64::MAX. — Guard each bump behind `doc_len > 0` (mirror insert/delete); saturate `on_delete`.
- **`crates/shamir-engine/src/index2/write_ops.rs:113`** — `BumpFtsStats` broadcast corrupts a second FTS index's stats on the same table — op carries no idx_id; B's BM25 absorbs A's doc lengths (restart-bounded). — Thread an `idx_id` and ignore bumps not addressed to the backend.
- **`crates/shamir-engine/src/index2/fts_ranked_backend.rs:160`** — `plan_update` tokenizes the OLD doc twice — `tokenize_set` already calls `tokenize_with_freq`. — Call `tokenize_with_freq(old)` once; derive the set from its keys.
- **`crates/shamir-engine/src/index2/vector/brute_force.rs:245`** — `yield_now()` per write defeats actor coalescing — extra scheduler round-trips / snapshot publishes per bulk op. — Drop the per-write yields; the bounded channel provides backpressure.
- **`crates/shamir-engine/src/index2/vector/brute_force.rs:60`** — `std::sync::Mutex` contradicts index2's stated lock-free invariant — doc/invariant drift (cold shutdown path only). — Use an atomic `Option`/`tokio::sync::Mutex` or relax the module doc.
- **`crates/shamir-engine/src/index2/vector/hnsw_adapter.rs:121`** — Dot-metric distance differs between Hnsw and BruteForce adapters — identical `VectorConfig` yields different f32 scores (sign/offset); latent for anyone reading the score. — Centralise the metric→distance mapping in one shared helper.

---

## shamir-db

### High
- **`crates/shamir-db/src/shamir_db/shamir_db.rs:1095`** — Function-initiated DB access bypasses all per-table access control — `FacadeDbGateway` routes every get/insert/query through `execute()` = `Actor::System`, ignoring the caller's and the function's computed effective actor; per-table ACLs are dead on this path. — Carry the effective `Actor` and call `execute_as(actor, ...)`.
- **`crates/shamir-db/src/shamir_db/execute.rs:79`** — Path traversal in file-backed CreateRepo — unvalidated `db_name`/`create_repo` join into the redb path; `..`/absolute components escape `data_root` to create/overwrite a redb file anywhere writable. — Validate name components (reject `..`, separators, absolute/UNC, NUL; allow `[A-Za-z0-9_-]`), canonicalize, assert within `data_root`.
- **`crates/shamir-db/src/shamir_db/shamir_db.rs:1319`** — `create_group` RMW of `next_group_id` is unsynchronised — concurrent creates allocate the same id, one group silently overwritten. — Serialise id allocation (dedicated mutex or atomic CAS), reserve before write.
- **`crates/shamir-db/src/shamir_db/shamir_db.rs:1327`** — `create_group` persists group before bumping the id counter — a crash in between resurrects the id and overwrites the next group on restart. — Bump the counter durably before writing the group, or make both writes one atomic batch.

### Medium
- **`crates/shamir-db/src/shamir_db/shamir_db.rs:80`** — `ShamirDb::clone()` deep-copies the entire `function_meta` DashMap per request — bare non-Arc field on the `execute_as`/tx/function hot paths; O(catalogue-size) per request, breaking the "cheap Arc clone" invariant. — Wrap as `Arc<DashMap<...>>`.
- **`crates/shamir-db/src/shamir_db/execute.rs:1172`** — Per-op authorization re-resolves shared ancestor metadata for every batch op — O(K·depth) catalogue lookups where the ancestor chain is shared. — Memoize resolved `ResourceMeta` per `ResourcePath` for one `execute_as` call.
- **`crates/shamir-db/src/shamir_db/system_store.rs:549`** — `resource_meta` catalogue lookups are full table scans (system tables have no indexes) — authorization cost scales with the number of dbs/repos/tables/functions on every authenticated request. — Add `.with_indexes()` on system tables or an in-memory meta cache invalidated on DDL/chmod.

### Low
- **`crates/shamir-db/src/shamir_db/execute.rs:80`** — Blocking `std::fs::create_dir_all` in async admin handler — stalls a tokio worker (rare DDL path). — Use `tokio::fs::create_dir_all(...).await`.
- **`crates/shamir-db/src/shamir_db/shamir_db.rs:1325`** — `create_group` default of `1` collides when groups exist but the counter setting is absent — overwrites group 1. — Seed from `max(existing group_ids) + 1`.

---

## shamir-connect

### Medium
- **`crates/shamir-connect/src/server/lockout.rs:87`** — `subnet_of` doesn't canonicalize IPv4-mapped IPv6 → bucket evasion/collision — all `::ffff:` clients collapse into one bucket (cross-client lockout/DoS), and a dual-stack attacker gets two independent brute-force budgets. — Canonicalize via `Ipv6Addr::to_canonical()` before deriving the prefix.

### Low
- **`crates/shamir-connect/src/server/changepw.rs:128`** — Unchecked `now_ns - issued_at_ns` panics on backward clock step — debug panic (per-session DoS); release fails closed. Lone exception to the crate's `saturating_sub` discipline. — Use `now_ns.saturating_sub(issued_at_ns)`.
- **`crates/shamir-connect/src/server/argon2_semaphore.rs:91`** — Lost-wakeup window in `acquire_until` — condvar guards no state coupled to the atomic; a waiter can wait up to ~1s longer than necessary under contention (no hang). — Re-check the atomic predicate under the notify lock, or shrink the poll interval.
- **`crates/shamir-connect/src/server/audit_chain.rs:104`** — `u8` length prefixes for transport/ip_subnet/result silently truncate >255-byte fields — ambiguous canonicalization weakens the tamper-evidence chain (inputs currently short). — Use u16 prefixes (matching `event`) or reject over-long fields.
- **`crates/shamir-connect/src/server/session.rs:317`** — Per-user session cap does a full-store O(N) scan under a global `parking_lot::Mutex` on the auth path — cost grows with total fleet size; auth is rate-limited so contention is low. — Maintain a `by_user` secondary index and drop the global lock.

---

## shamir-server

### High
- **`crates/shamir-server/src/db_handler.rs:754`** — `create_scram_user` stores username un-normalized, diverging from the login lookup key — non-canonical names become unusable accounts; `Admin`/`admin` both insert yet collide at login; no empty/length validation. — Run `NormalizedUsername::from_raw` on the write path (and bootstrap); persist/lookup by `.as_str()`.

### Medium
- **`crates/shamir-server/src/scheduler.rs:216`** — Audit-checkpoint tick runs blocking fsync I/O on a tokio worker — two synchronous fsyncs stall the worker every checkpoint period. — Wrap the checkpoint in `spawn_blocking`/`block_in_place` (the batched flusher already does).
- **`crates/shamir-server/src/bootstrap.rs:133`** — RandomToken bootstrap: superuser committed before the token file is written — a token-write failure permanently locks out the operator (credential lost). — Write the token file first; or roll back the user / log the token on write failure.
- **`crates/shamir-server/src/user_directory.rs:353`** — `update_roles` can change roles without bumping `tickets_invalid_before_ns` (violates §12.6) — on clock skew / equal timestamps, stale sessions keep cached permissions (e.g. revoked admin stays admin). — Force `tickets_invalid_before_ns = max(now, existing+1)` whenever roles change.
- **`crates/shamir-server/src/user_directory.rs:184`** — Per-request validity check does 2 redb opens + full blob copy + full PersistedUser decode to read one u64 — dominant redundant work on every authenticated request. — Add a `user_id → u64` index or cache the value in-memory; bump on writes.

### Low
- **`crates/shamir-server/src/tables_registry.rs:86`** — `iter_entries` mis-parses `(db, repo)` when a name contains a dot — `split_once('.')` splits on the first dot; boot-replay re-attaches the wrong/no table after restart. — Use a separator that can't appear in names, or store db/repo as structured fields.
- **`crates/shamir-server/src/server_meta.rs:66`** — Long-lived server secrets in plain `Vec<u8>`, dropped without zeroization — crypto roots (signing seed, lockout key, ticket key) linger in freed heap. — Wrap in `Zeroizing<Vec<u8>>` / `ZeroizeOnDrop`.
- **`crates/shamir-server/src/bootstrap.rs:142`** — Bootstrap token file written without restrictive perms on Windows — `#[cfg(unix)]`-only chmod; same for the TLS key in tls.rs. — Set a restrictive ACL on Windows or warn loudly.
- **`crates/shamir-server/src/connection.rs:558`** — `run_handshake` calls `lookup_roles` twice per login — two redb read txns + two decodes for the same record. — Read once, clone the `Vec<String>` for the ticket.
- **`crates/shamir-server/src/connection.rs:462`** — Stale comment block contradicts the actual `verify_proof` code path — misleads readers of the auth handshake. — Replace with a one/two-line note on the final design.
- **`crates/shamir-server/src/connection.rs:873`** — Comment describes already-applied struct edits (doc drift). — Delete the "Patch the struct" scaffolding comments.
- **`crates/shamir-server/src/user_directory.rs:162`** — Public API leaks third-party `redb::Error` — inconsistent with sibling stores; couples the signature to the redb version. — Return a crate-local `thiserror` enum.
- **`crates/shamir-server/src/server.rs:184`** — Duplicate step number (5,5,6) in `shutdown()` doc-sequence. — Renumber to 5,6,7.

---

## shamir-tx

### Medium
- **`crates/shamir-tx/src/mvcc_store.rs:66`** — Archive pre-read conflates I/O errors with NotFound, silently skipping history archival — a non-NotFound error skips archival while the write still overwrites main; a live snapshot's `get_at` then sees the wrong value (snapshot-isolation violation). 4 sites. — `match`: `Ok` archive / `Err(NotFound)` skip / `Err(e) => return Err(e)`.
- **`crates/shamir-tx/src/mvcc_store.rs:176`** — `delete_versioned` discards `main.remove()` result — a backend I/O failure is swallowed; caller sees Ok while the row is still live. — Propagate with `?`.
- **`crates/shamir-tx/src/mvcc_store.rs:315`** — `apply_committed_ops` clones the whole `Vec<KvOp>` on the commit hot path — Phase 4 reads only keys; the clone is a needless Vec alloc + refcount bumps under `commit_lock`. — Collect keys first, then move `ops` into `transact`.

### Low
- **`crates/shamir-tx/src/mvcc_store.rs:144`** — `set_versioned_many` Phase 3 clones every value `Bytes` though only keys are re-used — avoidable refcount bumps on the snapshot-active path. — Move values into `main_ops`; keep only keys for Phase 4.
- **`crates/shamir-tx/src/repo_tx_gate.rs:264`** — `prune_commit_log_below` traverses the pruned range twice (count then remove) — redundant pass purely for a return count scc doesn't provide. — Compute `len_before - len_after`, or drop the count.
- **`crates/shamir-tx/src/id_remap.rs:52`** — Public API leaks `rmp_serde::encode::Error` — foreign error type in the signature; can't distinguish decode vs encode. — Return `DbResult<Bytes>`, mapping serde failures to `DbError::Codec`.
- **`crates/shamir-tx/src/tx_context.rs:480`** — Stringly-typed `Result<(), String>` on public commit-path API — opaque, non-matchable, inconsistent with `DbResult`. — Thread `DbResult<()>` through `apply_id_remap`/`rewrite_set_bytes`.
- **`crates/shamir-tx/src/lib.rs:5`** — Crate doc claims it hosts `GcWorker`, which doesn't exist (line 31 correctly lists it as upcoming). — Drop `GcWorker` from the "hosts" sentence.

---

## shamir-wal

### Medium
- **`crates/shamir-wal/src/wal_manager.rs:135`** — V1/V2 dispatch by "WAL2" magic sniff can misclassify a V1 marker, and one bad key aborts the whole recovery scan — a V1 txn_id whose low 32 bits equal the magic is read as V2 and fails decode; the `?` then aborts `list_inflight`, hiding all inflight markers and breaking crash recovery. — Give V1 markers a distinct on-disk tag; never let one undecodable key abort the scan (skip-and-log instead of `?`).

### Low
- **`crates/shamir-wal/src/wal_entry_v2.rs:183`** — `encode` does a redundant full-body copy per transactional WAL write — intermediate `body` Vec + `extend_from_slice` second pass. — `serialize_into(&mut out, self)` after pushing the 5-byte header.
- **`crates/shamir-wal/src/wal_manager.rs:173`** — `info_store_for_test` is `pub` in production builds (not cfg-gated) — hands out full info_store read/write to any downstream crate; semver/encapsulation hazard. — Gate behind `#[cfg(any(test, feature = "test-util"))]`.

---

## shamir-sdk

### Low
- **`crates/shamir-sdk/src/params.rs:68`** — `Params::bytes` always clones the buffer — copies large binary params per accessor call; siblings borrow. — Add a borrowing `bytes_ref(&self) -> Result<&[u8]>`.
- **`crates/shamir-sdk/src/db.rs:51`** — Doc says `Table::get` returns `Ok(None)` but signature is `Option<Value>` — users write `?` against it (won't compile). — Fix doc to "Returns `None`"; optionally return `Result<Option<Value>>`.
- **`crates/shamir-sdk/src/http.rs:80`** — Consuming builder methods (`header`/`method`/`body`) lack `#[must_use]` — discarding the result silently drops the mutation. — Annotate with `#[must_use]`.

---

## shamir-transport-tcp

### Medium
- **`crates/shamir-transport-tcp/src/tls.rs:41`** — Private key PEM material never zeroized on the server config path — raw PKCS8 bytes left in freed heap; `zeroize` is only a dev-dependency. — Hold key buffers in `Zeroizing`; return the generated key as `Zeroizing<String>`; make `zeroize` a real dependency.

### Low
- **`crates/shamir-transport-tcp/src/framing.rs:8`** — Module doc promises legal empty frames, but `len==0` is always `PeerClose` — contract inconsistency; `write_frame(&[])` is indistinguishable from close. — Tighten the doc (zero-length reserved for close) and reject empty payloads in `write_frame`.
- **`crates/shamir-transport-tcp/src/framing.rs:153`** — `write_frame` truncates length via `as u32` with no guard — oversized payload corrupts the frame stream (writer fails open; reader caps). — Validate with `u32::try_from` / against `MAX_FRAME_SIZE` → `FrameError::TooLarge`.
- **`crates/shamir-transport-tcp/src/tls.rs:25`** — Library API leaks `Box<dyn Error>` — inconsistent with the crate's thiserror enums; opaque `"no PKCS8 key in PEM"`. — Introduce a `TlsError` thiserror enum.
- **`crates/shamir-transport-tcp/src/tls.rs:67`** — Doc claims generic over `rustls::ConnectionTrait` but it's the crate-local `ConnectionExporter`. — Fix the doc to reference the local trait.

---

## shamir-transport-ws

### Low
- **`crates/shamir-transport-ws/src/framing.rs:58`** — `ws_send` truncates length via `as u32`, no `MAX_WS_FRAME_SIZE` guard — send/recv asymmetry; recv validates, send doesn't. — Reject `> MAX_WS_FRAME_SIZE` with `WsFrameError::TooLarge` before the cast.

---

## shamir-client

### Low
- **`crates/shamir-client/src/client.rs:140`** — Password plaintext copy not zeroized on handshake error paths — `process_challenge` only zeroizes on success; early errors leave plaintext in freed heap. — Wrap the copy in `Zeroizing::new(...)`.
- **`crates/shamir-client/src/client.rs:64`** — Resumption ticket (bearer credential) stored as plain `Vec<u8>`, no Drop/Zeroize — re-auth secret resident in freed heap. — Store as `Zeroizing<Vec<u8>>` / `ZeroizeOnDrop`.
- **`crates/shamir-client/src/client.rs:307`** — `create_scram_user` takes plaintext password `&str`, never zeroized — fresh credential lands in multiple non-zeroized heap buffers (String + serialized req_bytes). — Accept `Zeroizing<Vec<u8>>`; zeroize the serialized buffer after send.
- **`crates/shamir-client/src/client.rs:40`** — Doc drift: `accept_new_host` is a no-op when `trusted_pin` is `Some` — the doc instructs flipping a field with no effect in the pinned flow. — Reword: "Ignored when `trusted_pin` is `Some`."

---

## Prioritized Top-10 Actions (most impactful first)

1. **Enforce row-level security** (`shamir-engine/.../batch/executor.rs:134`) — AND each permission's `row_filter` into read/update/delete and thread the real `Actor` instead of `Actor::System`; RLS is currently a silent no-op.
2. **Route function-initiated DB access through `execute_as(actor)`** (`shamir-db/shamir_db.rs:1095`) — stop the `FacadeDbGateway` System-authority bypass so per-table ACLs and setuid actors are enforced for WASM functions.
3. **Cap nesting depth on untrusted Filter/Cond deserialization** (`shamir-query-types/batch/types.rs:133`) — prevent a few-KB deeply-nested request from stack-overflowing and aborting the whole server post-handshake; make planner walks iterative.
4. **Clamp untrusted allocation drivers** — `size_hint` in `shamir-types/value.rs:172`, vector top-k in `hnsw_adapter.rs:193`, and Argon2id memory/time in `builtin.rs:37` — three remote OOM/abort DoS vectors; add `.min(SANE_CAP)` / `MAX_TOPK` / param maxima.
5. **Fix the interner `entries_after` gap stall** (`shamir-types/interner.rs:224`) — change `Some(None) => break` to `continue`; otherwise interned keys above a leaked id are never persisted and vanish after restart (silent data loss).
6. **Fix deleted-cursor stream truncation in fjall/nebari/sled** (`storage_fjall.rs:343`, `storage_nebari.rs:585`, `storage_sled.rs:174`) — resume with `Bound::Excluded(cursor)` so a concurrently-deleted cursor key can't silently drop the rest of a scan (cache rebuild / export loss).
7. **Snapshot unique-index defs before awaiting** (`shamir-engine/index_manager.rs:972` + 1010/1145/1212) — collect `indexes_unique.iter()` into a `Vec` to stop holding DashMap shard guards across `.await` on the per-insert hot path.
8. **Replace fxhash-derived principal identity** (`shamir-types/access.rs:30`) — use a collision-resistant hash or catalogue-assigned ids so usernames can't be collided into another principal's Owner rights.
9. **Validate path components in CreateRepo** (`shamir-db/execute.rs:79`) — reject `..`/separators/absolute/UNC and assert the joined path stays within `data_root` to close the arbitrary-file-write traversal.
10. **Harden auth identity & sessions in shamir-server** — normalize usernames on the write path (`db_handler.rs:754`), and force `tickets_invalid_before_ns` forward on any role change (`user_directory.rs:353`) — fixes unusable/colliding accounts and stale-admin sessions after revocation.