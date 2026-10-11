# Brief — N1L-1 bounded journal read exposes the confirmed bound and a progress wait

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

- `crates/shamir-engine/src/repo/repo_instance.rs`: ONLY the journal READ part (`read_changelog_from` and `JOURNAL_CONFIRM_BUDGET` at ~:1520-1560) plus one new sibling method; another agent edits the `changefeed()` init closure / gap marker region (S5c-1) - do not touch it
- NEW `crates/shamir-engine/src/repo/bounded_journal_read.rs` (one primary export: the result struct) + one-line module declaration/re-export in `crates/shamir-engine/src/repo/mod.rs` and, if the type must be public API, the crate root re-export where `JournalRead` is re-exported
- `crates/shamir-db/src/shamir_db/shamir_db/changelog.rs`: new thin wrappers next to `read_changelog_from_journal`
- tests: NEW `crates/shamir-engine/src/repo/tests/bounded_journal_read_tests.rs` (or the existing repo tests dir; follow its manifest) - deterministic, no sleeps
- NO shamir-tx edits (`JournalRead` keeps its shape; the new struct wraps it), NO shamir-server edits (the bridge is a later step), NO commit-path edits

## Task

N1L-1 (task #1308; umbrella #1295 N1-live). Read the reviewer design below FIRST: section 2 ('Required internal API', 'Hint ahead of W', 'Confirmation timeout') and PR split step 1. S5b-2 (bounded engine journal reads) and S5b-3 are MERGED (your base): `RepoInstance::read_changelog_from` takes `w = tx_gate().completion().watermark()`, calls `feed.confirm_through(w, JOURNAL_CONFIRM_BUDGET)` and `read_from_bounded(.., bound)`. Sibling agents: S5c-1 (shamir-tx gap-marker + feed init closure), N1L-2 (server push.rs), S5b-4 (apply_replicated). Stay inside your scope.

PROBLEM: the bridge (a later step) must know the ACTUAL bound B the read was limited to and whether the barrier confirmed the requested W. `JournalRead` carries only `events` and `gap_at`; `read_changelog_from` swallows B; `ShamirDb::read_changelog_from_journal` maps every error and every missing db/repo to `None`, so a caller cannot tell 'repo unavailable' from 'read error'. Without B the bridge cannot advance its processed-through cursor past aborted/empty versions, and cannot tell a confirmation timeout from a short page.

THIS STEP (read/progress contract; no behaviour change for existing callers):
1. New struct, e.g. `BoundedJournalRead { read: JournalRead, sampled_w: u64, bound: u64, confirmed: bool }` (`confirmed = bound >= sampled_w`; `bound <= sampled_w` always). Document the contract: events are exactly the journal prefix `<= bound`; an EMPTY page proves exhaustion through `bound` only when `confirmed`; a short page is NOT exhaustion (the store adapter may return early on its byte budget, `changelog_store.rs` ~:63-79); `gap_at` semantics unchanged.
2. `RepoInstance::read_changelog_bounded(from_version, limit) -> DbResult<BoundedJournalRead>` holding the sampling logic (W sampled FIRST, then confirm, then read bounded by the returned bound); `read_changelog_from` becomes a thin wrapper returning `.read` so its behaviour and all existing tests stay identical. Keep the single `JOURNAL_CONFIRM_BUDGET` constant and the single warning policy (confirmation logs/counts its own timeout; do not add a second log).
3. A progress-wait accessor for the bridge, e.g. `RepoInstance::await_journal_progress(target_version) -> DbResult<..>` built on `gate.await_visible` / `visible_floor` (SNAP-1, merged: read `repo_tx_gate.rs` for `VisibleOutcome` and its cancellation-safety/reopen semantics) that resolves when the completion watermark has reached `target_version` (or reports reopen/closed), so the bridge can park off the hot path when a hint is ahead of W. Do not add polling; it must be cancellation-safe and documented as such.
4. `ShamirDb` wrappers in `changelog.rs`: `read_changelog_bounded(db, repo, from_version, limit) -> Result<Option<BoundedJournalRead>, DbError>` where `Ok(None)` = db/repo does not exist and `Err` = a real read/init failure (error-preserving, unlike `read_changelog_from_journal`), and the matching `await_changelog_progress`. Existing wrappers keep their behaviour.
5. Tests (deterministic): bound == W when the writer has persisted everything (`confirmed == true`, empty page after the last event proves exhaustion); parked committer A below B: bound < latest version, events stop below A, `confirmed` stays true while W < latest (W is the bound, not the latest); confirmation timeout (use the existing feed test seams from `shamir-tx` changefeed tests / `journal_order_tests.rs` to gate the writer; paused time if the budget is awaited via tokio time): `confirmed == false`, `bound < sampled_w`, the prefix is old but correct, no panic; `await_journal_progress` resolves after the parked version settles (complete AND abort), and reports reopen; the engine-level and DB-level wrappers return `Ok(None)` for a missing repo and `Err` for an injected read failure (find an existing failing-store fixture; if none, test the engine wrapper only and say so); `read_changelog_from` output equals `read_changelog_bounded().read`.
Constraints: no new Mutex/RwLock, no per-commit cost (read side only), imports at the top, one primary export per file, tests under `tests/`. Do NOT run benches.

REVIEWER DESIGN (N1-live):

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
