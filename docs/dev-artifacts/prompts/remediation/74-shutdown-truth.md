# Brief — SHUTDOWN-TRUTH shutdown-truth

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- `crates/shamir-server/src/server/server_handle.rs`, `crates/shamir-server/src/runtime.rs`, `crates/shamir-server/src/main.rs` (exit code), and the shutdown plumbing they call
- `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs`: ONLY the return/aggregation semantics of `flush_all_with_quiesce_timeout` and the system-store flush (the per-repo `shutdown_clean` call stays); another agent edits other regions of the engine
- `crates/shamir-server/src/observability.rs`: ONE new counter/gauge for unclean shutdown
- tests under `crates/shamir-server/src/tests/` or `crates/shamir-server/tests/`
- docs: ONLY the shutdown paragraph in `docs/guide-docs/guide/07-operations.md` (a small, accurate edit); the TRUTH-DOCS agent owns every other doc

## Task

SHUTDOWN-TRUTH (task #1321). The whole remediation series (SNAP-FIX, S5b, N1-live, S5c, W/B tasks) is MERGED on master, your base. Four independent read-only design reviewers have just reviewed the repository; their answers are appended as the binding rationale for this task. Verify every claim you act on against the code before changing anything (a reviewer's OBSERVED line numbers may have drifted by a few lines). Sibling agents run concurrently on other follow-ups; stay strictly inside your scope. Tests that park one commit and then await another INLINE hang (commit and non-tx writes now wait for the completion watermark W): spawn the second operation, never raise a timeout. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, no builds/tests (CI only), imports at the top of files, one primary export per file, tests under `tests/` dirs, builder-only queries in code and docs (except the documented wire-format docs).

DEFECT (reviewer-found, verify): a clean shutdown now requires W = allocation high-water after quiesce and drain (`RepoInstance::shutdown_clean`, `repo_instance.rs` ~:1852-1877) so that the persisted clean horizon J can be written; when that fails or the shared budget is exhausted, `flush_all_with_quiesce_timeout` (`db_management.rs` ~:629) returns an error, but the server logs it and `serve` still returns `Ok(())`, and the outer 30-second deadline also returns success after a warning (`server_handle.rs` ~:165, `runtime.rs` ~:52). So a shutdown that could not write J - which turns the NEXT start into an uncertified crash recovery with a conservative replication gap - is reported to the supervisor/operator as success.
REQUIRED:
1. `ServerHandle::shutdown()` (and whatever `serve` returns) must expose a distinct result: clean vs unclean, with the list of repositories that did not reach a clean horizon and the error text for each (a small `ShutdownReport`-style type, one primary export per file). Existing callers keep compiling; those that ignore it behave as before.
2. The process exit status of the server binary is non-zero (use a documented distinct code, e.g. 70 for 'shutdown not clean', not the generic 1) when the report is unclean; the log line is at ERROR level, emitted once, naming each repository and saying that the next start will certify a conservative replication gap.
3. A Prometheus counter `shutdown_unclean_total` (and the repository count of the last unclean shutdown as a gauge if it fits the existing style) in `observability.rs`.
4. Do not change the timing budgets (30 s outer, shared per-repo budget) and do not weaken what `shutdown_clean` requires.
5. Tests: injected flush failure on one of two repos -> unclean report naming only that repo, exit code mapping, counter incremented; deadline exhaustion -> unclean; fully clean shutdown -> clean report, J written (re-open certifies nothing). Use the existing shutdown and server fixtures (`shutdown_quiesce_tests`, the server shutdown e2e).
6. Docs: in `07-operations.md` state the exit code, what an unclean shutdown means, and that the next start certifies.

REVIEWER ANSWER (behaviour review; the 'normal stop can become an unclean restart' point and follow-up 4 are the relevant parts):

## Verdict: **conditional GO for first alpha; NO-GO for the current operational contract unchanged**

Keep the correctness changes. Do **not** restore eager snapshots, snapshot ANN, false settlement after projection failure, or silent subscription loss.

Before the first tag, require:
1. A distinct **committed-but-visibility-pending acknowledgement**, with a configurable bounded **post-commit** wait.
2. Operator-visible repository progress/degradation state, including stalled active tails and orphan replay.
3. Truthful shutdown reporting and corrected commit/retry documentation.

**One stuck lower version can indefinitely block acknowledgements for every higher version in the same repository.** This is not necessarily database-wide, and it does not block read-only/empty commits that allocated no version. `Unknown` tickets are different: they trigger reopen rather than indefinite visibility waiting. Persistent replay failures and live, wedged post-WAL tails have no equivalent automatic escape.

**Evidence standard:** **OBSERVED** below means inspected source behavior, not a reproduced runtime incident. **INFERRED** identifies production consequences. No builds, tests, benchmarks, or CI results were inspected.

## Decision table

| Item | Severity and alpha decision | Default/configuration decision | Required before first tag |
|---|---|---|---|
| **1. Result-size enforcement** | **Resource-safety win; compatibility break. Accept.** OBSERVED: server default is `true`; all four deployment examples enable it. The estimator charges cumulative materialization, including intermediates, not wire bytes or RSS. Encoded batch responses have an additional unconditional cap. `crates/shamir-server/src/config.rs:361`, `deploy/server.example.ktav:87`, `crates/shamir-engine/src/query/batch/result_size.rs:12`, `crates/shamir-server/src/db_handler/handler.rs:631`. | **ON is right.** Existing enforcement and size knobs suffice; disabling materialization enforcement must not disable wire protection. | Document separate materialization/wire limits and **post-execution rejection semantics**. Count refusals separately by stage; measure estimated bytes, encoded bytes, and peak RSS in CI. **Correction to the premise:** cursor rejection is `cursor_page_too_large`, not `result_too_large`; it uses the server page cap. `crates/shamir-server/src/db_handler/cursor_handlers.rs:315`, `crates/shamir-server/src/db_handler/handler.rs:747`. |
| **2. Exact snapshot vector ranking** | **Data-correctness win; substantial latency risk. Accept.** OBSERVED: Snapshot/Serializable dispatch bypasses live ANN for supported vector shapes; scoring scans snapshot-plus-overlay batches and sorts/truncates winners. `crates/shamir-engine/src/table/read_exec.rs:344`, `crates/shamir-engine/src/table/read_exec.rs:1965`. | **Exact is right. No “unsafe ANN in snapshot” knob.** Permit operators to restrict expensive queries or concurrency, not weaken isolation. | Existing limitations documentation is helpful. Require a CI scaling measurement over N, dimension, k, residual selectivity, and concurrency. Expose exact-plan frequency, scanned rows, and latency; query stats already record scanned rows. `crates/shamir-engine/src/table/read_exec.rs:2024`. A dedicated work/admission knob can follow unless measurements establish unacceptable resource exposure. |
| **3. W snapshots and acknowledgement waits** | **Data-correctness win; critical availability risk. Accept W, reject indefinite request residency as the only contract.** OBSERVED: allocated commits wait after the inner driver; read-only/empty outcomes skip the wait. `crates/shamir-engine/src/tx/commit.rs:587`. | Ordinary success must continue to mean visibility through the returned version. Add a configurable bounded acknowledgement wait returning a **distinct pending outcome**, not ordinary success or abort. Keep explicit strict/unbounded mode for callers that consciously require it. | Pending outcome, exported progress/age/waiter metrics, and a stalled-repository runbook. A gauge alone makes the incident diagnosable but does not release occupied request capacity. |
| **4. Post-WAL projection failure / follower partial apply** | **Data-correctness win; availability risk. Accept failure-closed settlement.** OBSERVED: failed data/index projection drops the guard and wakes orphan replay; successful replay records a changefeed gap before settling visibility. Follower partial apply has no local WAL replay owner. `crates/shamir-engine/src/tx/materialize.rs:258`, `crates/shamir-engine/src/tx/drainer.rs:1198`, `crates/shamir-engine/src/tx/apply_replicated.rs:456`. | **Do not configure false settlement.** Use the item-3 pending acknowledgement for known committed transactions. Expose explicit recovery-required/degraded state. | Distinguish active-tail stall, retrying orphan, and reopen-required states. Ensure follower supervisor/admission handles reopen as an operational stop, rather than generic endless backoff. |
| **5. Non-transactional write acknowledgement** | **Read-after-write correctness win; latency risk and compatibility break. Accept only with explicit degraded-outcome semantics.** OBSERVED: CRUD releases writer guards before waiting—essential to avoid replay cycles. `crates/shamir-engine/src/table/table_manager_crud.rs:258`. | Keep release-before-wait and migration no-wait exceptions. Apply acknowledgement policy consistently to embedded and server APIs. | **Current exception needs disclosure or a typed outcome:** on reopen, the helper logs and returns normally despite visibility not being established. `crates/shamir-tx/src/repo_tx_gate.rs:1113`, `crates/shamir-tx/src/mvcc_store/mod.rs:788`. Do not turn a completed write into a retryable failure. |
| **6. AsOf validation/waiting** | **Data-correctness win; compatibility break and latency risk. Accept.** OBSERVED: unallocated versions fail; allocated unsettled versions wait; reopen produces a typed error. `crates/shamir-engine/src/table/read_temporal.rs:85`. | **Right semantics.** Never silently clamp a requested version or substitute Latest. A cancellable read-wait deadline is safe and distinct from a commit deadline. | Document that AsOf can wait indefinitely and how callers cancel it; expose wait latency/degradation. An optional typed read-wait timeout is desirable, but not a blocker once the repository stall contract is explicit. |
| **7. Subscription termination and journal delivery** | **Data-correctness win; availability/compatibility tradeoff. Accept.** OBSERVED: 100 attempts mean **99 sleeps, 495 ms scheduled sleep**, not a hard 500-ms deadline. Gap/read failure terminates draining; snapshot failure returns before Ready. `crates/shamir-server/src/subscriptions/push.rs:76`, `crates/shamir-server/src/subscriptions/journal_drain.rs:78`, `crates/shamir-server/src/subscriptions/bridge.rs:501`. | Fail closed is right. Accept the retry window as an alpha default; do not require a tuning knob before measuring it. Terminal notifications are best-effort. | Export closure reasons, push retry/exhaustion counts, and commit-to-journal-to-delivery latency. Require clients to handle closure/absence of Ready, discard incomplete snapshots, and resynchronize. Current protocol already documents best-effort slow-consumer/closed notifications. `docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:304`. |
| **8. Cold-open certification and flush errors** | **Data-correctness win; major operational compatibility/availability break. Accept consciously as experimental replication.** OBSERVED: certification persists `(J,H]` conservatively; constructor recovery reaches certification before registry insertion; errors propagate. `crates/shamir-engine/src/repo/repo_instance.rs:1843`, `crates/shamir-engine/src/tx/recovery.rs:458`, `crates/shamir-engine/src/db_instance/db_instance.rs:55`. | **Fail closed is right. No bypass-certification knob.** Do not advertise automatic crash-transparent replication recovery. | Retain the existing reseed runbook and expose certification/shutdown failures. The documented workaround can require a **new source lineage**, not merely restarting a follower. `docs/guide-docs/guide/07-operations.md:89`. This is acceptable for alpha, not an HA claim. |

## Critical path: what actually escapes, and what does not

### 1. Reopen-required is an escape—but not repair

**OBSERVED:** `await_visible` checks W first, then reopen; pending waiters recheck at 50-ms intervals. On reopen, `commit_tx` changes the materialization outcome to Deferred. New transactional commits are refused at entry.

References: `crates/shamir-tx/src/repo_tx_gate.rs:1065`, `crates/shamir-tx/src/repo_tx_gate.rs:1089`, `crates/shamir-engine/src/tx/commit.rs:599`, `crates/shamir-engine/src/tx/commit.rs:814`.

An unresolved `Unknown` ticket sets reopen in the drainer; guard Drop also handles Unknown immediately. A `Pending` ticket merely stops the replay prefix.

References: `crates/shamir-engine/src/tx/drainer.rs:484`, `crates/shamir-tx/src/version_guard.rs:196`.

**Qualification:** the acknowledgement can subsequently encounter the separate, bounded five-second durability backpressure wait. Reopen is not necessarily an immediate wire response. `crates/shamir-engine/src/tx/commit.rs:615`, `crates/shamir-engine/src/tx/commit.rs:758`.

### 2. Persistent orphan replay failure can freeze W indefinitely

**OBSERVED:** finalization stops at a failed table. Orphan history failure leaves the version unsettled; changefeed-gap persistence must also succeed before completion. Background replay logs failure and retries on subsequent wake/interval passes. There is no failure-count escalation to reopen in these inspected paths.

References: `crates/shamir-engine/src/tx/drainer.rs:1094`, `crates/shamir-engine/src/tx/drainer.rs:1198`, `crates/shamir-engine/src/tx/drainer.rs:1210`, `crates/shamir-engine/src/tx/drainer.rs:1486`.

**INFERRED:** persistent storage errors, unavailable table registration, or failing gap persistence can hold W below the orphan forever. Higher-version acknowledgements accumulate even when their own projections succeeded.

### 3. A live wedged post-WAL tail is more dangerous than an orphan

**OBSERVED:** replay explicitly stops at active post-WAL ownership. It cannot safely replay concurrently with the live committer. Cancelling the caller transfers the suspended inner commit to a detached task rather than terminating ownership.

References: `crates/shamir-engine/src/tx/drainer.rs:546`, `crates/shamir-engine/src/tx/commit_driver.rs:91`.

**INFERRED:** an I/O operation that never completes can retain active ownership indefinitely. Caller disconnection does not necessarily convert it into a recoverable orphan.

There **is** a batch-operation diagnostic watchdog, but it only logs; it does not recover or fail-stop. Its registration covers executing aliases, whereas final transactional commit occurs after the plan finishes. Do not mistake it for a commit-tail watchdog.

References: `crates/shamir-engine/src/query/batch/op_watchdog.rs:11`, `crates/shamir-engine/src/query/batch/batch_execute.rs:562`, `crates/shamir-engine/src/query/batch/batch_execute.rs:864`.

### Smallest safe mitigation

Bound **only the acknowledgement wait after the inner commit has returned a known outcome**. Return transaction/version metadata plus a distinct `visibility_pending` state. Ordinary success still requires W ≥ version; pending callers must wait/reconcile, not replay writes.

This does **not** solve a stuck inner commit or establish the outcome of a Pending/Unknown WAL ticket. Those require a repository-level stall diagnostic/admission state and an explicit reopen/reconciliation procedure. Never report “committed” merely because a version was allocated.

## Production interactions the owner should expect

- **Repository head-of-line amplification.** OBSERVED: WAL windows share append/sync work, while visibility depends on contiguous version completion. INFERRED: batch-wide disk delay or one slow materialization tail amplifies acknowledgement p99 across otherwise unrelated tables. The WAL leader also drains successive windows before returning. `crates/shamir-wal/src/wal_group_commit.rs:208`, `crates/shamir-wal/src/wal_group_commit.rs:284`.
- **Cross-repository server capacity coupling.** OBSERVED: Execute reserves response bytes before execution and holds the reservation through commit/visibility waiting; dispatch tasks retain connection permits. INFERRED: a repository-local stall can occupy enough global byte budget to obstruct healthy repositories too. `crates/shamir-server/src/db_handler/handler.rs:562`, `crates/shamir-server/src/connection/request_loop.rs:323`.
- **The execution-time cap is not a commit deadline.** OBSERVED: the final deadline check precedes commit; no subsequent check bounds acknowledgement waiting. A configured 60-second execution cap does not imply a 60-second response bound. `crates/shamir-engine/src/query/batch/batch_execute.rs:814`.
- **Warning volume scales with waiters, not incidents.** OBSERVED: the five-second warning fires once **per wait**, not every five seconds. INFERRED: many affected requests still generate a warning burst; every parked waiter also has a 50-ms recheck timer. Aggregate/rate-limit by repository. `crates/shamir-tx/src/repo_tx_gate.rs:1095`.
- **Result rejection can conceal a completed write.** OBSERVED: wire-size enforcement happens after execution and subscription activation; non-transactional materialization charging can occur after an operation executes. INFERRED: `result_too_large` is not universally proof of rollback. Preserve committed metadata where possible and explicitly forbid blind retries. `crates/shamir-server/src/db_handler/handler.rs:595`, `crates/shamir-server/src/db_handler/handler.rs:631`, `crates/shamir-engine/src/query/batch/batch_execute.rs:577`.
- **Stalled W pins retention.** OBSERVED: the GC floor includes W even without explicit snapshots. INFERRED: continued writes above a stuck floor retain history/log state; unbounded waiting is not a memory bound. `crates/shamir-tx/src/repo_tx_gate.rs:774`.
- **A normal stop can become an unclean restart.** OBSERVED: shutdown requires W = allocation high-water after quiescence/drain. All repositories share one flush budget. The server logs flush failure, and `serve` still returns `Ok(())`; its outer 30-second deadline also returns success after warning. Consequently, a shutdown error is currently not reliably visible as process failure. `crates/shamir-engine/src/repo/repo_instance.rs:1872`, `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:629`, `crates/shamir-server/src/server/server_handle.rs:165`, `crates/shamir-server/src/runtime.rs:52`.

## Prioritized, PR-sized follow-ups

1. **P0 — Bounded visibility acknowledgement with truthful outcomes.**  
   Scope: `repo_tx_gate.rs`, engine `tx/commit.rs`, transaction outcome/wire types, DB transaction facade, server transaction handlers, clients. Add configurable post-inner-commit waiting and explicit pending metadata; preserve strict mode and WAL-durability distinctions. Handle non-tx outcomes without implying rollback.

2. **P0 — Repository progress/degradation diagnostics and admission policy.**  
   Scope: `completion_tracker.rs`, `repo_tx_gate.rs`, `version_guard.rs`, `commit_driver.rs`, `drainer.rs`, server observability/status. Export allocation H, W, history watermark, oldest blocking version/age/class, active tails, unresolved tickets, orphan retry/error state, waiters, and reopen-required. The slow counter exists, but waiter count is test-only and the inspected Prometheus bridge does not export these gate diagnostics. `crates/shamir-tx/src/completion_tracker.rs:133`, `crates/shamir-tx/src/completion_tracker.rs:147`, `crates/shamir-server/src/observability.rs:457`. Do not force-settle stalled versions.

3. **P0 — Follower recovery-required propagation.**  
   Scope: `tx/apply_replicated.rs`, replication `follower_loop.rs`, error classification and supervisor/status. Make partial-apply reopen state explicit and stop inappropriate retries/admission. Current apply errors take generic backoff, while the inspected apply allocation path has no reopen-entry check. `crates/shamir-server/src/replication/follower_loop.rs:395`, `crates/shamir-engine/src/tx/apply_replicated.rs:396`.

4. **P0 — Truthful shutdown and retry documentation.**  
   Scope: server `server_handle.rs`, `runtime.rs`, shutdown status/error plumbing; `TRANSACTION_OUTCOMES.md`, operations and limitations guides. Surface unsuccessful clean shutdown as a distinct result. Correct “Deferred means next-open-only recovery”: online replay now participates, and Deferred is not itself a visibility-status field. `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md:65`. Include result-cap post-commit ambiguity and non-tx reopen behavior.

5. **P1 — CI-only performance/operability characterization before tag.**  
   Scope: existing engine/server benchmark and CI harnesses, observability instrumentation. Measure exact-vector scaling; group-commit acknowledgement p50/p99/max; prolonged lower-version stalls; global byte-budget occupancy; replay retry/log rate; estimated/encoded/RSS ratios; subscription delivery lag and slow-consumer closures. Use results to decide vector admission and subscription retry knobs—not to weaken consistency.

### Behaviors to consciously accept as-is

Accept conservative result estimates, exact snapshot vector search, contiguous snapshots, failure-closed projection settlement, typed AsOf rejection, terminal subscription gaps/backpressure, conservative cold-open certification, and propagated flush errors. Also accept **manual replication reseeding and potentially expensive snapshot reads as explicit alpha limitations**.

Do **not** accept indefinite opaque acknowledgement stalls, success-looking degraded non-tx acknowledgements, or success-looking unsuccessful clean shutdowns without the above operational contract.

**Files changed:** none. **Builds/tests run:** none, as requested. **Notable:** conclusions are a static design review; production latency and failure frequency remain unmeasured.
