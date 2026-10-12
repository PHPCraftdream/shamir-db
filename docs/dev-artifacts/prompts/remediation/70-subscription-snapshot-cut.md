# Brief — N1L-4 exact initial-snapshot handoff at a consistent cut

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

- `crates/shamir-server/src/subscriptions/bridge.rs`: ONLY the `initial` snapshot branch (~:421-490) and the cursor handoff after it; the journal drain (`journal_drain.rs`) must not change except for one small cursor-advance entry point if unavoidable
- `crates/shamir-db/src/shamir_db/shamir_db/` (a new small file or `changelog.rs` / `execute/db_tx.rs` neighbour) ONLY if the facade needs a thin helper to run the snapshot reads inside one Snapshot transaction and to return that transaction's snapshot version
- tests: `crates/shamir-server/src/subscriptions/tests/` NEW `bridge_snapshot_cut_tests.rs` (+ one-line `pub mod`); you may adjust the existing `bridge_ordering_tests.rs` pre-snapshot-W test, whose at-least-once expectation this task replaces
- docs: ONLY the sentences about duplicates at the initial-snapshot handoff in `docs/guide-docs/KNOWN_LIMITATIONS.md` and `docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md` (they must now describe the exact handoff)
- NO engine / shamir-tx changes, NO wire format change, NO TS client changes

## Task

N1L-4 (task #1310; umbrella N1-live #1295). N1L-1, N1L-2, N1L-3 (#125, #122, #128: the bridge is journal-authoritative) and SNAP-1/2/3/3a (#115, #116, #118, #127: Snapshot/Serializable transactions read at the contiguous completion watermark W, `commit_tx` acknowledges only once W >= its version) are MERGED on master, your base. Sibling agents work concurrently on SNAP follow-ups in `shamir-tx` / engine commit paths and on the S5c crash certification (repo open path); stay inside your scope.

DEFECT: with `initial = true` the bridge runs independent snapshot queries (`db.execute_as` per target, `bridge.rs` ~:421-465), each reading latest state at its own moment, after seeding the journal cursor from W sampled BEFORE the queries (N1L-3 made that loss-free but at-least-once: a row committed during the snapshot can appear both in the snapshot and again from the journal, and different targets are not mutually consistent).
THIS STEP: make the handoff EXACT, using SNAP-3: run ALL snapshot reads of one repo inside ONE read-only Snapshot transaction (use the facade's interactive transaction API, `tx_begin_as` / `tx_execute_as` and a rollback/drop at the end - read `crates/shamir-db/src/shamir_db/execute/db_tx.rs`; the transaction is single-repo, so use one transaction per repo of the subscription; it must run under the subscriber's actor so authorization and filters are unchanged; budget/limits per the existing policy for server-owned reads), take that transaction's `snapshot_version` S (it equals W at begin), deliver the snapshot frames (version-zero convention, Ready ordering unchanged), then set the repo's journal cursor to `max(C, S)` and let the existing journal drain deliver strictly `> S`. Rows committed at versions <= S are in the snapshot and are NOT replayed; rows committed > S are replayed exactly once; targets of one repo are mutually consistent. Behaviour change (required): today a failed snapshot query is only logged and the subscription continues to Ready with a partial snapshot, which is wrong. A snapshot/transaction failure must terminate the subscription with the existing terminal notifications and no Ready; say so in the report. Keep the pre-snapshot W sampling as the cursor floor for the live-only path; the cut S replaces it only for `initial = true`.
Also verify `from_version + initial`: the backfill drain (stop_when_clean) runs first, then the snapshot at S >= the drained bound, then live from S + 1.
Tests (deterministic; use a small test seam in the bridge or the facade helper to interleave a commit between the snapshot transaction begin and the end of its reads, spawning the committer - never awaiting it inline while it may wait for W): a row committed after S appears exactly once (via the journal, not in the snapshot); a row committed at <= S appears exactly once (in the snapshot, never replayed); two targets of one repo are consistent (both read at S); a failed snapshot terminates the subscription (no Ready); `from_version + initial` ordering; filters/authorization unchanged for snapshot frames; existing e2e subscription tests (`crates/shamir-server/tests/subscriptions_e2e.rs`) keep passing.
Docs: replace the at-least-once caveat by the exact handoff statement; remaining honest caveats: no delivery acknowledgement or exactly-once across disconnects, journal-writer latency, crash can lose the journal tail (S5c, partially certified).
Constraints: commit hot path untouched; no new Mutex/RwLock; one primary export per file; imports at the top; tests under `tests/`. Do NOT delegate to child agents or wait on schedules; finish the work yourself in this worktree. No git mutation, no builds/tests (CI only).

REVIEWER DESIGN (N1-live; the section 'Initial snapshot is a separate handoff invariant'):

## Verdict

**Adopt broadcast-as-wake-up-hint, with all subscription change delivery sourced from the W-bounded journal. Do not add a reorder buffer or an ordered dispatcher in the first implementation.**

**Approval is conditional on three correctness requirements:** expose the journal’s actual confirmed read bound to the bridge; eliminate silent push drops; replace the unsafe initial-snapshot watermark handoff. This provides ordered, duplicate-free delivery **while the subscription remains healthy and its journal history is available**. Unconditional exactly-once delivery through journal loss, connection failure, or client-side drops is impossible under the existing protocol.

**OBSERVED — the checkout differs from the stated premise:** S5b-2 is **not present in this tree**. `RepoInstance::read_changelog_from` still calls unbounded `read_from` at `crates/shamir-engine/src/repo/repo_instance.rs:1522-1528`. The follower also still publishes before journal enqueue at `crates/shamir-engine/src/tx/apply_replicated.rs:507-538`. Both are implementation dependencies, not assumptions to waive.

Here, **OBSERVED** means inspected source, not runtime reproduction. Recommendations and performance expectations below are **INFERRED/design judgments**.

## 1. Why this approach

The necessary ordering invariant already exists for local commits: enqueue-or-gap immediately precedes `guard.commit()` without an intervening await (`crates/shamir-engine/src/tx/publish_journaled.rs:21-29`). Journal keys/read order are ascending, and bounded decoding excludes versions above the supplied bound (`crates/shamir-engine/src/repo/changelog_store.rs:45-85`; `crates/shamir-tx/src/changefeed.rs:475-518`).

The current bridge instead advances its watermark on arrival, before delivery, then ignores lower versions (`crates/shamir-server/src/subscriptions/bridge.rs:408-417`). **INFERRED:** B-before-A broadcast delivery therefore loses A exactly as described.

| Alternative | Decision |
|---|---|
| **Per-subscriber reorder buffer** | Reject. W settles versions, not whether each version has an event. Aborts and empty-footprint commits produce no journal event (`crates/shamir-tx/src/changefeed.rs:583-585`). Correct gap detection therefore still needs journal/completion information. Buffers multiply retained payloads by subscriber count and require bounded overflow/recovery logic. |
| **Await W before broadcasting each event** | Reject. Awaiting belongs off the commit path. Moreover, concurrent waiters waking together still send in scheduler order, not version order. A separate sequencer would still be necessary. |
| **Single W-ordered dispatcher per repo** | Defer. It can amortize journal reads and decoding across many subscribers, but introduces lifecycle, bounded batch retention, slow-reader isolation, and catch-up/live handoff machinery. Ordered broadcast still needs journal recovery on lag. Use it only if fanout measurements justify it. |
| **Journal-authoritative bridge** | Choose. One ordering source, bounded per-subscription memory, no added commit lock/await/allocation. Cost moves to readers, with journal-writer latency added to delivery. |

“No added lock” means no **new** locking primitive on the commit path; preserve the existing non-awaiting channel operations rather than claiming their internals are lock-free.

## 2. Exact bridge algorithm

### State and activation

Per subscription, maintain per repo:

- **Processed-through cursor `C`**, distinct from “last version that produced a client frame.” Filtered-out events must advance `C` too.
- Maximum observed **hint target `H`**.
- Pending drain/confirmation state.
- One bounded journal page and, while delivering an event, its **change index**.
- Existing subscription-wide envelope `seq`, target index, filter slots, actor and delivery mode.

Versions order transactions; several record frames from one transaction have the **same** `commit_version`. The correct wire guarantee is nondecreasing frame versions, with each transaction’s selected changes delivered before the next transaction.

1. **Attach receivers before catch-up**, preserving `bridge.rs:189-200`.
2. For `from_version`, preserve its existing inclusive starting semantics: the current read starts at `fv` (`bridge.rs:225-230`). Thereafter resume at `C + 1`, with checked overflow.
3. For live-only subscriptions without a resume cursor, establish an explicit **activation cut using W**, not a post-snapshot `last_committed` maximum. Events through that cut are excluded unless requested through resume/snapshot semantics.
4. Schedule an initial drain after registration. Do not require a future commit to wake an already-present backlog.

### Wake and drain

On a hint:

1. Update `H = max(H, hinted_version)`; mark the repo dirty. **Never discard a hint merely because its version is below the newest hint.** An older tail may be the signal that progress is now possible.
2. Sample **W = `gate.visible_floor()`**, which is the completion watermark (`crates/shamir-tx/src/repo_tx_gate.rs:1035-1039`).
3. Confirm W with the existing approximately one-second barrier budget.
4. Let **B be the returned confirmed bound, capped at sampled W**. Read only the ascending journal prefix through B.
5. Process each event through the **existing live filtering/delivery path**, not the less-cached backfill implementation.
6. Advance `C` to the event version only after every selected change has been accepted by the push sink; filtered changes count as processed.
7. Continue paging from `C + 1` until the bounded prefix is exhausted. **Do not treat a short page as exhaustion:** the storage adapter can return early on its 16 MiB byte budget (`crates/shamir-engine/src/repo/changelog_store.rs:63-79`). An empty bounded page establishes exhaustion.
8. Once exhausted without a gap, advance the processed-through cursor to B, covering aborted/empty versions without inventing events.
9. Apply a bounded work quota/yield between pages so catch-up cannot monopolize the runtime.
10. Coalesce accumulated hints and recheck progress before parking.

**Required internal API:** return the actual B and confirmation outcome alongside the page, or expose a fixed-bound drain handle. Existing `JournalRead` contains only `events` and `gap_at` (`crates/shamir-tx/src/changefeed.rs:209-214`). The bridge cannot safely infer B from the last event or from an empty result.

### Exceptional cases

- **`Lagged`:** treat it as another drain request, not evidence of data loss. Recover from `C + 1` through the journal. Remove the unconditional Gap emission currently at `bridge.rs:566-578`; emit Gap only when journal recovery reports a gap.
- **Hint ahead of W:** drain the currently safe prefix, then park an off-hot-path `await_visible(H)` progress wait, multiplexed with other repos/cancellation. This handles an earlier unresolved version without waiting for another payload broadcast. The existing wait is cancellation-safe and detects reopen (`repo_tx_gate.rs:1041-1092`).
- **Confirmation timeout:** deliver only through returned B; never advance to requested W. Keep pending work and retry with a capped backoff timer or a confirmation-progress notification. **Do not return to hint-only waiting:** the final hint may already have been consumed. Timers exist only while progress is outstanding; no idle polling.
- **Journal `gap_at`:** emit the existing Gap envelope with `gap_at`, then terminate this subscription and require explicit resynchronization. Do not continue delivering a later suffix as if the history were intact. The API explicitly requires resync (`changefeed.rs:203-208`). It can conservatively report a gap beyond the requested bound (`changefeed.rs:510-518`); tolerate that conservative stop.
- **Backpressure:** the current helper silently drops rejected frames below its threshold and returns success (`crates/shamir-server/src/subscriptions/push.rs:95-121`). **That behavior must change.** For the smallest PR, fail closed on the first rejected required frame: retain the cursor, best-effort emit SlowConsumer/Closed, and stop. Do not silently continue. A subsequent readiness-aware bounded retry implementation may retain one serialized frame and change index, but must not rerun reactive Batch/Call work on every retry.
- **Slow subscriber:** journal catch-up remains bounded by pages, not an expanding event buffer. Sink exhaustion terminates; unavailable journal history produces Gap. Never block commits for that subscriber.
- **Idle repo:** await receivers only when clean; completion waits and retry timers are armed only for pending work. No periodic journal reads on healthy idle subscriptions.
- **Closed receiver/reopen/unavailable repo:** explicit subscription termination; no ambiguous empty-read success.

### Initial snapshot is a separate handoff invariant

**OBSERVED:** the bridge runs independent snapshot queries, then seeds its cursor from a later `last_committed` value (`bridge.rs:323-400`; `repo_instance.rs:883-885`).

**INFERRED:** a commit after a table’s snapshot read but before that seed can be excluded from both snapshot and live delivery. Merely replacing that final maximum with W does not establish an atomic snapshot boundary.

**Recommendation:** use a consistent snapshot at an explicit version cut S, then replay journal events strictly after S. Preserve snapshot frames’ version-zero convention and Ready ordering (`bridge.rs:345-388`). If the query layer cannot supply that cut yet, do not claim exact snapshot/live handoff; make that capability a prerequisite for closing N1-live’s `initial=true` guarantee.

## 3. Filtering and authorization

**Keep their semantics unchanged.**

- Source table ACL authorization stays at activation: `crates/shamir-server/src/subscriptions/bridge.rs:141-167`.
- Authorized sources still become per-subscription targets, event masks and filters: `bridge.rs:169-222`.
- For every journal event, retain the live path’s table/op precheck, cached filter evaluation and delivery caches: `bridge.rs:420-548`.
- Preserve OR semantics across overlapping targets, Put filtering, Delete behavior and fail-closed missing Put bytes: `crates/shamir-server/src/subscriptions/target_match.rs:81-110`.
- Reactive Batch/Call remains per subscriber, under the actor and configured limits: `crates/shamir-server/src/subscriptions/push.rs:35-70`. Never share its results across actors.

**Yes, the journal carries the full event.** `RecordChange` contains table, key, op and full Put bytes; `ChangelogEvent` includes repo/version/actor/changes (`crates/shamir-tx/src/changefeed.rs:52-103`). The writer serializes that event rather than just a key/version (`changefeed.rs:857-872`).

## 4. Engine and `shamir-tx` changes

**First correctness PR: retain the existing `Arc<ChangelogEvent>` broadcast API, but the bridge reads only its version as a hint.** This avoids unnecessarily breaking engine/DB/raw changefeed consumers while fixing server subscriptions.

- Keep prepublish journal enqueue unconditional.
- Keep `broadcast_changefeed_event` postpublish and synchronous (`repo_instance.rs:1484-1490`).
- Keep the zero-subscriber broadcast shortcut. It is safe only because registration precedes cursor/catch-up establishment; subscription startup must perform its own drain.
- The legacy journal-only branch must continue journaling regardless of subscriber count (`repo_instance.rs:1545-1555`), until S5b-4 replaces its follower usage.
- Do not add another `Arc`, spawned task, barrier send or await per commit.

**The hint ring does not logically need an Arc; `u64` suffices.** Convert to a version-only ring in a later API-scoped PR after auditing callers. Replace—not supplement—the existing send on the subscription route. Keeping two per-commit fanouts defeats the low-hot-path-cost goal.

Tradeoff:

- Version-only ring retains roughly 8 KiB of version values at capacity 1024, excluding channel overhead, instead of retaining references to potentially large event graphs (`changefeed.rs:138-140`, `:177-179`).
- Journal reads deserialize events into owned values for each reader (`changefeed.rs:493-501`). Existing filter/delivery caches do **not** eliminate that journal-event decoding.
- Cost is approximately proportional to subscribers × journal bytes consumed. A shared bounded read/decode cache or dispatcher is a measured follow-up, not the first correctness mechanism.
- Confirmation establishes completed store writes, **not a per-event fsync guarantee**: only marker writes explicitly flush in the adapter (`crates/shamir-engine/src/repo/changelog_store.rs:28-42`). Do not conflate N1 ordering with S5c crash durability.

## 5. Expected impact and minimum evidence

**INFERRED, unmeasured estimates:**

- Commit overhead: same order as today; no additional per-commit allocation/await/lock. Version-only hints should reduce retained memory, not radically change commit latency.
- Healthy live delivery: adds writer queue/store visibility latency, barrier scheduling and a range read/decode. For small warm-memory events, budget **tens of microseconds to low milliseconds**, not a guaranteed result.
- Under storage load, delivery tracks writer backlog; confirmation can consume its approximately one-second budget.
- Large payloads/high subscriber counts can produce **multiple-fold reader CPU/allocation amplification** versus Arc fanout. Small-event cached fanout may be much closer. CI benchmarks must decide.

Minimum CI evidence:

1. **A parked postpublish/prebroadcast; B commits:** subscriber receives A then B while A’s broadcast is still parked; releasing A adds no duplicate. Cover synchronous and AsyncIndex paths. Reuse `fire_post_publish_pre_broadcast_test_hook` (`repo_instance.rs:1494-1500`) and the existing scheduling fixture (`crates/shamir-engine/src/tx/tests/journal_publish_order_tests.rs:59-125`).
2. **A unresolved before publication; B hints first:** neither B nor a false gap is delivered; A completing **or aborting** releases correct progress. Include empty-footprint settlement with no new payload hint.
3. **Lagged:** overflow the ring deterministically, drain journal, assert exact ordered set and no Gap when history exists.
4. **Timeout/final hint:** gated writer plus paused time; old prefix only, later delivery without another commit, no duplicates.
5. **Gap:** dropped enqueue, failed write, corrupt/read-error paths; Gap then termination, no later ordinary events.
6. **Paging:** multiple pages; short byte-limited pages; filtered-out pages; sparse versions; backlog finishing after the final hint.
7. **Backpressure:** reject a specific frame within a multi-change event; no cursor advancement past it and no silent later delivery. Verify registry cleanup/termination.
8. **Many subscribers:** differing cursors, masks, filters and speeds; one slow subscriber cannot block others.
9. **Activation/snapshot/resume:** registration race, inclusive resume boundary, consistent snapshot cut and Ready semantics.
10. **Idle:** instrument journal calls; after catch-up, no repeated reads/barriers without pending work.

Use barriers/notifies and paused time, not sleeps. The existing server watermark tests merely manipulate integers and do not exercise delivery (`crates/shamir-server/src/subscriptions/tests/bridge_tests.rs:3-51`).

Benchmark in CI: existing `subscription_delivery`, `subscription_fanout`, `subscription_throughput`, plus `tx_pipeline`/`tx_concurrent` and `changelog_read`. Measure subscribers 1/16/128, payload sizes, filtered traffic, warm-memory and persistent storage, p50/p99 latency, writer depth/drops, confirmation timeouts, allocations and retained memory.

## 6. Ordered PR split

1. **Read/progress contract.**  
   Scope: `shamir-tx/src/changefeed.rs`, barrier tests; engine repo journal-read methods; DB changelog wrappers. Land S5b-2 and expose actual B/confirmation outcome internally without changing replication wire format.

2. **Push acceptance semantics.**  
   Scope: server `subscriptions/push.rs`, bridge call sites and backpressure/cleanup tests. Stop silent drops; minimum policy is fail closed. **Can run in parallel with step 1**, with bridge changes coordinated.

3. **Journal-authoritative bridge.**  
   Scope: `subscriptions/bridge.rs`, topic-separated bridge tests, narrow engine fixture extensions. Unified paged resume/live drain, hint coalescing, Lagged recovery, pending completion/timeout handling, unchanged filters/caches. **Depends on 1 and 2.**

4. **Snapshot-cut handoff.**  
   Scope: snapshot/query API needed to expose a consistent cut, bridge initial branch, initial/resume e2e tests. Design/API work can run alongside 1–2; coordinate bridge implementation after 3. **Required before claiming `initial=true` completeness.**

5. **Version-only hint API, if desired.**  
   Scope: tx feed, engine repo API, DB wrappers and all direct consumers/fixtures. **After 3**, and coordinate with S5b-4 to avoid conflicting follower emit changes. Keep this separate from the first fix.

6. **CI performance gate and measured fanout optimization.**  
   Existing benchmark scopes; add only missing ordered-delivery cells. A shared dispatcher/cache is justified only by these results.

**S5b-4 can proceed in parallel**, but must land before claiming this works for subscriptions on followers: current follower W can pass an event before it is queued (`apply_replicated.rs:507-538`).  
**S5c remains separate:** crash recovery/history retention is not repaired by wake-up hints.

## 7. Must not break

- **Wire format:** retain Event/Gap/Ready/SlowConsumer/Closed envelopes, `sub`, `seq`, `data`, `gap_at`; no W metadata on the client wire.
- **Per-record delivery:** do not deduplicate different changes sharing a commit version.
- **Actor isolation, filters and limits:** retain table ACL checks, source OR matching and per-actor reactive execution.
- **Caching:** preserve database/repo/version/change discrimination (`bridge.rs:105-121`, `:436-451`, `:489-529`). Do not base correctness on all subscribers advancing together; fast-subscriber eviction must remain only a performance issue.
- **Cleanup:** every exit/cancellation must retain the RAII registry guard (`bridge.rs:50-58`, `:100-102`).
- **Server compatibility:** retain existing coverage in `crates/shamir-server/tests/subscriptions_e2e.rs`, including basic delivery (`:247`), unsubscribe (`:589`), multi-repo rejection (`:777`) and mixed authorization (`:982`).
- **TS semantics:** the router forwards arrival order without repairing gaps/order (`crates/shamir-client-ts/src/core/subscription-router.ts:53-80`); the handle queues those events and terminates on Closed (`crates/shamir-client-ts/src/core/subscription-handle.ts:40-73`). Gap does not automatically resync.
- **Honest guarantee:** TS early-registration buffering can itself drop frames at 256 (`subscription-router.ts:21-28`, `:72-78`), and the protocol has no delivery acknowledgement. N1-live must not advertise end-to-end exactly-once across disconnects/client loss.
- **Gap-marker limitations:** the feed retains only the minimum known hole (`changefeed.rs:535-545`) and only reports it when it is at/after the read start (`:512-516`). That is not a complete historical hole index. Do not claim arbitrary resume cursors detect every later hole after an earlier one has been skipped.

**Files changed: none. Builds/tests/benchmarks run: none, as requested. Recommendation: proceed with journal-authoritative delivery, gated on the read-bound API, explicit push-failure behavior, and a correct snapshot cut; keep version-only rings and dispatcher optimization separate.**

SNAPSHOT DESIGN (binding for W semantics):

### SNAP-FIX design (reviewer-grade analysis at master f2e32c33; binding for all SNAP leaf tasks)

CONFIRMED BUG (CI probe, PR #108 branch `test/snapshot-stability-probe`, test `crates/shamir-engine/src/tx/tests/snapshot_visibility_park_tests.rs` + a test-only pre-materialize seam in `materialize.rs`): a Snapshot transaction C that begins while an earlier-allocated version A (v10) is parked and a later version B (v11) already published gets snapshot_version = 11; its point read / scan / COUNT of A's table are None / empty / 0; after A materializes the SAME reads return the row / 1 row / COUNT 1.

ROOT CAUSE. `RepoTxGate::register_snapshot` (`crates/shamir-tx/src/repo_tx_gate.rs:~479`, re-read at `:~483`) pins every snapshot at `last_committed`, which is NOT the contiguous completion watermark W (`gate.completion().watermark()` = highest V such that every version k <= V is Materialized or Aborted; `completion_tracker.rs:~59`). `last_committed` is raised eagerly and non-contiguously by `publish_committed_max` (`repo_tx_gate.rs:~721-740`) from: `mvcc_store/mvcc_history.rs:~522` (`apply_committed_visible`, Phase 5a, per MVCC table, before 5b/5c and before `version_guard.commit()` at `materialize.rs:~228` / legacy `commit.rs:~959`), `mvcc_history.rs:~407` (drainer `write_committed_batch_to_history`), `mvcc_history.rs:~633` (`write_committed_to_history`), `tx/recovery.rs:~675-679` (harmless, single-threaded open), `table/table_manager_changefeed.rs:~69`, and the plain store `publish_committed` (`:~706-709`, no production caller). The contiguous raisers are correct: `VersionGuard::commit`/`Drop` -> `advance_last_committed` (`version_guard.rs:~136-151`), `sync_last_committed_from_watermark` (`repo_tx_gate.rs:~748-751`), non-tx writes (`mvcc_store/mod.rs:~820/923/1016/1078`). Versions are allocated at two places: `tx/pre_commit.rs:~2557` (lock-free path) and `:~2734` (legacy path). Any version <= S that has not settled can later appear inside S: point reads `resolve_read` (`mvcc_store/mod.rs:~706-759`, direct path `cur_v > 0 && cur_v <= S` at `:~712`), `get_at_many` (`:~1141-1213`), `snapshot_stream` (`:~1354-1469`, overlay `snapshot_le` `:~1406`), COUNT (a scan since B04). Serializable is worse than unstable: the predicate window (S, last] cannot see a late v10 <= S (`repo_tx_gate.rs:~983-1007`) so write skew can commit.

DECIDED FIX = (a) + (d) + GC floor that includes W:
 - transaction snapshots pin at W (`completion().watermark()`); `register_snapshot` reads W;
 - a commit is ACKNOWLEDGED only once W >= its `commit_version` (wait in the `commit_tx` wrapper `commit.rs:~587-610`, after `commit_tx_inner`, on `Ok` and on `Err(WalDurability{commit_version})`, before `apply_backpressure`; all locks are released by then; cancel-safe); without it "begin after my own commit" and `AsOf(commit_version)` break;
 - GC treats W as a snapshot that is always alive: floor = min(active snapshots U {W}) ALWAYS (not only as a fallback);
 - the eager `last_committed` stays ONLY as the floor for latest-state reads (read-committed, Pessimistic) and DDL pins; the persisted marker stays the eager value (CRIT-B);
 - REJECTED: (b) exclusion set at begin (O(last - W) begin cost, every read primitive changes, GC still needs W); (c) making `last_committed` contiguous now (breaks the predicate window upper bound, Pessimistic lock release, rename `drain_to_history`, DDL backfills) - deferred to a later unification.
 - No deadlock: a waiter waits only for strictly LOWER versions, and their owners never wait on W before settling; allocation always happens after commit_lock/uwl/drain-guard acquisition; the drainer never waits on W. Liveness: no hard bound on how far W lags (slowest unsettled lower version: in-flight materialization, a non-tx writer spanning `history.transact`, orphans until the next drainer pass, Pending tickets until resolved, Unknown/foreign ticket = reopen required and W frozen). Never add a timeout that silently gives up (it would void read-your-own-commit); add a metric and a warning past a threshold; waiters bail out on `requires_reopen()` returning the outcome with `materialization = Deferred`.

MERGE ORDER (each PR independently mergeable):
 PR-1 shamir-tx additive: completion wait primitive (`CompletionTracker` waiters counter + Notify signalled after a successful CAS in `try_advance`; `RepoTxGate::visible_floor()` and `await_visible(v) -> Visible | ReopenRequired`).
 PR-2 shamir-tx: GC floor includes W (`min_alive` `repo_tx_gate.rs:~771-790`; L6 `vacuum_key` deletes the deferred anchor only if old_v <= W else sets `vacuum_needs_scan` `mvcc_gc.rs:~84-123`; keep an anchor when a snapshot is live OR cur_v > min_alive `:~193-201`). Strictly more conservative, safe alone, and fixes a bug that exists TODAY for non-tx writes (v10 parked, non-tx writes of K at v11 and v12, K@5 deleted, a snapshot at <= 10 reads None).
 PR-3a engine: explicit index-build pin (`RepoTxGate::open_snapshot_at(pin)`; pin at `version_allocation_high_water_mark()` read under the barrier after the drain: `table/table_manager.rs:~1581-1586`, `table_manager_index_mgmt.rs:~2709-2714`). Index-build pins must NOT become W.
 PR-3 (after 1, 2, 3a): snapshot = W together with the ack wait; rename production eager publishers to `raise_published`; `publish_committed_max` becomes a `#[doc(hidden)]` test/bench seam that force-settles the tracker through v and raises the published value (keeps ~100 test call sites meaning "everything <= v is committed"); `assign_next_version` becomes `pub(crate)`; rework the probe (spawn B instead of awaiting it inline while A is parked; assert C.snapshot <= W and < A's version, B not finished while A is parked, D sees A and B).
 PR-4: seeds and AsOf at W (`current_commit_version`, subscription bridge seed, `AsOf(v > W)` waits or is rejected). PR-5: non-tx ack wait. PR-6: Materialized must mean fully visible (Phase 5a failure must not reach `version_guard.commit()`; follower partial apply). PR-7: `finalize_reservation` must be max-monotonic (`mvcc_store/mod.rs:~666-681`, like `publish_cell` `:~547-553`).

RED TESTS (named by the design): 1 the probe (reworked); 2 Serializable variant (A parked post-WAL holding commit_lock, B Snapshot commits v11, C Serializable begins, scans empty table a, writes table c: red if C commits with S_C >= A's version); 3 begin after own commit (B's commit must not return before W >= 11; after it returns a new Snapshot sees B's row); 4 aborted lower version does not stall (pre-WAL drop; FAILED ticket via the Mem sink sync-fault knob); 5 orphan below a commit (B returns only after the drainer replays it; drainer suppressed -> B pending until drain_all); 6 Unknown ticket below a commit (wait bails with Deferred, no hang); 7a GC: no snapshot open, v10 parked, non-tx writes of K at v11 and v12, a snapshot at W must read K@5 (RED TODAY); 7b B's Serializable footprint at v11 must survive a GC tick with no snapshot open and T (Serializable at W) must conflict with it; 8 stress (N writers with yields and parks, readers repeat point/scan/COUNT twice inside one snapshot; invariants: snapshot <= W at open, ack implies W >= v); 9 index build (v10 parked on another table, v11 completed on this table; after Ready the index has a posting for v11's row); 10 two Snapshot writers to the same key, the lower one released last: `version_of(K)` must be 12 and the latest read returns v12's value.

TEST HANG WARNING: tests that park one commit and then await another INLINE will hang once the ack wait exists (known: `tx/tests/post_wal_orphan_ordering_tests.rs:~215`; audit `commit_detach_tests`, `post_wal_cancel_tests`, `drainer_tests`, `fk_ri_barrier_tests`, `shutdown_quiesce_tests`). Fix the tests (spawn the second commit), never raise timeouts.

