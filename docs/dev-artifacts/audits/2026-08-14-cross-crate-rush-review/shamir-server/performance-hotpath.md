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
