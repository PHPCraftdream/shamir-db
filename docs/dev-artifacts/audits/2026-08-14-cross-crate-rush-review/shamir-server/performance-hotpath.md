<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-server — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

No measured timing conclusions are available. Several allocation and bounded-work observations are supported, but the universal constant-time/no-unbounded-buffer assurances need qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 0 | 0 | 0 | 2 | 2 | 6 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-summary"></a>

### Claim Summary — No hidden hot-path complexity, avoidable allocations or unbounded buffers across all 113 files

Status: `unverified`. Current risk: —.

Historical exhaustive inspection and independent-agent coverage cannot be reconstructed from current source alone. This revalidation inspected assigned mechanisms, not a fresh whole-crate audit. Explicit unbounded-budget configuration is supported, so bounded memory is deployment-dependent.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:451](../../../../../crates/shamir-server/src/server/server_launcher.rs#L451); [crates/shamir-server/src/byte_budget.rs:96](../../../../../crates/shamir-server/src/byte_budget.rs#L96); [crates/shamir-server/src/config.rs:355](../../../../../crates/shamir-server/src/config.rs#L355).

<a id="review-subscription-bridge"></a>

### Claim Subscription bridge — Indexed subscription matching and CV-first cache eviction

Status: `unverified`. Current risk: —.

The index narrows matching to k relevant targets, not total O(1) event work; lookup constructs a temporary table String. CV-first TreeIndex keys and remove_range_sync are present, but exact O(evicted + log N) behavior of pinned scc 3.8.4 was not externally verified.

Evidence: [crates/shamir-server/src/subscriptions/target_match.rs:14](../../../../../crates/shamir-server/src/subscriptions/target_match.rs#L14); [crates/shamir-server/src/subscriptions/target_match.rs:44](../../../../../crates/shamir-server/src/subscriptions/target_match.rs#L44); [crates/shamir-server/src/subscriptions/target_match.rs:83](../../../../../crates/shamir-server/src/subscriptions/target_match.rs#L83); [crates/shamir-server/src/subscriptions/decode_cache.rs:44](../../../../../crates/shamir-server/src/subscriptions/decode_cache.rs#L44); [crates/shamir-server/src/subscriptions/decode_cache.rs:132](../../../../../crates/shamir-server/src/subscriptions/decode_cache.rs#L132); [crates/shamir-server/src/subscriptions/deliver_cache.rs:101](../../../../../crates/shamir-server/src/subscriptions/deliver_cache.rs#L101); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

<a id="review-connection-request-loop"></a>

### Claim Connection request loop — Bounded concurrency/backpressure and direct length-prefixed serialization

Status: `not-applicable`. Current risk: —.

Semaphore and writer-channel capacities derive from max_in_flight, and encode_prereserved serializes after a four-byte prefix directly. This bounds counts and removes one framing copy, not all allocations or server-wide bytes.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:63](../../../../../crates/shamir-server/src/connection/request_loop.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153); [crates/shamir-server/src/connection/request_loop.rs:155](../../../../../crates/shamir-server/src/connection/request_loop.rs#L155); [crates/shamir-server/src/connection/request_loop.rs:278](../../../../../crates/shamir-server/src/connection/request_loop.rs#L278).

<a id="review-cursor-pagination"></a>

### Claim Cursor pagination — Capped keyset retry and one-time null probe

Status: `not-applicable`. Current risk: —.

The inspected keyset fetch grows its row limit up to the configured ceiling and returns StuckAtCeiling when progress is impossible. The null probe occurs during cursor creation. A row-result ceiling is not a bound on rows scanned or universal query cost.

Evidence: [crates/shamir-server/src/db_handler/cursor_handlers.rs:909](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L909); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1007](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L1007); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1029](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L1029); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1038](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L1038); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1338](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L1338).

<a id="review-registries"></a>

### Claim Registries — Atomic cardinality mirrors and pruning of historical session/IP entries

Status: `not-applicable`. Current risk: —.

The named cardinality paths use atomics, and zero-count IP/session entries are removed. This observation does not prove every registry is leak-free; the assigned subscription teardown and supervisor-liveness defects remain.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:37](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L37); [crates/shamir-server/src/subscriptions/registry.rs:156](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L156); [crates/shamir-server/src/conn_limiter.rs:240](../../../../../crates/shamir-server/src/conn_limiter.rs#L240); [crates/shamir-server/src/conn_limiter.rs:249](../../../../../crates/shamir-server/src/conn_limiter.rs#L249); [crates/shamir-server/src/cursor_registry.rs:597](../../../../../crates/shamir-server/src/cursor_registry.rs#L597); [crates/shamir-server/src/tx_registry.rs:250](../../../../../crates/shamir-server/src/tx_registry.rs#L250).

<a id="review-byte-budget"></a>

### Claim Byte budget — CAS fast path, contention-only Notify parking and upfront reservation

Status: `not-applicable`. Current risk: —.

The CAS attempt precedes waiter creation; contended waits enable registration before rechecking. Execute reserves before execution and guards release through Drop. Unbounded mode is an explicit supported exception, and not every response path uses this Execute reservation.

Evidence: [crates/shamir-server/src/byte_budget.rs:131](../../../../../crates/shamir-server/src/byte_budget.rs#L131); [crates/shamir-server/src/byte_budget.rs:149](../../../../../crates/shamir-server/src/byte_budget.rs#L149); [crates/shamir-server/src/byte_budget.rs:155](../../../../../crates/shamir-server/src/byte_budget.rs#L155); [crates/shamir-server/src/byte_budget.rs:234](../../../../../crates/shamir-server/src/byte_budget.rs#L234); [crates/shamir-server/src/db_handler/handler.rs:573](../../../../../crates/shamir-server/src/db_handler/handler.rs#L573); [crates/shamir-server/src/connection/request_loop.rs:97](../../../../../crates/shamir-server/src/connection/request_loop.rs#L97).

<a id="review-user-directory"></a>

### Claim User directory — Cached hot-path ticket invalidation lookup

Status: `not-applicable`. Current risk: —.

The lookup uses an in-memory hash map of atomic invalidation epochs, populated during boot and maintained on user mutations. O(1) is expected hash-map lookup shape, not a proven worst-case or measured latency.

Evidence: [crates/shamir-server/src/user_directory.rs:278](../../../../../crates/shamir-server/src/user_directory.rs#L278); [crates/shamir-server/src/user_directory.rs:328](../../../../../crates/shamir-server/src/user_directory.rs#L328); [crates/shamir-server/src/user_directory.rs:432](../../../../../crates/shamir-server/src/user_directory.rs#L432); [crates/shamir-server/src/user_directory.rs:562](../../../../../crates/shamir-server/src/user_directory.rs#L562).

<a id="review-backup-restore"></a>

### Claim Backup/restore — Streaming file hashing; only a small manifest is read whole

Status: `refuted`. Current risk: —.

File-content hashing does use a fixed 1 MiB buffer. However, the manifest is read and decoded whole with no size ceiling, and verification also materializes file-path/accounting collections. Calling that index necessarily small or the entire operation fixed-memory is unsupported.

Evidence: [crates/shamir-server/src/backup.rs:50](../../../../../crates/shamir-server/src/backup.rs#L50); [crates/shamir-server/src/backup.rs:59](../../../../../crates/shamir-server/src/backup.rs#L59); [crates/shamir-server/src/backup.rs:404](../../../../../crates/shamir-server/src/backup.rs#L404); [crates/shamir-server/src/backup.rs:416](../../../../../crates/shamir-server/src/backup.rs#L416); [crates/shamir-server/src/backup.rs:449](../../../../../crates/shamir-server/src/backup.rs#L449).

<a id="review-replication"></a>

### Claim Replication — 1000-event pulls and exponential backoff on transient failures

Status: `refuted`. Current risk: —.

1000 is the requested pull limit, and catalogue reconciliation runs every ten seconds. The blanket backoff assurance is false for Hello replies; structured-error replies also reset backoff before invoking sleep_backoff, preventing exponential growth for repeated structured errors.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:50](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L50); [crates/shamir-server/src/replication/follower_loop.rs:231](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L231); [crates/shamir-server/src/replication/follower_loop.rs:265](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L265); [crates/shamir-server/src/replication/follower_loop.rs:278](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L278); [crates/shamir-server/src/replication/follower_loop.rs:287](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L287); [crates/shamir-server/src/server/server_launcher.rs:1526](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1526).

<a id="review-scheduler-observability"></a>

### Claim Scheduler / observability — Periodic GC and metrics work are off the request path

Status: `not-applicable`. Current risk: —.

The named maintenance work is scheduled in separate tasks. This establishes scheduling separation, not zero runtime-worker interference or measured cost.

Evidence: [crates/shamir-server/src/scheduler.rs:136](../../../../../crates/shamir-server/src/scheduler.rs#L136); [crates/shamir-server/src/scheduler.rs:273](../../../../../crates/shamir-server/src/scheduler.rs#L273); [crates/shamir-server/src/observability.rs:96](../../../../../crates/shamir-server/src/observability.rs#L96).

## Corrections and qualified non-findings

- Describe matching as expected hash lookup plus O(k) target/filter work, with a temporary owned-key allocation.
- Treat the scc eviction complexity as unverified for the pinned implementation, not established by its local doc comment.
- Distinguish bounded frame/task counts from bounded process memory and bounded query scanning.
- The manifest has no enforced small-size guarantee.
- Backoff is not exponential on every failure shape.

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
