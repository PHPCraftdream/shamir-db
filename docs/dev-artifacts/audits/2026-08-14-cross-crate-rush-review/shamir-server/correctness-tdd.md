<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The read-only transaction bypass, supervisor liveness gap and bridge teardown race remain source-demonstrable. Prefix matching is intentional. Several remaining entries concern test or diagnostic quality rather than current runtime failures.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 9 | 0 | 0 | 2 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Interactive-tx path bypasses the read-only-replica gate entirely

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

With an explicitly ReadOnly handler and a principal authorized to write the target resources, TxBegin → TxExecute(insert) → TxCommit reaches the engine without NodeMode rejection. Database/table authorization still applies. Registered node-mode tests call Execute, so they do not catch this path.

Evidence: [crates/shamir-server/src/db_handler/handler.rs:529](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L529); [crates/shamir-server/src/db_handler/tx_handlers.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L168); [crates/shamir-server/src/db_handler/tx_handlers.rs:247](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L247); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L126); [crates/shamir-server/src/db_handler/tests/node_mode_tests.rs:157](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tests/node_mode_tests.rs#L157).

<a id="review-2"></a>

### Claim 2 — Dead follower-loop registry entries block resubscription after a journal gap

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A terminal StaleLeaderEpoch return leaves the catalogue active and the dead task registered; reconcile checks contains_sync rather than joins. A gap normally becomes resync_required and is removed on the next inactive-state reconcile, but reactivation before that reconcile also preserves the stale entry. The existing gap test never tests restart.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L249); [crates/shamir-server/src/replication/supervisor.rs:213](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L213); [crates/shamir-server/src/replication/supervisor.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L231); [crates/shamir-server/src/replication/supervisor.rs:286](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L286); [crates/shamir-server/src/replication/tests/supervisor_tests.rs:402](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/tests/supervisor_tests.rs#L402).

<a id="review-3"></a>

### Claim 3 — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Reserve placeholder, remove it during close_all, spawn/attach a still-running bridge: scc 3.8.4 update_sync does not invoke its closure on a missing key, dropping the captured handle. Tokio 1.49.0 JoinHandle drop detaches rather than aborts. Teardown races with dispatch on a multithread runtime. The late-attach test checks counts using an empty task, not live-task termination.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L134); [crates/shamir-server/src/subscriptions/registry.rs:150](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L150); [crates/shamir-server/src/db_handler/subscribe_handler.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/subscribe_handler.rs#L87); [crates/shamir-server/src/connection/request_loop.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L48); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="review-4"></a>

### Claim 4 — Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Repeated non-regressing Hello replies to Pull reach an immediate continue. A malformed custom or authenticated upstream source can trigger rapid retries; the normal leader path need not produce this variant. Retry rate and CPU impact depend on transport latency and are unmeasured.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L265); [crates/shamir-server/src/replication/follower_loop.rs:281](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L281); [crates/shamir-server/src/replication/follower_loop.rs:287](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L287).

<a id="review-5"></a>

### Claim 5 — `try_join_next()` swallows non-panic `JoinError`s silently

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The branch is silent, but the inspected cancellation producer is abort_all during teardown, whose separate drain intentionally discards results. No currently reachable individual-cancellation request-loss mechanism was found. This is optional diagnostics debt, not a demonstrated availability defect.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L240); [crates/shamir-server/src/connection/request_loop.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L415).

<a id="review-6"></a>

### Claim 6 — Vacuous dead-code discard of `ConnectError::AuthFailed`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The imported enum is referenced only by the final discarded value. Removal changes no behavior; the code does not establish an omitted historical error-handling obligation.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L42); [crates/shamir-server/src/connection/request_loop.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L431).

<a id="review-7"></a>

### Claim 7 — Log-mask override matching is a raw substring/prefix test, not a namespace-segment match

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Longest raw-prefix matching is documented and tested, including wal matching wal_compact. Requiring :: would break intended underscore-prefix behavior. A hypothetical walnut target is not counter-evidence to that contract.

Evidence: [crates/shamir-server/src/logging.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L135); [crates/shamir-server/src/logging/tests/log_mask_tests.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging/tests/log_mask_tests.rs#L64); [docs/guide-docs/guide/07-operations.md:321](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/07-operations.md#L321).

<a id="review-8"></a>

### Claim 8 — `Scheduler::shutdown()` regression test has no timeout bound on the documented race it guards against

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The immediate-shutdown test lacks a local timeout, but runner timeouts already detect a hang. Production receivers are created before spawning, eliminating the specified lost-send window. A short local deadline improves localization rather than adding otherwise absent hang detection.

Evidence: [crates/shamir-server/tests/scheduler.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L153); [crates/shamir-server/tests/scheduler.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L156); [crates/shamir-server/src/scheduler.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L126); [.config/nextest.toml:49](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.config/nextest.toml#L49); [.config/nextest.toml:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.config/nextest.toml#L63).

<a id="review-9"></a>

### Claim 9 — `safe_run`'s panic-survival contract is never actually exercised by a panicking tick

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Production ticks use catch_unwind, but registered scheduler mocks only increment counters. Removing the panic catcher would not fail these tests. The discriminating oracle is a first invocation that unwinds followed by an observed successful later tick.

Evidence: [crates/shamir-server/src/scheduler.rs:314](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L314); [crates/shamir-server/tests/scheduler.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L44); [crates/shamir-server/tests/scheduler.rs:174](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L174).

<a id="review-10"></a>

### Claim 10 — `/readyz` is never observed returning "not ready" through the real boot path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The handler has a real false→503 branch, but the HTTP test observes only 200 after launch. Stock observability is started after data listeners bind, so its negative branch should be tested through direct spawn before mark_ready, not an invented pre-listener boot window.

Evidence: [crates/shamir-server/src/observability.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L75); [crates/shamir-server/src/observability.rs:557](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L557); [crates/shamir-server/tests/observability_http.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L149); [crates/shamir-server/src/server/server_launcher.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L878).

<a id="review-no-findings-for-other-reviewed-areas"></a>

### Claim No findings for other reviewed areas — Other areas are free of logic bugs and vacuous tests

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Positive counterexamples include registry_insert_and_remove performing neither operation, the count-only attachment oracle, and response-budget tests taking the guard inside a scope unlike production. The newly identified production guard-lifetime defect further defeats the clean conclusion.

Evidence: [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L4); [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L48); [crates/shamir-server/src/db_handler/tests/byte_budget_exhaustion_tests.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tests/byte_budget_exhaustion_tests.rs#L126); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348).

## Evidence and recipe corrections

- Non-panic JoinError logging and a local scheduler deadline are diagnostics/test-localization nits; no current individual-cancellation failure was established.
- A missing-slot abort alone does not prevent subscriptions admitted after close_all. Teardown must close admission or cancel/drain dispatch before its final bridge sweep.
- Bootstrap gate unit tests copy the caller's match; changing production to omit that match would not necessarily fail them.
- The response-budget helpers claim production-equivalent scoping but retrieve the guard inside the scope, whereas production retrieves it afterward.

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
