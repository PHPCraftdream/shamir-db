# shamir-sdk -- Performance & O(x->0)

## Summary

The crate's hot paths are the msgpack (de)serialization around every host-import call and the guest linear-memory lifecycle that goes with them. The dominant theme issue is memory, not CPU: **every** host-import call leaks its outbound msgpack buffer, and the host-returned buffer is never reclaimed either, so a loop-heavy procedure (bulk insert / repeated get) grows guest linear memory O(cumulative traffic) — the exact unbounded growth the O(x→0) pillar forbids. `Table::query` compounds this by offering no bounded alternative (no limit/cursor), materializing a whole result set twice. CPU-level linear scans (`Params::get`, `HttpResponse::from_value`) exist but are small-N; one latent unbounded busy-spin sits in `__rt::block_on`. Tests (`src/tests/`) thoroughly cover wire conformance and validation shape, but nothing exercises the host-import memory lifecycle, and the crate has no benches.

## Findings

### 1. Host-import ABI leaks both directions' buffers on every call — unbounded guest linear-memory growth in loops

- **File:line:** `crates/shamir-sdk/src/host_imports.rs:60-66` (`encode_leak`), leak call sites 82, 112, 124, 142, 155, 174, 200; host-returned buffers read via `from_raw_parts` at 96, 105, 130, 145, 161, 182, 206, 224 and never freed; the one-shot result leak is `__rt::leak_result` (`crates/shamir-sdk/src/__rt.rs:25-30`).
- **Severity:** high
- **Issue:** `encode_leak` `core::mem::forget`s a fresh `Vec<u8>` on every `batch_put` / `global_set` / `call` / `db_get` / `db_insert` / `db_query` / `http_fetch`, and every buffer the host returns via `shamir_alloc` is likewise abandoned after decoding. The inline justification ("the Store is dropped after `shamir_call` returns", lines 55-59) bounds the leak **per invocation**, not per call — within a single invocation growth is O(total bytes transferred), violating pillar 3 (per-op cost must trend toward constant). Guest linear memory is a hard-capped 32-bit space, so exhaustion is reachable with realistic data volumes.
- **Failure scenario:** a `#[procedure]` bulk-load — `for doc in docs { ctx.db().table("users").insert(doc)?; }` with 100k × 1 KiB docs — leaks ~100 MB request-side plus the same again response-side, tripping wasm allocation failure / OOM trap mid-batch. A per-row `batch_put` scratchpad loop in a `#[function]` behaves identically.
- **Suggested fix:** make per-call memory transient: (a) keep one reusable scratch buffer in a `static Cell<Vec<u8>>` (wasm32 guest is single-threaded here), resize-and-overwrite per call, hand the host its ptr/len — bounded at max-seen message size; or (b) export a `shamir_free(ptr, len)` the host calls after its synchronous read; or (c) a bump arena reset between host calls. At minimum, document the per-call leak as an invocation-lifetime budget so authors know loops are the hazard.

### 2. `Table::query` has no limit/pagination — the whole result set is buffered twice and retained

- **File:line:** `crates/shamir-sdk/src/db.rs:98-109`; ABI side `crates/shamir-sdk/src/host_imports.rs:170-184`; advertised pattern in `crates/shamir-sdk/src/prelude.rs:34-37`.
- **Severity:** medium
- **Issue:** The ABI returns one packed `(ptr, len)` blob that the guest decodes into a full `Vec<Value>`, and the SDK exposes no `limit`/`offset`/cursor parameter — the only bound a guest author has is whatever their filter achieves. `query(None)` materializes the entire table twice (host-side contiguous msgpack buffer + guest-side `Vec<Value>` tree), and per finding 1 both copies are also leaked for the invocation's lifetime. Combined cost is ~2× result size, retained.
- **Failure scenario:** the prelude's own example (`let rows = ctx.db().table(params.str("table")?).query(None)?;`) against a million-row table: host builds a multi-hundred-MB blob, guest decodes an equal-size structure, then OOM-traps.
- **Suggested fix:** add `limit`/keyset-cursor parameters to the `db_query` ABI (and a `Table::query_paginated` / iterator-style API), or chunked/streaming returns. If the ABI can't change soon, document the unbounded-buffering contract loudly on `Table::query` and in the prelude example.

### 3. `__rt::block_on` busy-spins forever if a guest future yields `Pending` — unbounded CPU burn

- **File:line:** `crates/shamir-sdk/src/__rt.rs:36-61`; all four macro kinds drive guest futures through it (`crates/shamir-sdk-macros/src/lib.rs:144, 264, 391, 556`).
- **Severity:** medium
- **Issue:** The no-op-waker driver spin-loops on `Pending` (`spin_loop()`, line 57). Since nothing ever wakes the future, any genuinely-async guest code hangs at 100% CPU with zero forward progress until the host's wall-clock kill. The justifying comment ("pure functions ... are `Ready` on the first poll") is stale: `#[procedure]`/`#[function]` with db/http host imports are driven through the same `block_on`, and while the SDK's own imports are synchronous, nothing stops a guest author from `.await`ing an async primitive (`tokio::time::sleep`, channel `recv`) — it compiles and then livelocks. This is the degenerate unbounded-cost case of O(x→0): per-op CPU cost is infinite instead of constant.
- **Failure scenario:** `#[procedure] async fn f(...) { tokio::time::sleep(Duration::from_secs(1)).await; ... }` — pins a core spinning forever per invocation; under concurrency every guest pins a thread and the host's executor starves.
- **Suggested fix:** treat `Pending` as a hard error — trap after the first (or a small N of) polls with "guest future yielded Pending; async host imports are not supported in this SDK slice" — or implement a real waker via a host import. At minimum, delete the stale "pure functions only" premise from the doc.

### 4. `Params::get` linear-scans the parameter map on every typed access; `bytes()` clones the payload

- **File:line:** `crates/shamir-sdk/src/params.rs:26-32` (scan), `params.rs:68-77` (clone in `bytes`).
- **Severity:** low
- **Issue:** Every `params.i64(..)` / `str(..)` / `bytes(..)` is an O(P) `iter().find()` over `Vec<(String, Value)>`; a function reading M params pays O(P×M) per invocation, and each miss additionally allocates an error `String`. `bytes()` also clones the whole `Vec<u8>`/str payload per call. The dependence on `Vec` instead of a map is a documented trade-off (`crates/shamir-sdk/src/value.rs:13-15`, avoiding `indexmap` in the guest binary), and P is a handful in practice — but it is exactly the "repeated lookups / full scans in helpers" pattern pillar 3 names, with no guard or comment acknowledging the accepted cost.
- **Failure scenario:** none at documented sizes; visible only if P or per-invocation accessor counts grow large (e.g. row-mapping functions reading 10+ params per record).
- **Suggested fix:** keep the `Vec` but do one O(P) indexing pass in `decode_params` (sorted key index or tiny Fx-hash map, per pillar 4) so lookups are O(1); or at least add a comment recording the accepted small-N cost. Optionally add a consuming `take_bytes` variant to avoid the clone for `Bin`.

### 5. HTTP path double-copies payloads and triple-scans the response map

- **File:line:** `crates/shamir-sdk/src/http.rs:98-111` (`to_value` clones method, url, headers, and the **entire body**), `http.rs:130-153` (`from_value` does three separate linear `find` passes and clones every header string plus the body), `crates/shamir-sdk/src/context.rs:116-119` (`http_fetch` builds an intermediate `Value::Map` then msgpack-encodes it).
- **Severity:** low
- **Issue:** Each `http_fetch` copies the request body twice (once into the intermediate `Value::Bin`, once into msgpack bytes, which is then leaked per finding 1), and the response map is scanned three times (status / headers / body) with full clones instead of one ownership-moving pass. Per-call overhead is O(body bytes) extra allocations on top of the unavoidable encode.
- **Failure scenario:** a multi-MB POST (file upload) holds 3–4 simultaneous copies of the body in guest linear memory; irrelevant for small JSON calls but measurable for binary payloads.
- **Suggested fix:** make `HttpRequest::to_value` consuming (`into_value(self)`) and have `Ctx::http_fetch` use it — the borrowed path has no other callers; fold `from_value` into a single pass over the map, moving `body`/header strings out instead of cloning.

### 6. `Db::table` allocates a fresh `String` per handle

- **File:line:** `crates/shamir-sdk/src/db.rs:50-54`.
- **Severity:** nit
- **Issue:** `ctx.db().table("users")` clones the table name into a new `String` each call; a row-loop that re-opens the handle per iteration re-allocates it every time (and finding 1 makes that pattern likely, since the handle itself is cheap to recreate).
- **Failure scenario:** none material — a few bytes per iteration.
- **Suggested fix:** none needed; if loops are the common shape, show hoisting `let users = ctx.db().table("users");` outside the loop in the docs (`db.rs:22-38` example already implies it).

