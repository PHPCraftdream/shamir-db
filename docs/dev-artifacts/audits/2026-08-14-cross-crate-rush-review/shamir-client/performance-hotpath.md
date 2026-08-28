# shamir-client -- Performance & O(x->0)

## Summary

The crate is mostly disciplined about its own hot paths (`read_frame_into` buffer reuse in `reader_task`, bounded subscription channels, the O(N)-ack'd `FieldMap::len`), but the single hottest primitive — `Client::roundtrip` — allocates and memcpy's the full serialized request twice per call while leaving its own "T-cl-1 reuse buffer" optimization and the workspace's purpose-built zero-copy `RequestEnvelopeRef` unused. The v2 smart paths (`execute_with_touch` read de-intern / id-keyed encode) add per-row hidden allocations (`ByteBuf` clone + `get_or_create` key String allocs per row×repo, `Bytes::to_vec` per record) where zero-copy variants already exist in-tree. Three unbounded-growth vectors remain: orphaned `pending` entries after caller cancellation, server-controlled key cardinality of the `early_buffer`, and version-ungated ambient interner sync work on every `execute`. Behavioural test coverage of the hot paths is good (demux, timeouts, early-buffer cap, v2 passthrough), but nothing measurable guards the perf claims — the crate has no benches.

## Findings

### 1. `roundtrip` clones the whole serialized request per call and ignores the zero-copy envelope built for exactly this path
- **File:line:** `crates/shamir-client/src/client.rs:975-989` (clone at :982, owning envelope at :986-988)
- **Severity:** high
- **Issue:** The T-cl-1 thread-local `REQ_BUF` serializes `DbRequest` into a reused buffer, then immediately does `buf.clone()` — a fresh heap allocation + full-payload memcpy on **every** request, defeating the stated purpose of the reuse buffer. The clone is then moved into the owning `RequestEnvelope::new`, which additionally allocates a 32-byte `session_id` Vec per request (`session_id.to_vec()` in `shamir-connect/src/common/envelope.rs:43`), and `to_msgpack()` copies the request bytes *again* as the embedded `req` field. Net: 3 allocations + 2 full-payload copies per request in the hottest client path. Meanwhile `RequestEnvelopeRef<'a>` (`shamir-connect/src/common/envelope.rs:92-110`) exists specifically as "zero-copy borrowed envelope for the client encode path … tight client-side request loops where the same `[u8; 32]` session id is sent on every request" — it is benched in `shamir-connect/benches/hot_paths.rs` but never used by this crate.
- **Failure scenario:** Pipelined/multi-MB batches pay 2× row-byte memcpy + 3 allocs per request on the caller's task; throughput-bound clients (napi binding, repl `Pull` loops) eat constant-factor overhead on every op.
- **Suggested fix:** Move the `next_request_id.fetch_add` above the encode; serialize the request into the TLS `REQ_BUF`, then synchronously build `RequestEnvelopeRef { session_id: &self.session_id, request_id: Some(rid), req: &buf }` and call its `to_msgpack()` (single output allocation, zero copy of `req`/`sid`); drop the `REQ_BUF` borrow before the first `.await` (`self.write.lock().await`) exactly as the current comment already promises. Delete the `buf.clone()` and the owning `RequestEnvelope` use.

### 2. v2 read path: per-row `ByteBuf` clone and per-row×repo `get_or_create` key allocations in de-intern
- **File:line:** `crates/shamir-client/src/interner_cache_ops.rs:652-661` (clone at :654), `:728-734` (`get_or_create` per record at :729), `:207` (key construction)
- **Severity:** medium
- **Issue:** `deintern_query_result` clones every `IdBytes` row (`bytes.clone()`) solely so the borrow doesn't cross the de-intern `.await` — a full row-byte memcpy per result row. Then `try_deintern_repos` calls `client.interner_cache().get_or_create(db, repo)` **inside the per-record loop**, and `get_or_create` unconditionally builds `(db.to_string(), repo.to_string())` even on the fast-path hit — 2 String allocations + an `scc` read per row per repo.
- **Failure scenario:** A 10k-row `ResultEncoding::Id` response costs ~10k row-byte clones + ~20k String allocations + 10k scc lookups that resolve to the same `Arc<FieldMap>` every time — all in the read hot path of the flagship v2 flow.
- **Suggested fix:** (a) Resolve `Vec<Arc<FieldMap>>` once per `QueryResult` (before the record loop) and pass the `Arc`s down; (b) take ownership of the row without cloning via `std::mem::replace(record, QueryRecord::IdBytes(ByteBuf::new()))` (cheap placeholder; the response is discarded on any `?` error anyway); (c) optionally add a borrowed-key `lookup(db, repo)` fast path so the common hit doesn't allocate the tuple key.

### 3. `early_buffer` key cardinality is server-controlled and unbounded
- **File:line:** `crates/shamir-client/src/client.rs:260-269`; `crates/shamir-client/src/subscription.rs:30-33`
- **Severity:** medium
- **Issue:** The early buffer is bounded **per sub** (`EARLY_BUFFER_CAP = 256`), but the map key `envelope.sub` comes straight off the wire. Any push frame whose `sub` has no registered handle creates a new entry (`buf.entry(envelope.sub).or_default()`), so a buggy or hostile server can grow the map's key cardinality without bound — up to 256 envelopes × up to `MAX_FRAME_SIZE_DEFAULT` (16 MiB) each, per distinct attacker-chosen sub id.
- **Failure scenario:** A subscription-heavy client connected to a misbehaving/compromised server receives a stream of pushes with random `sub` values; client memory balloons without any consumer-side cap firing (the `tracing::debug!` "early buffer full" line only fires *after* 256 pushes accumulate for the same id).
- **Suggested fix:** Bound the whole structure, not just each entry: e.g. a global `AtomicUsize` early-bufferred-envelope count with a total cap (drop + warn when exceeded), or cap distinct buffered sub ids and evict/deny new keys beyond it. Match the per-key cap comment's own promise that a stalled consumer "can no longer balloon client memory unboundedly".

### 4. Orphaned `pending` entries when the caller's future is cancelled (no drop guard)
- **File:line:** `crates/shamir-client/src/client.rs:993-998` (register before write), `:178-203` (cleanup only on the crate's *own* timeout)
- **Severity:** medium
- **Issue:** `roundtrip` inserts the oneshot sender into `pending` and only removes it on: response arrival, send failure, its own `request_timeout` elapse, or connection close. If the *caller* drops the future mid-await (an outer `tokio::time::timeout`, `select!` racing, task abort), the `(rid, tx)` entry stays in the map. It is reclaimed only if the server eventually answers that rid or the connection dies — otherwise one map entry + one oneshot per cancelled request accumulates for the connection's lifetime.
- **Failure scenario:** A long-lived connection whose users wrap calls in their own timeouts against a server that occasionally drops/never answers a rid grows `pending` monotonically — precisely the "unbounded growth" class the theme targets; the demux drain at close then walks and fails every dead sender.
- **Suggested fix:** Return a small RAII guard from `roundtrip`'s registration site holding `(PendingMap, rid)` whose `Drop` removes the rid (mirroring `await_pending_response`'s timeout-path cleanup), so every exit path — cancellation included — leaves no orphan.

### 5. `std::sync::Mutex` on per-request/per-frame hot paths without the required contention-model justification
- **File:line:** `crates/shamir-client/src/client.rs:161` (`PendingMap`), `:300-304` (per-response lock), `:996-998` (per-request lock), `:242-244` (per-push subscriptions lock), `:260` (early-buffer lock)
- **Severity:** medium
- **Issue:** CLAUDE.md pillar 1 and the "Banned in hot paths" table require lock-free primitives on hot paths, with any exception carrying an *inline comment naming the contention model*. `pending` is a single global mutex locked by every concurrent caller (insert) **and** the reader task (remove per response frame) — a genuine contention point that grows with in-flight concurrency; the existing inline comments (`:301`, `:995`) argue poison-recovery and "no `.await` while held", i.e. reentrancy safety, not a contention model. `scc::HashMap` (pillar 5) is already a dependency of this very crate (`interner_cache.rs`) and is the table's prescribed registry primitive.
- **Failure scenario:** Many concurrent callers on one `Client` serialize every request registration and every response delivery through one mutex — throughput degrades with fan-in, exactly what pillar 1 exists to prevent.
- **Suggested fix:** Migrate `PendingMap` / `SubscriptionMap` / `EarlyBuffer` to `scc::HashMap<_, _, THasher>` (`insert_sync`/`remove_sync` are lock-free and no `.await` is involved), or, if `std::sync::Mutex` is deliberately kept, add the CLAUDE.md-mandated inline contention-model comment per site.

### 6. Ambient interner sync runs on every `execute` regardless of server version or cache state
- **File:line:** `crates/shamir-client/src/client.rs:779-784` (`distinct_repos` walk per execute); `crates/shamir-client/src/interner_cache_ops.rs:357-384` (collect + touch before the `>= 2` check at :389)
- **Severity:** medium
- **Issue:** On **every** `execute` (the `interner_epochs.is_empty()` gate is always true for builder-built batches), the client walks all queries via `distinct_repos` — O(ops) with an unconditional `tr.repo.clone()` per op (`shamir-query-types/src/batch/query_entry.rs:105`) — then calls `get_or_create` per repo (2 more String allocs) and inserts epochs into the request. None of this can matter on a pre-v2 server (`server_query_version() < 2` never id-key encodes/de-interns), yet it is sent anyway. Similarly `execute_with_touch` performs the full field-name collection and may fire real `interner_touch` **roundtrips** before the version check at :389 — wasted wire trips on v1, plus `all_repos`'s `tr.repo.clone()` per op (:370).
- **Failure scenario:** A v1-server workload (or any workload against a repo with a cold/irrelevant cache) pays a hidden per-request O(ops) allocation + walk, plus extra roundtrips in `execute_with_touch`, for machinery whose results are never consumed.
- **Suggested fix:** Gate the epoch advertisement (client.rs:779) and the pre-touch/collection phase (interner_cache_ops.rs:357-384) on `self.server_query_version() >= 2`; short-circuit when the registry holds no map for `db`. Combined with finding 2's hoisted FieldMaps, the per-request ambient cost on warm v2 paths drops to a few atomics.

### 7. `encode_record_idmsgpack` pays an avoidable full-record copy per INSERT (`Bytes::to_vec`)
- **File:line:** `crates/shamir-client/src/interner_cache_ops.rs:611-613`
- **Severity:** low
- **Issue:** `query_value_to_storage_bytes` already returns an owned `Bytes` (zero-copy from the internal `Vec`), but the client calls `.to_vec()` — an allocation + full-record memcpy per record — before wrapping in `ByteBuf`. The sibling `query_value_to_storage_bytes_into` scratch variant exists precisely because this "+1 alloc + memcpy per row" pattern caused a measured regression (see its doc in `shamir-types/src/codecs/interned/messagepack.rs:878-889`).
- **Failure scenario:** Large v2 INSERT batches copy every encoded record one extra time on the write hot path.
- **Suggested fix:** Use `bytes.into_vec()` (zero-copy when the `Bytes` is unique, which it is here) — or reuse one scratch buffer per batch via the `_into` variant and push `ByteBuf::from(scratch-copy)` only where ownership is required.

### 8. `collect_field_names` clones every field-name key of every record before dedup
- **File:line:** `crates/shamir-client/src/interner_cache_ops.rs:468-480` (`k.clone()` per key), `:377-380` (sort/dedup after the fact)
- **Severity:** low
- **Issue:** For each write op, every map key (recursively, including nested maps/lists) is cloned into a `Vec<String>` — including duplicates across records of the same batch — then sorted and deduplicated, and `touch_fields`/`missing_names` re-walks and clones only the genuinely missing ones anyway. The eager full-clone pass is redundant allocation proportional to total record keys per request.
- **Failure scenario:** A 1k-record batch with 20 fields each allocates and discards ~20k Strings per `execute_with_touch` call.
- **Suggested fix:** Collect `&str` references (the batch outlives the touch loop) into a `TFxSet<&str>` per repo, and clone only the deduped unknown set — one allocation per *distinct unknown* field instead of per key occurrence.

### 9. Push frames pay up to two failed envelope parses before `PushEnvelope` decode
- **File:line:** `crates/shamir-client/src/client.rs:121-135` (`decode_frame` tries Response then Error), `:240` (third attempt: PushEnvelope)
- **Severity:** low
- **Issue:** Every non-response/non-error frame (i.e. every push on a subscription-heavy connection) is deserialization-attempted three times; each failed attempt partially decodes and allocates (e.g. the 32-byte `sid` Vec) before erroring. Constant-factor, but it is per-frame work in the reader task's loop.
- **Failure scenario:** High-rate push streams burn 2× wasted parse work per frame; garbage frames pay 3×.
- **Suggested fix:** Peek the first msgpack map key (a few bytes via `rmp` decode primitives, no full deserialization) to route directly to Response / Error / Push, or reorder to try `PushEnvelope` when the frame's first key is `"sub"`-shaped. Cheap discrimination only — the constant factor is not worth complexity beyond a first-key check.

### 10. Perf claims are unmeasured: no benches, no allocation-behaviour tests
- **File:line:** `crates/shamir-client/Cargo.toml` (no `[[bench]]`); `crates/shamir-client/src/tests/` (no allocation/growth assertions)
- **Severity:** nit
- **Issue:** The crate documents deliberate hot-path optimizations ("T-tcp-1" buffer reuse at client.rs:218-223, "T-cl-1" thread-local encode buffer at :970-977, the per-yield mutex-skip guard in cursor_stream.rs:237-244) but has no bench (workspace convention: `bench_scale_tool::Harness`) and no test observing allocation counts or map growth — so regressions like finding 1's clone (which silently nullifies T-cl-1) or findings 3/4's growth vectors are invisible to the suite. Behavioural coverage itself is good: demux ordering/garbage/EOF-drain, timeout cleanup, early-buffer full-drop, v2 passthrough + refresh, ambient delta, cursor close/cancel are all tested.
- **Suggested fix:** Add one `benches/roundtrip.rs` (request encode + demux decode, per CLAUDE.md bench conventions) and a unit test asserting `pending`/early-buffer invariants after cancellation/misbehaving-server scenarios — that alone would have caught findings 1, 3, and 4.
