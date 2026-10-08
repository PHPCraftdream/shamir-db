<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Several narrow bounded-work observations are supported. Universal bounded-memory assurances are contradicted by the production response-guard handoff. Dependency source is available, but exact concurrent range-removal complexity remains unproved.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 0 | 0 | 0 | 3 | 1 | 6 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-summary"></a>

### Claim Summary — No hidden hot-path complexity, avoidable allocations or unbounded buffers across all 113 files

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

The claimed blanket assurance has positive counter-evidence: the default finite response budget releases its reservation before queued/writing response bytes disappear, manifests have no size ceiling, and target lookups allocate. Historical exhaustive-review coverage and quantitative performance cannot be reconstructed.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L340); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348); [crates/shamir-server/src/subscriptions/target_match.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/target_match.rs#L44); [crates/shamir-server/src/backup.rs:404](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L404).

<a id="review-subscription-bridge"></a>

### Claim Subscription bridge — Indexed subscription matching and CV-first cache eviction

Status: `unverified`. Current risk: `—`.

Prior-cycle decision: `unverified`.

The index does an owned-key hash lookup followed by up to k target/filter checks, and the caches use leading commit-version keys with range removal. Exact scc 3.8.4 src/tree_index.rs is available and explicitly notes O(N) border-subtree traversal; the full claimed O(evicted + log N) bound under concurrency is not established by the local cache comment.

Evidence: [crates/shamir-server/src/subscriptions/target_match.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/target_match.rs#L44); [crates/shamir-server/src/subscriptions/target_match.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/target_match.rs#L83); [crates/shamir-server/src/subscriptions/decode_cache.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/decode_cache.rs#L132); [crates/shamir-server/src/subscriptions/deliver_cache.rs:101](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/deliver_cache.rs#L101); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-connection-request-loop"></a>

### Claim Connection request loop — Bounded concurrency/backpressure and direct length-prefixed serialization

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Semaphore/channel counts are bounded by max_in_flight and serialization starts after a reserved four-byte prefix. These mechanisms remain real, but count bounds do not establish the promised server-wide byte-budget lifetime or a bounded teardown duration.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L153); [crates/shamir-server/src/connection/request_loop.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L155); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348).

<a id="review-cursor-pagination"></a>

### Claim Cursor pagination — Capped keyset retry and one-time null probe

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Keyset internal limits grow to a ceiling with a StuckAtCeiling exit, and the null probe occurs during eligible cursor creation. Internal reads include a peek row; the cap limits returned rows/retries, not total scan/sort work. IndexSeek creation can deliberately use a full-scan fallback.

Evidence: [crates/shamir-server/src/db_handler/cursor_handlers.rs:909](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L909); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L1007); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1038](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L1038); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1338](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L1338); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1376](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L1376).

<a id="review-registries"></a>

### Claim Registries — Atomic cardinality mirrors and pruning of historical session/IP entries

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The cited paths maintain atomic counters and remove zero-count session/IP entries. This is a narrow implementation observation; subscription teardown and dead supervisor entries prevent a general leak-free conclusion.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L37); [crates/shamir-server/src/conn_limiter.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/conn_limiter.rs#L240); [crates/shamir-server/src/cursor_registry.rs:601](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/cursor_registry.rs#L601); [crates/shamir-server/src/tx_registry.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tx_registry.rs#L250).

<a id="review-byte-budget"></a>

### Claim Byte budget — CAS fast path, contention-only Notify parking and upfront reservation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

CAS-before-registration, enable-before-recheck and Execute upfront acquisition are present. Their local accounting works, but production destroys the task-local guard before taking it for WriterMsg. The narrow fast-path observation is supported; end-to-end response retention is not.

Evidence: [crates/shamir-server/src/byte_budget.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L131); [crates/shamir-server/src/byte_budget.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L155); [crates/shamir-server/src/db_handler/handler.rs:573](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L573); [crates/shamir-server/src/byte_budget.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L339); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348).

<a id="review-user-directory"></a>

### Claim User directory — Cached hot-path ticket invalidation lookup

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Lookup reads the warmed in-memory map and atomic epoch, without a Fjall read/decode. Mutation updates occur after persistence. Expected hash-lookup cost is supported, but scc read_sync is bucket-locked rather than universally lock-free or worst-case O(1).

Evidence: [crates/shamir-server/src/user_directory.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L278); [crates/shamir-server/src/user_directory.rs:432](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L432); [crates/shamir-server/src/user_directory.rs:606](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L606); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-backup-restore"></a>

### Claim Backup/restore — Streaming file hashing; only a small manifest is read whole

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

File-content hashing uses a fixed 1 MiB buffer, but fs::read loads the entire uncapped manifest and verification stores accounting/on-disk path collections. 'Small' and whole-operation fixed-memory claims are unsupported.

Evidence: [crates/shamir-server/src/backup.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L50); [crates/shamir-server/src/backup.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L59); [crates/shamir-server/src/backup.rs:404](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L404); [crates/shamir-server/src/backup.rs:416](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L416); [crates/shamir-server/src/backup.rs:449](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L449).

<a id="review-replication"></a>

### Claim Replication — 1000-event pulls and exponential backoff on transient failures

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

1000 is a requested pull limit, not a validation of every source's returned event count. Hello skips sleeping; successful transport replies reset backoff before structured-error/decode handling, defeating escalating backoff for repeated such failures.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L231); [crates/shamir-server/src/replication/follower_loop.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L265); [crates/shamir-server/src/replication/follower_loop.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L278); [crates/shamir-server/src/replication/follower_loop.rs:287](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L287); [crates/shamir-server/src/replication/follower_loop.rs:316](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L316).

<a id="review-scheduler-observability"></a>

### Claim Scheduler / observability — Periodic GC and metrics work are off the request path

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Maintenance runs in separate spawned tasks. Synchronous tick/collector work can still occupy runtime workers, and degraded-index collection also walks indexes. Separate scheduling does not prove zero interference or the module's quantitative cost claims.

Evidence: [crates/shamir-server/src/scheduler.rs:299](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L299); [crates/shamir-server/src/observability.rs:447](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L447); [crates/shamir-server/src/observability.rs:456](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L456); [crates/shamir-server/src/observability.rs:473](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L473).

## Evidence and recipe corrections

- The response budget is not merely deployment-dependent: its production handoff currently loses the guard even when the default finite cap is enabled.
- The scc archive is available. Its range-removal implementation does not justify treating the local O(evicted + log N) comment as a proven concurrent bound.
- A requested replication event limit is not an enforced bound against every ReplSource response.
- Cursor ceilings permit a peek row and do not cap total underlying scan/sort work.
- Observability's nanosecond rendering and 30–50 microsecond collection claims were not verified; separate Tokio tasks share runtime resources.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- Performance & O(x->0)

## Summary

`shamir-server` is unusually mature on this theme: the subscription fan-out
path (`subscriptions/decode_cache.rs`, `deliver_cache.rs`, `target_match.rs`)
already migrated off `DashMap`/linear-scan to `scc::TreeIndex` with CV-first
keys for O(log N) lookups and O(evicted + log N) eviction, with per-bridge
target indices replacing O(T) scans. The connection hot path
(`request_loop.rs`), cursor pagination (`db_handler/cursor_handlers.rs`,
`cursor_registry.rs`), tx registry, byte-budget accountant, and per-IP/global
connection limiters all use atomics or `scc`/`dashmap` with documented,
re-verified contention arguments and RAII-bounded cleanup. Every read
(manual, full-file, of all 113 `.rs` files) plus an independent second-pass
agent scan of the remaining unreviewed files turned up no new hidden
O(N)/O(N²) hot-path defect, no per-iteration-allocation-that-should-be-hoisted,
and no unbounded in-memory buffer.

## Findings

No findings for this theme.

Every candidate area was checked and found either genuinely bounded or
already documented+justified inline as an accepted, reviewed tradeoff:

- **Subscription bridge (`subscriptions/bridge.rs`, `push.rs`, `reactive.rs`,
  `target_match.rs`, `filter_eval.rs`, `decode_cache.rs`,
  `deliver_cache.rs`)** — per-event work is O(1)-gated via a per-bridge
  `TargetIndex` built once at subscribe time; the global decode/deliver
  caches are `scc::TreeIndex` keyed CV-first specifically so eviction
  (`cache_evict_up_to`/`deliver_cache_evict_up_to`) is a bounded range-remove,
  not a full-map scan (this replaced an earlier `DashMap` version — see the
  "Stage 2 of the hidden-O(N) sweep" doc references in both files).
  `DeliverMode::Batch`/`Call` inherently re-execute a per-subscriber query
  (`reactive.rs`) — unavoidable given bind-variable semantics, not a
  regression.
- **Connection request loop (`connection/request_loop.rs`, `framer.rs`)** —
  back-pressure via `Semaphore` + bounded `mpsc`; `encode_prereserved`/
  `write_frame_prereserved` avoid the extra memcpy a naive length-prefix
  implementation would pay per frame.
- **Cursor pagination (`db_handler/cursor_handlers.rs`, `cursor_registry.rs`)**
  — the keyset/offset/index-seek bookmark machinery is dense but every
  retry loop is capped by `cursor_limits.max_cursor_page_size`
  (`limit_ceiling`), and the one full-table-scan probe
  (`order_by_column_contains_null`) is explicitly documented as a one-time,
  `create_cursor`-time cost, not a per-page cost.
- **Registries (`tx_registry.rs`, `cursor_registry.rs`, `conn_limiter.rs`,
  `subscriptions/registry.rs`)** — every live-count is an `AtomicUsize`/
  `AtomicU32` mirror maintained at each mutation site (never a `.len()` scan),
  matching CLAUDE.md's O(x->0) pillar; map entries are pruned back to zero on
  release (`PerIpLimiter::release`, `CursorRegistry::free_session_slot`), so
  none of these accumulate unboundedly across historical connections/IPs/
  sessions.
- **Byte budget (`byte_budget.rs`)** — lock-free CAS-loop fast path,
  `Notify`-based parking only on contention, upfront-reserve-then-shrink
  avoids a double-acquire on the common path.
- **User directory (`user_directory.rs`)** — hot-path ticket-invalidation
  lookup is an O(1) in-memory cache (`tickets_cache: SccHashMap`) warmed once
  at boot; all `db.persist(PersistMode::SyncAll)` + full-directory-scan
  operations (`invalidate_all_tickets`, boot-time migration) are admin/boot
  operations, not per-request.
- **Backup/restore (`backup.rs`)** — verified (not merely assumed from the
  crate's own doc comment) that file hashing streams through a fixed
  `HASH_STREAM_BUFFER_SIZE` buffer for both backup manifest generation and
  restore verification; the only whole-file read is the small `manifest.json`
  index itself.
- **Replication (`replication/follower_loop.rs`, `supervisor.rs`,
  `in_process.rs`)** — pull loop is bounded by `DEFAULT_PULL_LIMIT = 1000`
  events per iteration with idempotent bookmark advancement and exponential
  backoff on transient failures; the supervisor's catalogue reconciliation
  re-reads admin-managed (small, not request-driven) subscription/profile
  tables on a 10s tick, not per-request.
- **Scheduler / observability (`scheduler.rs`, `observability.rs`)** — all
  periodic-tick GC/metrics work, correctly off the request hot path.

No code changes were made; this is a read-only review.

</details>
