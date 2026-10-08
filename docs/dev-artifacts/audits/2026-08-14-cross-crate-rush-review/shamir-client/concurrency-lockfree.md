<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-client — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Two compound lifecycle/order races remain. Global mutex use violates repository policy, but scc does not supply the advertised lock-free replacement. OnceLock is optional rather than a defect fix.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 7 | 0 | 0 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Reader-exit drain races pending registration → permanent hang of an in-flight request

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Initial closed check is separated from registration. After the reader's final drain, a successful write leaves an unanswered sender under default None response timeout.

Evidence: [crates/shamir-client/src/client.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L408); [crates/shamir-client/src/client.rs:1240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1240); [crates/shamir-client/src/client.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1271).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — `std::sync::Mutex` on the per-request hot path (`PendingMap`) without a sanctioned-category justification

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Every caller inserts and reader removes under a global standard mutex. The inline no-await argument does not name the required contention category; throughput degradation is unmeasured.

Evidence: [crates/shamir-client/src/client.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L250); [crates/shamir-client/src/client.rs:1269](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1269).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `std::sync::Mutex` on the push-streaming hot path (`SubscriptionMap`, `EarlyBuffer`)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each push locks the registry, with misses locking the buffer as well. No sanctioned-category argument is supplied. This establishes policy drift, not a measured medium runtime problem.

Evidence: [crates/shamir-client/src/subscription.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L28); [crates/shamir-client/src/subscription.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L33); [crates/shamir-client/src/client.rs:331](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L331).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Non-atomic subscriptions/early-buffer handoff: lost push + out-of-order delivery

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Separate miss/append and insert/flush scopes permit append-after-flush and direct-send-before-FIFO-flush schedules. Independent concurrent maps do not establish a compound atomic transition.

Evidence: [crates/shamir-client/src/client.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L330); [crates/shamir-client/src/client.rs:349](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/client.rs:1000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1000).

Grouping/duplicate: [SUMMARY.md#1.3](SUMMARY.md#review-1-3). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Early buffer cardinality is unbounded for the life of the connection

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Per-sub caps leave distinct unknown IDs unbounded, and reader exit does not clear retained buffers. Faulty/hostile peer assumptions qualify arbitrary-key flooding.

Evidence: [crates/shamir-client/src/client.rs:350](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L350); [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407).

Grouping/duplicate: [SUMMARY.md#4.3](SUMMARY.md#review-4-3). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Concurrency-claim test gaps: stampede guard and drain race never exercised concurrently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Sequential warm dump and pre-registered EOF waiters cannot catch concurrent first initialization or production insert-after-drain.

Evidence: [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194); [crates/shamir-client/src/tests/demux_tests.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L175).

Grouping/duplicate: [SUMMARY.md#2.3](SUMMARY.md#review-2-3). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — `CursorStream::cursor_id` cell could be a `OnceLock` instead of `StdMutex`

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Write-once storage can be expressed differently, but the existing short mutex scope is not shown to break behavior. Correct its lock-before-is_none comment separately.

Evidence: [crates/shamir-client/src/cursor_stream.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L221); [crates/shamir-client/src/cursor_stream.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L250).

Grouping/duplicate: [SUMMARY.md#2.4](SUMMARY.md#review-2-4). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — `next_request_id` wraps silently at `u32::MAX`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

After a full ID cycle an old pending sender may be replaced and late payloads miscorrelated. Replacement resolves the old receiver as ConnectionClosed; it does not permanently hang that receiver.

Evidence: [crates/shamir-client/src/client.rs:1259](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1259); [crates/shamir-client/src/client.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1271); [crates/shamir-client/src/client.rs:290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L290).

Grouping/duplicate: [SUMMARY.md#1.13](SUMMARY.md#review-1-13). This is not an additional independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — `interner_cache.rs` is fully pillar-compliant and lock-free

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Exact scc 3.8.4 entry methods obtain bucket read/write locks. THasher, CAS-max and nonpoisoning do not alter that fact.

Evidence: [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123); [crates/shamir-client/src/interner_cache.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L115).

Grouping/duplicate: [SUMMARY.md#NF.7](SUMMARY.md#review-nf-7). This is not an additional independent defect.

<a id="review-nf-2"></a>

### Claim NF.2 — No lock is held across an await except the sanctioned write-half guard

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Standard-mutex guards in inspected paths end before awaits. The write-half Tokio guard deliberately spans I/O; OnceCell uses semaphore-based asynchronous initialization coordination.

Evidence: [crates/shamir-client/src/client.rs:389](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L389); [crates/shamir-client/src/client.rs:1277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1277); [crates/shamir-client/src/interner_cache.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L175).

Grouping/duplicate: [SUMMARY.md#NF.7](SUMMARY.md#review-nf-7). This is not an additional independent defect.

## Evidence and recipe corrections

- scc 3.8.4 documentation distinguishes lock-free resizing from bucket-locked entry access and warns about synchronous methods in async execution.
- A one-lock handoff must cover routing decisions and FIFO flush together. Moving only the flush or using two independent entry_sync operations leaves a residual race.
- No amount of no-await commentary alone establishes the repository's required contention model.
- A raw test insert after reader completion does not exercise a fixed Client registration seam.
- Cursor OnceLock reads still involve a state check; branch-free performance is not established.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client -- Concurrency & lock-free invariants

## Summary

The crate is split between two concurrency worlds. `interner_cache.rs` is fully pillar-compliant (lock-free `scc::HashMap` + `THasher`, CAS-max `AtomicU64` epoch, a documented sanctioned `tokio::sync::OnceCell` dump guard, and the required `// O(N) ack:` on the one `len()`); no lock anywhere in the crate is held across an `.await`, and the `tokio::sync::Mutex` write-half guard is the sanctioned "guard across `.await`" exception. However, the rid-demux core (`client.rs`, `subscription.rs`) runs the per-request and per-push hot paths on `std::sync::Mutex<TFxMap<...>>` (pending map, subscription registry, early buffer) -- a documented banned hot-path primitive whose inline comments address await-safety/poisoning, never the contention model, and which fits none of the three sanctioned `std::sync::Mutex` categories. On top of the ideology violations there is one real protocol race: the reader task's final `closed`-store + pending-map drain is not ordered against `roundtrip`'s closed-check + pending insert, which can permanently hang a waiter when `request_timeout = None` (the default).

## Findings

### 1. Reader-exit drain races pending registration → permanent hang of an in-flight request
- **File:line:** `crates/shamir-client/src/client.rs:966` (`closed.load` in `roundtrip`), `client.rs:993-998` (register-into-pending), `client.rs:318-327` (reader's `closed.store` + drain)
- **Severity:** high
- **Issue:** `roundtrip` checks `closed` *before* inserting its oneshot sender into `pending`, and the reader task stores `closed = true` *then* drains `pending` as two unordered steps. A caller can pass the `closed` check, have the reader store `closed` and drain (missing the caller's rid), and *then* insert its sender into the map. The reader task has exited; the map stays alive via `Client.pending`; the sender is never dropped and never sent.
- **Failure scenario:** server restarts / connection EOF at the moment the client issues a request, with `ConnectOptions::request_timeout = None` (the documented default, "preserves the prior unbounded-wait behaviour"). The caller's `rx.await` never resolves -- an un-killable task hang of exactly the class CLAUDE.md says must be hunted, never tolerated. Window is narrow but recurring under reconnect storms; `src/tests/demux_tests.rs` only covers EOF-drain with waiters pre-registered, so the race is untested.
- **Suggested fix:** order the two sides against each other: in `reader_task`, acquire the `pending` lock, then `closed.store(true, Release)` and drain while still holding it; in `roundtrip`, after `map.insert(rid, tx)` under the same lock, re-load `closed` (Acquire) and if set, remove the entry and return `ConnectionClosed`. Then either the insert precedes the reader's locked drain (drain catches it) or the re-check sees `closed == true` (caller self-cleans). Add a regression test that interleaves insert-after-drain.

### 2. `std::sync::Mutex` on the per-request hot path (`PendingMap`) without a sanctioned-category justification
- **File:line:** `crates/shamir-client/src/client.rs:161` (`pub(crate) type PendingMap = Arc<StdMutex<TFxMap<u32, PendingSender>>>`), constructed at `client.rs:545` and `648`, locked per request at `client.rs:996` (insert) and per response at `client.rs:302` (remove)
- **Severity:** high (per CLAUDE.md's normative ban; practical impact moderate)
- **Issue:** Pillar 1/5 mandate lock-free `scc::HashMap`/`DashMap` for shared registries, and CLAUDE.md states `std::sync::Mutex` is "banned in hot paths" outside the three sanctioned categories (dead scaffolding / DDL-only guard sets / first-touch-only population). This map is locked twice on *every* request/response round trip -- the hottest path in the SDK -- and does not fit any sanctioned category (not dead, not DDL, and every request touches a *new* rid). The inline comments (`client.rs:301`, `client.rs:995` -- "std::sync::Mutex, no .await while held") address await-safety, not the required contention-model argument.
- **Failure scenario:** N tasks pipelining concurrent `execute` calls (the documented "fully supported" mode) serialize all rid registrations behind one global mutex contending with the single reader task's per-response removals; under CLAUDE.md's standards this is an unjustified hot-path lock, not merely a style nit.
- **Suggested fix:** `scc::HashMap<u32, PendingSender, THasher>` (the crate already depends on `scc` for `InternerCacheRegistry`) or `DashMap::with_hasher(THasher::default())`; finding 1's ordering fix then moves to a post-insert `closed` re-check + `remove_sync`.

### 3. `std::sync::Mutex` on the push-streaming hot path (`SubscriptionMap`, `EarlyBuffer`)
- **File:line:** `crates/shamir-client/src/subscription.rs:28` and `:33` (type aliases), locked per push frame in `client.rs:242` and `client.rs:260` (reader loop), plus `client.rs:725`/`730` (`subscribe_push`) and `subscription.rs:62` (`SubscriptionHandle::drop`)
- **Severity:** medium
- **Issue:** Same banned-primitive class as finding 2, on the subscription push path: the reader task takes the subscriptions lock for *every* incoming push frame, and the early-buffer lock for every frame whose sub is not yet registered. No contention-model comment, no sanctioned category (early-buffer entries are re-locked and re-written, not first-touch-only). The single reader task is the frequent writer, but every `subscribe_push`/handle-drop also contends on the same locks.
- **Failure scenario:** push-heavy subscriptions make the demux loop acquire a global blocking mutex per frame; a `subscribe_push` registration or a `SubscriptionHandle` drop stalls frame routing.
- **Suggested fix:** `scc::HashMap<u64, PushSender, THasher>` for the registry and `scc::HashMap<u64, Vec<PushEnvelope>, THasher>` for the early buffer (`or_default` + push maps naturally onto `entry_sync`). If the single-reader argument is considered load-bearing, it must be written as an inline contention-model comment per CLAUDE.md -- but a lock-free primitive removes the need.

### 4. Non-atomic subscriptions/early-buffer handoff: lost push + out-of-order delivery
- **File:line:** `crates/shamir-client/src/client.rs:241-244` (reader: check registry, then, on miss, buffer at `260-263`), `client.rs:722-749` (`subscribe_push`: insert into registry, then flush buffer)
- **Severity:** low
- **Issue:** The reader's registry check and early-buffer insert are two separately locked steps, as are `subscribe_push`'s insert and flush. Interleaving (a) reader: registry miss → (b) `subscribe_push`: insert + flush (buffer empty) → (c) reader: early-buffer insert, strands that envelope in the buffer while the subscription is live -- it is never delivered and never evicted. A second interleaving flushes *older* buffered pushes after *newer* direct sends to the same channel (ordering violation within one sub).
- **Failure scenario:** a push arriving exactly during `subscribe_push` startup is silently lost or reordered; for change-feed consumers this is a missed event with no error surfaced.
- **Suggested fix:** make the handoff atomic: route under a single lock acquisition (check registry and, on miss, insert into the buffer while holding the same guard -- i.e. merge the two maps into one guarded structure, or with `scc` use `entry_sync` so registration and buffer-drain cannot interleave with the reader's miss path).

### 5. Early buffer cardinality is unbounded for the life of the connection
- **File:line:** `crates/shamir-client/src/client.rs:260-269` (reader insert path), `crates/shamir-client/src/subscription.rs:30-33` (`EARLY_BUFFER_CAP` bounds per-sub Vec only)
- **Severity:** low
- **Issue:** Per-sub buffering is capped at 256 envelopes, but the early-buffer map gains one entry per *distinct unknown `sub_id`* and entries are never evicted unless `subscribe_push` is called for that exact id. A buggy/malicious server (or sub-id confusion after reconnect) balloons client memory within a single session, against the module's own stated goal that "a stalled consumer can no longer balloon client memory unboundedly".
- **Failure scenario:** server pushes frames with garbage/rotated sub ids for a long-lived connection → unbounded `TFxMap` growth, up to 256 envelopes per bogus id.
- **Suggested fix:** a global buffered-envelope budget (e.g. `AtomicUsize` mirror, per pillar 3) with drop-oldest/stop-buffering once exceeded, and clear the buffer when the reader task exits.

### 6. Concurrency-claim test gaps: stampede guard and drain race never exercised concurrently
- **File:line:** `crates/shamir-client/src/tests/interner_cache_tests.rs` (no `tokio::spawn` anywhere in the file; `dump_repo` OnceCell guard tested only sequentially at lines ~194), `crates/shamir-client/src/tests/demux_tests.rs` (EOF-drain tests pre-register all waiters, lines 175-207)
- **Severity:** low
- **Issue:** The module docs and `dump_repo`'s doc claim "concurrent first-callers share one dump roundtrip (stampede guard)", but no test spawns two concurrent `dump_repo` calls against one `FieldMap`, so the `OnceCell` dedup (and the swallow-error path leaving the cell uninitialized) is only verified in sequence. Likewise nothing covers finding 1's insert-after-drain interleaving.
- **Failure scenario:** a future refactor of `ensure_populated`/`reader_task`'s drain can regress silently; the exact hang class the nextest timeouts exist to surface would only appear as an e2e `TIMEOUT`.
- **Suggested fix:** add (a) a multi-task test that fires N concurrent `dump_repo`s and asserts exactly one `interner_dump` roundtrip reaches the server, and (b) a demux test that registers a waiter after the reader has drained and asserts it resolves with `ConnectionClosed` (once finding 1 is fixed).

### 7. `CursorStream::cursor_id` cell could be a `OnceLock` instead of `StdMutex`
- **File:line:** `crates/shamir-client/src/cursor_stream.rs:221`, `:227`, `:250`, `:276`
- **Severity:** nit
- **Issue:** `Arc<StdMutex<Option<CursorId>>>` is written exactly once (guarded by the `is_none()` fast path at `:251`, correctly commented) and read thereafter -- a textbook first-touch-only cell. It qualifies as a setup-only fallback, but `std::sync::OnceLock<CursorId>` expresses the write-once invariant, removes the poison-handling boilerplate, and makes reads branch-free.
- **Suggested fix:** replace with `OnceLock<CursorId>` (keep the existing guard comment; it becomes the `OnceLock` doc).

### 8. `next_request_id` wraps silently at `u32::MAX`
- **File:line:** `crates/shamir-client/src/client.rs:349`, `:566`, `:985` (`AtomicU32::fetch_add(1, Relaxed)`)
- **Severity:** nit
- **Issue:** The lock-free rid allocator relies on rid uniqueness for the demux, but `fetch_add` wraps; after 2^32 requests on one connection a rid can alias an entry still present in `pending`, overwriting a live sender (one caller hangs, another gets a foreign response). Practically unreachable (needs >4·10^9 requests on one TLS session), recorded for completeness.
- **Suggested fix:** at wrap, return a `Protocol` error or force connection close (`closed.store(true)`), turning a silent cross-delivery into an explicit lifecycle event.


</details>
