<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-server — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Nine numbered observations remain open, with narrower reachability or impact qualifications; the namespace-prefix defect is refuted by the documented contract. The replica bypass remains conditional on explicitly configuring the handler ReadOnly.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 9 | 0 | 0 | 2 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Interactive-tx path bypasses the read-only-replica gate entirely

Status: `confirmed-open`. Current risk: `high`.

TxExecute reaches tx_execute_as without the Execute-only NodeMode gate, and TxCommit applies the transaction. Existing DB/table ACLs still apply: this is not available to every authenticated principal. The stock launcher never sets ReadOnly; exploitation requires an explicitly ReadOnly handler deployment.

Evidence: [crates/shamir-server/src/db_handler/handler.rs:529](../../../../../crates/shamir-server/src/db_handler/handler.rs#L529); [crates/shamir-server/src/db_handler/handler.rs:379](../../../../../crates/shamir-server/src/db_handler/handler.rs#L379); [crates/shamir-server/src/db_handler/tx_handlers.rs:168](../../../../../crates/shamir-server/src/db_handler/tx_handlers.rs#L168); [crates/shamir-server/src/db_handler/tx_handlers.rs:247](../../../../../crates/shamir-server/src/db_handler/tx_handlers.rs#L247); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:126](../../../../../crates/shamir-db/src/shamir_db/execute/db_tx.rs#L126); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:171](../../../../../crates/shamir-db/src/shamir_db/execute/db_tx.rs#L171); [crates/shamir-server/src/server/server_launcher.rs:461](../../../../../crates/shamir-server/src/server/server_launcher.rs#L461); [crates/shamir-server/src/db_handler/tests/mod.rs:5](../../../../../crates/shamir-server/src/db_handler/tests/mod.rs#L5).

<a id="review-2"></a>

### Claim 2 — Dead follower-loop registry entries block resubscription after a journal gap

Status: `confirmed-open`. Current risk: `high`.

Task exits do not clear registry entries or test join liveness. However, reconcile removes entries for every non-active row, including resync_required; the 10-second production tick therefore normally clears journal-gap entries. Stalling remains possible for terminal errors leaving state active, or resuming before an intervening inactive-state reconciliation. The original permanent-gap scenario is overstated.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:213](../../../../../crates/shamir-server/src/replication/supervisor.rs#L213); [crates/shamir-server/src/replication/supervisor.rs:223](../../../../../crates/shamir-server/src/replication/supervisor.rs#L223); [crates/shamir-server/src/replication/supervisor.rs:231](../../../../../crates/shamir-server/src/replication/supervisor.rs#L231); [crates/shamir-server/src/replication/supervisor.rs:286](../../../../../crates/shamir-server/src/replication/supervisor.rs#L286); [crates/shamir-server/src/replication/tests/supervisor_tests.rs:398](../../../../../crates/shamir-server/src/replication/tests/supervisor_tests.rs#L398); [crates/shamir-server/src/server/server_launcher.rs:962](../../../../../crates/shamir-server/src/server/server_launcher.rs#L962); [crates/shamir-server/src/server/server_launcher.rs:1526](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1526).

<a id="review-3"></a>

### Claim 3 — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown

Status: `confirmed-open`. Current risk: `medium`.

close_all can remove the handle-less placeholder before attach_handle; a missing-key update then drops the still-running JoinHandle without aborting it. Teardown closes subscriptions before aborting/draining dispatch, allowing this interleaving. The registered late-attach test checks cardinality, not cancellation of a live bridge.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:134](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L134); [crates/shamir-server/src/subscriptions/registry.rs:150](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L150); [crates/shamir-server/src/db_handler/subscribe_handler.rs:87](../../../../../crates/shamir-server/src/db_handler/subscribe_handler.rs#L87); [crates/shamir-server/src/db_handler/subscribe_handler.rs:93](../../../../../crates/shamir-server/src/db_handler/subscribe_handler.rs#L93); [crates/shamir-server/src/connection/request_loop.rs:413](../../../../../crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:48](../../../../../crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L48); [docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:38](../../../../../docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md#L38); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

<a id="review-4"></a>

### Claim 4 — Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)

Status: `confirmed-open`. Current risk: `medium`.

A non-regressing Hello reply to Pull logs and immediately continues without sleeping. A repeatedly malformed source can drive rapid retries; actual CPU cost depends on source and transport latency and was not measured.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:281](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L281); [crates/shamir-server/src/replication/follower_loop.rs:287](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L287); [crates/shamir-server/src/replication/follower_loop.rs:278](../../../../../crates/shamir-server/src/replication/follower_loop.rs#L278).

<a id="review-5"></a>

### Claim 5 — `try_join_next()` swallows non-panic `JoinError`s silently

Status: `confirmed-open`. Current risk: `low`.

The drain still logs only panics. This is a latent diagnostics gap, not a demonstrated currently reachable lost-request path: the visible cancellation producer is abort_all after leaving the reader loop.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:240](../../../../../crates/shamir-server/src/connection/request_loop.rs#L240); [crates/shamir-server/src/connection/request_loop.rs:242](../../../../../crates/shamir-server/src/connection/request_loop.rs#L242); [crates/shamir-server/src/connection/request_loop.rs:415](../../../../../crates/shamir-server/src/connection/request_loop.rs#L415).

<a id="review-6"></a>

### Claim 6 — Vacuous dead-code discard of `ConnectError::AuthFailed`

Status: `confirmed-open`. Current risk: `nit`.

The otherwise unused import and discarded enum value remain. This has no runtime effect; the alleged unmet historical error-handling obligation is unsupported.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:42](../../../../../crates/shamir-server/src/connection/request_loop.rs#L42); [crates/shamir-server/src/connection/request_loop.rs:431](../../../../../crates/shamir-server/src/connection/request_loop.rs#L431).

<a id="review-7"></a>

### Claim 7 — Log-mask override matching is a raw substring/prefix test, not a namespace-segment match

Status: `refuted`. Current risk: —.

Raw longest-prefix matching is already the documented contract, and registered tests deliberately require matches across underscore boundaries. Requiring :: would change that contract. A walnut example could clarify behavior but does not demonstrate a defect.

Evidence: [crates/shamir-server/src/logging.rs:107](../../../../../crates/shamir-server/src/logging.rs#L107); [crates/shamir-server/src/logging.rs:135](../../../../../crates/shamir-server/src/logging.rs#L135); [crates/shamir-server/src/logging.rs:142](../../../../../crates/shamir-server/src/logging.rs#L142); [crates/shamir-server/src/logging/tests/log_mask_tests.rs:32](../../../../../crates/shamir-server/src/logging/tests/log_mask_tests.rs#L32); [crates/shamir-server/src/logging/tests/log_mask_tests.rs:64](../../../../../crates/shamir-server/src/logging/tests/log_mask_tests.rs#L64); [docs/guide-docs/guide/07-operations.md:321](../../../../../docs/guide-docs/guide/07-operations.md#L321).

<a id="review-8"></a>

### Claim 8 — `Scheduler::shutdown()` regression test has no timeout bound on the documented race it guards against

Status: `confirmed-open`. Current risk: `low`.

The auto-discovered integration test still awaits shutdown without a local timeout. The implementation synchronously subscribes receivers before spawning, closing the documented race; this finding concerns failure localization, not a current shutdown bug. Nextest default timeout is 180 seconds, while its CI profile permits 600 seconds.

Evidence: [crates/shamir-server/tests/scheduler.rs:153](../../../../../crates/shamir-server/tests/scheduler.rs#L153); [crates/shamir-server/tests/scheduler.rs:156](../../../../../crates/shamir-server/tests/scheduler.rs#L156); [crates/shamir-server/src/scheduler.rs:126](../../../../../crates/shamir-server/src/scheduler.rs#L126); [.config/nextest.toml:49](../../../../../.config/nextest.toml#L49); [.config/nextest.toml:63](../../../../../.config/nextest.toml#L63).

<a id="review-9"></a>

### Claim 9 — `safe_run`'s panic-survival contract is never actually exercised by a panicking tick

Status: `confirmed-open`. Current risk: `low`.

safe_run still catches unwinding panics, but the registered integration mocks only increment counters. None forces a tick panic and verifies a subsequent invocation.

Evidence: [crates/shamir-server/src/scheduler.rs:140](../../../../../crates/shamir-server/src/scheduler.rs#L140); [crates/shamir-server/src/scheduler.rs:314](../../../../../crates/shamir-server/src/scheduler.rs#L314); [crates/shamir-server/tests/scheduler.rs:44](../../../../../crates/shamir-server/tests/scheduler.rs#L44); [crates/shamir-server/tests/scheduler.rs:174](../../../../../crates/shamir-server/tests/scheduler.rs#L174); [crates/shamir-server/tests/scheduler.rs:218](../../../../../crates/shamir-server/tests/scheduler.rs#L218).

<a id="review-10"></a>

### Claim 10 — `/readyz` is never observed returning "not ready" through the real boot path

Status: `confirmed-open`. Current risk: `low`.

The HTTP integration test observes only post-launch 200. False state and the 503 handler exist, but no HTTP assertion exercises them. Observability is spawned after data listeners bind, so the report's proposed pre-listener window does not exist in the stock launch order.

Evidence: [crates/shamir-server/tests/observability_http.rs:133](../../../../../crates/shamir-server/tests/observability_http.rs#L133); [crates/shamir-server/tests/observability_http.rs:149](../../../../../crates/shamir-server/tests/observability_http.rs#L149); [crates/shamir-server/src/observability.rs:75](../../../../../crates/shamir-server/src/observability.rs#L75); [crates/shamir-server/src/observability.rs:557](../../../../../crates/shamir-server/src/observability.rs#L557); [crates/shamir-server/src/server/server_launcher.rs:878](../../../../../crates/shamir-server/src/server/server_launcher.rs#L878); [crates/shamir-server/src/server/server_launcher.rs:936](../../../../../crates/shamir-server/src/server/server_launcher.rs#L936).

<a id="review-no-findings-for-other-reviewed-areas"></a>

### Claim No findings for other reviewed areas — Other areas are free of logic bugs and vacuous tests

Status: `refuted`. Current risk: —.

The blanket guarantee is too strong: registry_insert_and_remove never inserts or removes, and the late-attach test checks counts without checking live-task cancellation. The assigned teardown defect also contradicts a clean lifecycle guarantee. Remaining unrelated modules were not freshly audited.

Evidence: [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:4](../../../../../crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L4); [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:48](../../../../../crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L48); [crates/shamir-server/src/connection/request_loop.rs:413](../../../../../crates/shamir-server/src/connection/request_loop.rs#L413).

## Corrections and qualified non-findings

- Replace the unconditional critical/any-authenticated-client framing with an ACL-authorized, explicitly ReadOnly-handler threat model.
- NodeMode documentation is in db_handler/config.rs, not the top-level deployment Config; the stock launcher does not configure it.
- A reconciliation observing resync_required, paused or deletion removes the stale supervisor entry; restart is not the only recovery.
- Subscription teardown must also prevent dispatch from creating subscriptions after close_all; merely fixing missing-key attachment is not a complete lifetime barrier.
- Remove the assertion that a discarded ConnectError proves an unmet historical obligation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- Correctness & TDD-coverage

## Summary

Most of `shamir-server` is unusually well-hardened: dense doc comments cite specific prior
incidents, and the corresponding tests force real failure conditions rather than asserting
tautologies. The one critical defect found is a genuine security-relevant gap — the
interactive-transaction path (`TxBegin`/`TxExecute`/`TxCommit`) never checks `NodeMode`,
so a client can write through a node configured as a read-only replica by using the
transactional API instead of `Execute`, and the existing `node_mode_tests.rs` suite is
explicitly scoped (by its own doc comment) to `execute()` only. Beyond that, the
remaining findings are smaller races/gaps in replication supervision and connection
teardown, plus a handful of self-admitted or newly-identified TDD gaps.

## Findings

### 1. Interactive-tx path bypasses the read-only-replica gate entirely
- **File:** `crates/shamir-server/src/db_handler/tx_handlers.rs:73-205` (compare to the gate at `crates/shamir-server/src/db_handler/handler.rs:523-541`)
- **Severity:** critical
- **Issue:** `ShamirDbHandler::execute()` rejects any batch containing a write op when `self.node_mode == NodeMode::ReadOnly` (`handler.rs:529-541`). `tx_execute()` copies every other per-batch gate from `execute()` verbatim — the version check, the query-limits clamp (`tx_handlers.rs:88-100`), the admin/superuser gate (`tx_handlers.rs:102-116`), and the destructive-HMAC gate (`tx_handlers.rs:118-124`) — but has no equivalent check against `node_mode`/`is_write()`. `tx_begin()` and `tx_commit()` likewise never reference `node_mode` or `NodeMode` (confirmed: no match for `node_mode|NodeMode|ReadOnly` anywhere in `tx_handlers.rs`). The top-level dispatcher (`handler.rs:369-393`) routes `TxBegin`/`TxExecute`/`TxCommit` straight to these methods with no outer gate either.
- **Failure scenario:** A node is started with `NodeMode::ReadOnly` (a replica follower per `config.rs`'s documented invariant "a replica follower runs ReadOnly and rejects client writes"). A client that would be rejected via `Execute` with `code: "read_only_replica"` instead opens `TxBegin`, stages an `insert`/`upsert`/`delete` via `TxExecute`, and calls `TxCommit` — the write goes through the engine and is applied locally on the replica, silently diverging replica state from the leader and violating the single-writer invariant the whole read-only-replica feature exists to enforce.
- **Suggested fix:** Add the same `if self.node_mode == NodeMode::ReadOnly { ... }` write-rejection block to `tx_execute()` (mirroring `handler.rs:529-541`), and add a `node_mode_tests.rs` case that opens a `TxBegin`/`TxExecute` against a `ReadOnly` handler and asserts rejection.

### 2. Dead follower-loop registry entries block resubscription after a journal gap
- **File:** `crates/shamir-server/src/replication/supervisor.rs:286-337` (spawn block never clears the registry on task exit) interacting with `reconcile()` at `supervisor.rs:229-237` and `is_running`/`contains_sync` at `supervisor.rs:182-184, 231`
- **Severity:** high
- **Issue:** When a spawned follower-loop task terminates (`JournalGap` or any other `ReplError`), the `tokio::spawn` closure only logs a warning (`supervisor.rs:294-320`) — it never calls `self.registry.remove_sync(&sub_name)`. The `SubHandle` (holding now-dead `JoinHandle`s and a `CancellationToken` nobody will ever cancel) stays in `registry` indefinitely. `reconcile()`'s start-step skips any subscription that `self.registry.contains_sync(&sub.name)` (`supervisor.rs:231`), so once a loop dies this way, no subsequent `reconcile()`/`notify_changed()` call can ever restart it for that subscription name, even after the underlying gap condition is repaired and the row is flipped back to `active`.
- **Failure scenario:** A follower hits a journal gap, the loop exits and marks the subscription `resync_required`; an operator repairs the gap (e.g. re-seeds via snapshot) and flips the row back to `active` — `reconcile()` runs but does nothing, because the stale registry entry for that name still satisfies `contains_sync`. The subscription is permanently stuck until the process restarts or the subscription is deleted and recreated under a different name/profile (the only two paths that clear the entry: `stop_all` on full shutdown, or a rebind-to-different-profile detected by `reconcile`'s stop-step).
- **This gap is self-admitted in the test suite**, not just inferred: `crates/shamir-server/src/replication/tests/supervisor_tests.rs:398-401` reads verbatim: *"The dead loop task leaves a stale registry entry (pre-existing loop-liveness gap, out of scope for this task) — but a fresh `reconcile()` must NOT start a new loop..."* — i.e. the test explicitly works around the bug rather than covering the resume-after-repair path, which is the operationally relevant scenario this whole mechanism exists for.
- **Suggested fix:** Have the spawned closure call `self.registry.remove_sync(&sub_name)` (or a shared "mark dead" helper) on every exit path so `reconcile()` can restart a repaired subscription; add a regression test that flips the row from `resync_required` back to `active` and asserts a fresh loop is spawned.

### 3. `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown
- **File:** `crates/shamir-server/src/subscriptions/registry.rs:109-138` (race between `reserve_pending` → spawn → `attach_handle`, and a concurrent `close_all` at line 150-153) and its caller `crates/shamir-server/src/db_handler/subscribe_handler.rs:86-106`
- **Severity:** medium (narrow window, but the failure mode is a silent resource/task leak past the exact point the calling code assumes is a hard barrier)
- **Issue:** `activate_subscriptions` calls `registry.reserve_pending(sub_id)`, then `tokio::spawn(bridge::bridge_task(...))`, then `registry.attach_handle(sub_id, handle)` — no `.await` between these three steps, but they run as a distinct task from connection teardown's `registry.close_all()` (`connection/request_loop.rs:413`), so on a multi-thread runtime the two can genuinely race across OS threads. If `close_all()`'s `retain_sync(|_, _| false)` removes+drops the `reserve_pending`'d placeholder (which has `bridge_handle: None`) in the window before `attach_handle` runs, `ActiveSubscription::Drop` (`registry.rs:20-25`) has nothing to abort. `attach_handle`'s `update_sync` on the now-missing key is documented as a no-op (`registry.rs:129-133`) "the task has already finished, so `handle` is simply dropped" — but in this race the bridge task has *not* finished; the freshly-spawned `JoinHandle` is just dropped directly (not wrapped in an `ActiveSubscription`), and a bare `JoinHandle::drop` on a running task **detaches rather than aborts** it per tokio's documented semantics.
- **Failure scenario:** A client disconnects at the exact moment a `Subscribe` batch entry is being activated; the bridge task keeps running detached, holding `Arc<dyn PushSink>`/`Arc<ShamirDb>` clones, defeating the very close_all-before-drop ordering that `request_loop.rs:410-413`'s comment says exists specifically to let `conn`/`tx` be dropped safely afterward.
- No test in `crates/shamir-server/src/subscriptions/tests/` or `crates/shamir-server/src/db_handler/tests/subscribe_handler_tests.rs` calls `close_all()` at all (confirmed via search) — the race is entirely uncovered.
- **Suggested fix:** Either hold a short lock/generation-token across `reserve_pending`→`attach_handle` so `close_all` can't observe the half-attached state, or have `attach_handle`'s no-op path re-check-and-abort by having the caller retain the handle and abort it itself when `attach_handle` reports "slot gone."

### 4. Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)
- **File:** `crates/shamir-server/src/replication/follower_loop.rs:278-285`
- **Severity:** medium
- **Issue:** Every other degenerate-reply branch in the pull-response dispatch calls `sleep_backoff(&cancel, &mut backoff_ms).await` before retrying — e.g. the `ReplResponse::Error` branch at lines 267-277. The `ReplResponse::Hello` branch (an unexpected reply shape to a `pull` request) instead does a bare `continue` at line 284 with a comment "skip this iteration" but no backoff call.
- **Failure scenario:** A misbehaving or regressed `ReplSource` that keeps replying `Hello` to `pull` requests causes this branch to spin as fast as the transport allows, with no rate limiting — a busy loop consuming CPU and hammering the source instead of degrading gracefully like every sibling error branch.
- **Suggested fix:** Add the same `sleep_backoff(&cancel, &mut backoff_ms).await` call before `continue` in the `Hello` arm.

### 5. `try_join_next()` swallows non-panic `JoinError`s silently
- **File:** `crates/shamir-server/src/connection/request_loop.rs:240-249`
- **Severity:** low
- **Issue:** The drain loop only branches on `e.is_panic()` (line 242); any other `JoinError` (e.g. a `Cancelled` variant, or any future `JoinError` cause) falls through with no log line, no metric, and no client-visible signal — the dispatch task's outcome (and the request it was handling) simply vanishes.
- **Failure scenario:** If a future change introduces a code path that aborts an individual dispatch task (rather than the whole connection), the client that issued that request gets neither a reply nor an error — it just times out with no server-side trace of why.
- **Suggested fix:** Log the non-panic `JoinError` case too (even at `debug!`/`warn!`), rather than silently discarding it.

### 6. Vacuous dead-code discard of `ConnectError::AuthFailed`
- **File:** `crates/shamir-server/src/connection/request_loop.rs:431`
- **Severity:** nit
- **Issue:** `let _ = ConnectError::AuthFailed;` constructs a value purely to discard it; `ConnectError` is otherwise unreferenced in this file. This looks like leftover scaffolding from an incomplete refactor (an error path that was meant to construct/propagate this variant but doesn't). No test would catch its removal, and it does not affect behavior — but it signals a call site whose original logic obligation is now silently unmet.
- **Suggested fix:** Either wire this into the actual error path it was meant to represent, or remove the line and the now-unnecessary import.

### 7. Log-mask override matching is a raw substring/prefix test, not a namespace-segment match
- **File:** `crates/shamir-server/src/logging.rs:137-146`
- **Severity:** medium
- **Issue:** `LogMask::allows` picks the override via `target.starts_with(prefix.as_str())` with no check for a `::` (or other) boundary after the matched prefix. An override registered for a short namespace constant (e.g. `ns::TX = "tx"`) will also match any future target that merely starts with the same characters (e.g. a hypothetical `"tx_replication"` or `"txn_metrics"` target), silently inheriting that override's verbosity even though it is a semantically unrelated module.
- **Failure scenario:** An operator raises verbosity for namespace `"wal"` intending to cover only WAL-internal targets; a target literally named `"walnut"` (or any future crate/module whose name happens to start with the same substring) unintentionally inherits the same level, because `"walnut".starts_with("wal")` is `true` with no separator check.
- **TDD gap:** `crates/shamir-server/src/logging/tests/log_mask_tests.rs`'s prefix tests (`wal`/`wal_sync`/`wal_compact`) only ever exercise cases where the substring collision is the *intended* behavior (an underscore-joined sub-namespace) — none constructs a genuinely-unrelated-but-textually-colliding target to prove/disprove the boundary semantics.
- **Suggested fix:** Either document `allows()` as an intentional raw-prefix match (not segment-aware) so future `ns::` constants are chosen to avoid collisions, or require the byte immediately after the matched prefix to be `:` (or the prefix to be the full target) before accepting the match; add a test for the non-`::`-delimited collision case either way.

### 8. `Scheduler::shutdown()` regression test has no timeout bound on the documented race it guards against
- **File:** `crates/shamir-server/src/scheduler.rs:98-105` (doc rationale for choosing `broadcast` over `Notify`) vs. `crates/shamir-server/tests/scheduler.rs:145-157`
- **Severity:** low (TDD-coverage gap on a previously-real hazard class, not a live bug)
- **Issue:** The scheduler's own doc comment explains in detail why `broadcast` was chosen over `tokio::sync::Notify` specifically to close the "shutdown fires before the spawned task reaches its `select!`" race — the same bug class CLAUDE.md calls out project-wide (`CancellationToken` vs. lossy `Notify::notify_waiters`). The only test that shuts down immediately after spawn (`spawn_creates_tasks_then_shutdown_joins_them`) asserts only that `shutdown().await` eventually returns, with no `tokio::time::timeout` bound — if this race were ever reintroduced (e.g. a future task type using `Notify` instead of the shared `broadcast`), the regression would manifest as an indefinite hang caught only by nextest's 180s slow-timeout, not as a fast, readable assertion failure.
- **Suggested fix:** Wrap the `shutdown().await` call in that test with `tokio::time::timeout(Duration::from_secs(2), ...)` and assert `Ok(())`, so a reintroduced race fails fast with a clear message instead of a slow-timeout kill.

### 9. `safe_run`'s panic-survival contract is never actually exercised by a panicking tick
- **File:** `crates/shamir-server/src/scheduler.rs` (`safe_run` / `catch_unwind` wrapper around each periodic task) vs. `crates/shamir-server/tests/scheduler.rs`
- **Severity:** low (TDD gap)
- **Issue:** The scheduler wraps each periodic tick in a panic-catching guard so one bad tick doesn't kill the whole periodic task, but no test in `crates/shamir-server/tests/scheduler.rs` ever makes a GC/checkpoint stub `panic!()` inside a tick to prove the scheduler survives it and fires again on the next interval. All existing tests only prove each task type fires at least once under normal (non-panicking) conditions.
- **Suggested fix:** Add a scheduler test with a tick closure that panics on its first invocation and asserts a second successful invocation still occurs afterward.

### 10. `/readyz` is never observed returning "not ready" through the real boot path
- **File:** `crates/shamir-server/src/observability.rs:13-17` (documented boolean-ready contract) vs. `crates/shamir-server/tests/observability_http.rs` (`endpoints_return_expected_codes_and_content`)
- **Severity:** low (TDD gap on a documented contract's negative case)
- **Issue:** The existing HTTP test only asserts `/readyz` returns 200 *after* `launcher.launch().await` has already fully completed and `mark_ready()` has unconditionally run — the "should be 503 before listeners are bound" half of the documented contract is never driven through an actual in-flight boot sequence via HTTP; it's only indirectly inferable from `ObservabilityState::new()`'s default `false`.
- **Suggested fix:** Add a test that starts the observability HTTP server before the rest of the launch sequence completes (or with `mark_ready()` intentionally not yet called) and asserts a 503 from `/readyz` in that window.

## No findings for other reviewed areas

`cursor_registry.rs`, `tx_registry.rs`, `byte_budget.rs`, `registry.rs`'s cardinality tracking (`AtomicUsize` mirror, no banned `scc::*::len()` on the hot path), `access_tree.rs`, `backup.rs`/`restore.rs` (streaming SHA-256 manifest verification), `bootstrap.rs`, `config.rs`, `server_meta.rs`, `tables_registry.rs`, `tls.rs`, `user_directory.rs`, `version.rs`, `conn_limiter.rs`, `framer.rs`, `in_flight_guard.rs`, `push_sink.rs`, `connection_context.rs`, `user_state_lookup.rs`, `wire.rs`, and the `server/` boot-orchestration files were reviewed and found free of logic bugs and vacuous tests under this lens — these areas carry dense doc comments citing specific prior incidents (F-12, F-19, N-6, W-5, CR-A6, F-38, #439, #513, #527, etc.) and tests that force real failure conditions (corrupted msgpack, Windows sharing-violation locks, hand-crafted principal64 collisions) rather than tautological assertions.

</details>
