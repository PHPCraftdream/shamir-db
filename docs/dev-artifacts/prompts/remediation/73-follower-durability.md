# Brief — FOLLOWER-DURABILITY follower-durability

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

- `crates/shamir-engine/src/tx/apply_replicated.rs` (apply/commit tail, marker persistence, reopen check at version allocation)
- `crates/shamir-engine/src/meta/repl_bookmark.rs`, `crates/shamir-engine/src/meta/recovery_marker.rs` (explicit flush entry points)
- `crates/shamir-engine/src/repo/repo_instance.rs`: ONLY the gate-seeding region (~:880-910, where recovery-marker read/decode errors are swallowed and the floor is `max(marker, WAL)`); other agents edit other regions of this file
- `crates/shamir-server/src/replication/{follower_loop,supervisor}.rs` and their error/status types; `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` ONLY for status text of the new terminal state
- tests: `crates/shamir-server/src/replication/tests/` and `crates/shamir-engine/src/tx/tests/` (new files + one-line `pub mod`)
- NO docs (the TRUTH-DOCS agent owns `KNOWN_LIMITATIONS.md` / `TRANSACTION_OUTCOMES.md`); update only the module docs inside the Rust files you touch

## Task

FOLLOWER-DURABILITY (task #1320). The whole remediation series (SNAP-FIX, S5b, N1-live, S5c, W/B tasks) is MERGED on master, your base. Four independent read-only design reviewers have just reviewed the repository; their answers are appended as the binding rationale for this task. Verify every claim you act on against the code before changing anything (a reviewer's OBSERVED line numbers may have drifted by a few lines). Sibling agents run concurrently on other follow-ups; stay strictly inside your scope. Tests that park one commit and then await another INLINE hang (commit and non-tx writes now wait for the completion watermark W): spawn the second operation, never raise a timeout. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, no builds/tests (CI only), imports at the top of files, one primary export per file, tests under `tests/` dirs, builder-only queries in code and docs (except the documented wire-format docs).

DEFECTS (reviewer-found, verify each): (1) follower apply and bookmark advancement are separate calls (`follower_loop.rs` ~:352-362); the bookmark is persisted with a plain `Store::set` without an explicit flush (`repl_bookmark.rs` ~:57-58, `recovery_marker.rs` ~:62-75) and `apply_replicated` marks the local version durable without flushing history (~:515-541), so after a crash a durable bookmark can outrun durable follower data: the next pull starts too far ahead and SILENTLY misses source data, which no leader-side gap can detect. (2) gate seeding swallows recovery-marker read/decode errors and uses `max(marker, WAL)` (`repo_instance.rs` ~:888-907); the follower has no local WAL and treats marker persistence failure as non-fatal (`apply_replicated.rs` ~:68-71,521-533), so after a crash the follower's version floor can sit BELOW versions already present in its history (version reuse; downstream chain replicas then miss obligations). (3) a partial follower apply sets `requires_reopen` but the follower loop treats the resulting error as generic retryable backoff and `apply_replicated` has no reopen check at version allocation, so it keeps retrying against a dead gate. (4) two subscriptions for the same (db, repo) start two loops writing one destination.

REQUIRED:
1. Ordering invariant, stated in code docs and enforced: for every applied event the follower's history writes are flushed/durable BEFORE the bookmark that covers them is made durable, and the bookmark has an explicit flush before the loop advances or acknowledges anything. Use the existing flush entry points of the stores (find them; add a narrowly named method if none exists); keep the cost at one flush pair per applied batch (the loop applies in batches - batch the flush, not per event), and say so in the report with the measured shape from the code. A durable bookmark must never authorize skipping absent data.
2. Version-floor evidence for the WAL-less follower: after a successful apply, the recovery marker covering `local_version` is persisted WITH a flush before `Applied` is returned; if that persistence fails the follower must fail-stop (set `requires_reopen`, return a typed terminal error) instead of continuing with a floor that does not cover published versions. Gate seeding must PROPAGATE marker read/decode errors (the open fails) instead of swallowing them; keep the existing max(marker, WAL) rule otherwise. Do not add a history scan.
3. `apply_replicated` checks `requires_reopen` at version allocation and returns a typed error (for example `DbError::RepoRequiresReopen`; look for an existing variant first). The follower loop classifies it as TERMINAL (like `JournalGap`): stop, do not backoff-retry, and the supervisor persists a degraded/`reopen_required` state that the existing status paths (`ReplicationStatus` / `ListSubscriptions`) show; `Resume` must not silently restart a loop against a gate that still requires reopen.
4. One replication owner per destination repository: the supervisor refuses to start a second loop for the same (db, repo) with a typed error and a clear status message.
5. Tests (deterministic; recording/failing store fixtures from the existing replication and recovery tests; no wall-clock sleeps): a recording store proves history flush precedes the bookmark flush; a crash simulated between data apply and bookmark flush resumes at the OLD bookmark and converges (events re-applied idempotently); a crash between bookmark set and bookmark flush cannot leave a durable bookmark without durable data; marker persistence failure after a published apply fail-stops with `requires_reopen`; marker decode error fails the open; partial multi-table apply -> reopen-required terminal state visible in the status and `Resume` refused; a duplicate subscription for the same repository is rejected. Existing replication/apply tests stay green (adapt fixtures, never weaken assertions).
Hot path: follower apply (one flush pair per batch, no extra allocations). State this in the report; bench gate owed in CI.

REVIEWER ANSWER (storage-durability and recovery-required findings are the relevant parts; the seed-artifact PRs 2-4 are NOT in this task):

## Verdict

**Prioritize B: a bounded, operator-driven recovery path built around a verified snapshot + matching upstream bookmark. Keep the conservative detector. Do not fund S5c-X now; defer automatic reseed until the manual recovery primitive is proven.**

The ordering should be **correctness prerequisites → offline R2-lite → automatic R2 when operational demand justifies it → exact detection only if false-positive recovery cost becomes material**. Keep replication explicitly experimental throughout.

The central product problem is not “we sometimes stop unnecessarily”; it is **“after stopping safely, we have no supported way to recover the existing lineage.”** Exact detection improves availability when the journal survived. It does not repair an actual missing tail, bootstrap a new follower past a persisted interval, or make follower bookmark durability safe.

### Evidence terminology

**OBSERVED** below means directly read in this checkout—not runtime verification. **INFERRED** means a consequence or risk derived from those mechanisms. No builds, tests, or CI checks were performed.

## 1. Why B is the right investment

**OBSERVED:** The gap remains effective whenever the requested version is at or below its horizon: `crates/shamir-tx/src/changefeed.rs:645-648`. The follower checks that gap before decoding or applying events: `crates/shamir-server/src/replication/follower_loop.rs:305-325`. `Resume` merely sets catalogue state to `active`: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:385-387`.

**INFERRED:** An installed, verified follower image containing the source state through version **V ≥ affected horizon**, with upstream bookmark **V**, can resume at **V+1** without deleting or bypassing the leader’s gap certificate.

**Do not implement a leader-side “ignore this interval for follower X” switch.** The current pull semantics are version-based, not a repair-attestation protocol. Exempting a follower with an older bookmark would allow an actually missing event to be silently omitted. If its matching image already covers the horizon, the exemption is unnecessary.

Choose **offline, stopped-source seeding** for the first supported recovery path. This deliberately exchanges downtime for a much smaller consistency surface:

- Stop source writes and DDL; finish recovery and clean quiescence.
- Export authoritative logical state, schema and field-name information at a verified settled version V.
- Install into a new, isolated follower repository.
- Persist the image and its source identity/epoch/bookmark before making it eligible for replication.
- Resume from V+1; retain the source’s gap certificate for all other consumers.

Existing backup is **stop-and-copy server backup**, not a replication seed: `crates/shamir-server/src/backup.rs:3-11,29-32`. A backup file checksum does not establish that a follower’s data matches an upstream bookmark.

## 2. Smallest next PR-sized steps, in order

These are proposed scopes, not changes made in this review.

### PR 1 — Establish the durable follower progress invariant

**Scopes:**  
`crates/shamir-engine/src/tx/apply_replicated.rs`;  
`crates/shamir-engine/src/repo/repo_instance.rs`;  
`crates/shamir-engine/src/meta/repl_bookmark.rs`;  
`crates/shamir-server/src/replication/{follower_loop,supervisor}.rs`; existing replication tests.

Require one replication owner per destination repository, and ensure **all event data reaches the explicit durability boundary before its bookmark can become durable**. Partial apply errors must not be treated as ordinary indefinitely retryable progress if the engine requires reopening.

CI acceptance: crash between data application, data flush, bookmark set and bookmark flush; duplicate subscriptions; multi-table partial failure. A durable bookmark must never authorize skipping absent data.

This is a prerequisite for both manual and automatic recovery—not exact journal detection.

### PR 2 — Define and export an offline seed artifact

**Scopes:** engine repository snapshot/export boundary; server backup/CLI surface; replication recovery documentation.

Artifact must bind **source lineage, epoch, database/repository, settled V, schema, interner representation, data and integrity checks**. Freeze writes and DDL for this first implementation; export logical records or provide verified interner translation, rather than assuming leader IDs are portable.

Use the settled position, not latest publication diagnostics: `crates/shamir-engine/src/repo/repo_instance.rs:914-918`.

CI acceptance: export refusal without quiescence; schema/interner mismatch; corrupt or incomplete artifact; seed V below the certified horizon.

### PR 3 — Install the seed with crash-safe readiness

**Scopes:** engine seed installation and metadata; server CLI/restore surface; supervisor readiness checks.

Install only into an isolated destination. Publish an installation-complete marker **after** data, metadata and matching upstream bookmark are durable. Incomplete installation remains unservable/unreplicable across restart. Do not expose a standalone “advance bookmark” repair command.

Initially support the existing follower envelope: matching schema, no secondary indexes, controlled local writes. Preserve leader-local and follower-local version domains separately.

CI acceptance: interruption at every installation boundary; restart refuses partial image; completed seed resumes V+1 against the original gapped leader without changing its certificate.

### PR 4 — Expose the operator workflow and prove the round trip

**Scopes:** `admin_replication.rs`, replication query types/builders if an admin interface is chosen, supervisor state handling, existing server recovery tests, `guide/07-operations.md` and `KNOWN_LIMITATIONS.md`.

Connect verified installation to an explicit recovery-complete/resume action. Report source identity, seed version and installation status—not merely catalogue `active`.

CI acceptance: real leader crash → `resync_required` → offline seed → resume → subsequent writes converge; unrelated unseeded followers still stop.

**Do not describe the whole seed feature as a one-PR metadata toggle.** The command is small; the consistency contract is not.

## 3. When the other options become right

| Option | Decision now | What would change that decision |
|---|---|---|
| **A — Exact detector** | Defer. The supplied design’s WAL change, obligation ledger, legacy reconciliation and readiness work buy primarily fewer uncertainty-driven stops—not recovery. The 4–5 PR estimate is prior-review context, not independently established here. | Manual recovery exists; CI establishes the recovered-floor and follower durability invariants; measured intact-journal crash incidents make unnecessary reseeding a substantial operational cost; startup/migration costs are acceptable. |
| **C — Automatic R2** | Build on B later, not before it. Automating an unproven install contract multiplies failure modes. | Replicas are actually operated unattended, recovery downtime matters, and snapshot/schema/interner/epoch consistency plus interrupted-install recovery are proven. The roadmap already places snapshot bootstrap, schema and retention in R2: `docs/dev-artifacts/roadmap/REPLICATION.md:624`. |
| **D — Spend nothing** | Defensible release positioning, but not my recommendation for this remediation program. A supported manual escape hatch is the best bounded investment. | The owner explicitly shelves replication and accepts rebuilding lineages for experiments. There is no production promise forcing R2: `README.md:69-74`; the roadmap says future features are non-binding: `docs/dev-artifacts/roadmap/ROADMAP.md:3`. |

## 4. Current correctness risks the owner should know

### High priority: follower bookmark can outrun durable follower data

**OBSERVED:** Apply and bookmark advancement are separate calls: `crates/shamir-server/src/replication/follower_loop.rs:352-362`. Bookmark persistence ultimately performs `Store::set`, without an explicit flush: `crates/shamir-engine/src/meta/repl_bookmark.rs:57-58` and `crates/shamir-engine/src/meta/recovery_marker.rs:62-75`. Apply marks the local version durable after history writes, but this path does not explicitly flush history before returning: `crates/shamir-engine/src/tx/apply_replicated.rs:515-541`.

**INFERRED:** If bookmark persistence survives while preceding data does not, the next pull starts too far ahead and silently misses source data. A leader gap detector cannot diagnose this follower-local loss. **Not reproduced or established on the on-disk backend in this review.**

### High priority: certification coverage depends on a possibly understated H

**OBSERVED:** Gate construction swallows recovery-marker read/decode errors and uses `max(marker, surviving WAL maximum)`, not retained history: `crates/shamir-engine/src/repo/repo_instance.rs:888-907`. Follower apply has no local WAL and treats recovery-marker persistence failure as nonfatal: `crates/shamir-engine/src/tx/apply_replicated.rs:68-71,521-533`.

**INFERRED:** Surviving follower-written history above that recovered floor can lie outside `[J+1,H]`; downstream replication may therefore have missing obligations not covered by certification. Version reuse is also a concern. This is expressly outside the documented policy’s coverage: `docs/guide-docs/KNOWN_LIMITATIONS.md:572-585`.

**Recommendation:** Treat trustworthy durable version-floor evidence as a separate correctness requirement. Do not equate preserving a conservative detector with proving complete crash-gap coverage.

### Open refusal now depends on changelog metadata and write availability

**OBSERVED:** Normal factory open calls recovery before returning: `crates/shamir-engine/src/repo/repo_instance.rs:311-327`; recovery invokes certification: `crates/shamir-engine/src/tx/recovery.rs:458`. Clean-horizon decode/read errors propagate: `crates/shamir-engine/src/repo/changelog_store.rs:26-37`. GAP/J writes require explicit flushes: `crates/shamir-engine/src/repo/changelog_store.rs:64-85`.

**INFERRED:** Corrupt J or unavailable certificate writes can prevent opening an otherwise recoverable data repository, even when replication is unused. This is deliberate fail-closed behavior, but it needs an offline diagnostic/recovery procedure—not instructions to delete metadata. Certification awaits have no explicit deadline: `crates/shamir-tx/src/changefeed.rs:499-510,535-547`; a stalled backend can also stall open.

### Do not use stale limitation text as the implementation oracle

**OBSERVED:** `KNOWN_LIMITATIONS.md:542-545,560-562` still says field-name translation is absent and unknown tables are skipped. Current code translates and persists interner bindings (`apply_replicated.rs:348-372`), rejects unknown tables (`:220-227`), and makes rejection terminal (`follower_loop.rs:389-393`).

Those two advertised silent-loss mechanisms are **not supported by the current code I read**. Update the documentation in the recovery work.

## Verification boundary

I inspected certification/checkpoint ordering, marker durability calls, bounded gap reads, follower application/bookmark flow, supervisor terminal-state handling, admin resume behavior, existing recovery-test source, backup semantics and release/roadmap positioning. I did **not** verify runtime durability, reproduce crashes, inspect CI results, or independently re-prove the entire exact-detector design.

**Files changed: none. Builds/tests run: none, as requested.**

----

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
