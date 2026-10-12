# Brief — S5c-4 end-to-end proof of the crash certification for followers, backup/restore and operators

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

- NEW test files under `crates/shamir-server/src/replication/tests/` (for example `recovery_gap_consumer_tests.rs`, + one-line `pub mod` in that `tests/mod.rs`) and ONE new test function (or a small new file) next to `crates/shamir-server/tests/backup_restore_e2e.rs`
- docs: `docs/guide-docs/KNOWN_LIMITATIONS.md` ONLY the replication journal paragraph in section 5 (the one that ends with the unflushed-journal-tail sentence) and the 'Detected gaps stop replication' bullet next to it; `docs/guide-docs/guide/07-operations.md` ONLY a short subsection on what an operator must do after a leader crash; `docs/guide-docs/guide/08-interconnect.md` only if it states the old behaviour
- NO production code changes. If a scenario fails because of a production bug, do NOT paper over it: report it with file:line and the assertion

## Task

S5c-4 (task #1314; umbrella S5c #1296). S5c-1 (#123: the journal gap marker is an interval `[first_missing, affected_through]`, `record_recovery_gap`), S5c-2 (#129: persisted clean horizon J written at CLEAN shutdown; cold start certifies `[J + 1, H]` through `record_recovery_gap` before the repo is served, fails the open on any error) and N1L/S5b (#117, #119, #121, #125, #128, #134 pending) are MERGED on master, your base (check #134 status in git log; if the bridge snapshot-cut text is not on master yet, do not touch subscription wording). Read the S5c-2 code first (`RepoInstance::certify_cold_start`, `shutdown_clean`, `StoreChangelog::load_clean_horizon`, `RepoChangefeed::record_clean_horizon`), the follower loop (`crates/shamir-server/src/replication/follower_loop.rs` ~:300-330 `JournalGap` handling), the supervisor (`supervisor.rs` ~:300-335 persists `resync_required`), `admin_replication.rs` (status / ListSubscriptions) and the existing tests next to them.

The conservative variant was chosen on purpose (the exact detector is a deferred task): EVERY unclean stop of a leader with commits since the last clean start makes followers whose cursor is `<= H` stop with `JournalGap` and `resync_required`, even if the journal happened to be complete; followers cannot reseed automatically. This task proves that behaviour end to end and documents it honestly.

1. Consumer proof (deterministic, in-process, no sockets if the follower loop can be driven against a leader repo directly like the existing replication tests do; otherwise follow their fixture): a leader repo is hard-dropped after commits (no clean shutdown), reopened (certification runs), and a follower pulling from a cursor `<= H` receives the gap, STOPS before applying any accompanying event, its bookmark does not advance, and the supervisor persists `resync_required` visible through the status / ListSubscriptions path; a follower whose cursor is `> H` after new commits keeps replicating normally; a leader that shut down CLEANLY (`shutdown_clean` / the server shutdown path) and reopened produces no gap for a follower at its head; after the follower is recreated from scratch (bookmark 0, the documented procedure) it converges on the leader's data. State exactly which of these the existing fixtures can express; use the same style.
2. Backup/restore: add one test beside `backup_restore_e2e.rs` that backs up a leader data dir taken from a server that was NOT shut down cleanly (or a mid-run backup, whichever the harness supports), restores it, starts the restored server, and checks the restored repo opens (certification succeeds) and a follower cursor inside the covered range gets the gap rather than silently skipping data. A restored directory must never carry a clean horizon J that is newer than its history (a false certificate): state in the report how the backup mechanism copies the changelog store (does it carry J and the gap interval consistently with history? cite file:line), and write the test around the actual guarantee; if there is a hole, report it instead of hiding it.
3. Crash-harness: confirm with file:line whether `crates/shamir-engine/tests/crash_recovery.rs` (extended by S5c-2) already covers unclean-stop-then-reopen; add only what is missing at the server level.
4. Docs, accurate for the code on master: (a) KNOWN_LIMITATIONS section 5: unclean leader stop -> conservative gap `[J + 1, H]`, followers stop with `JournalGap` / `resync_required` even if the journal was complete, no automatic reseed (recreate the follower from scratch), a pre-S5c data directory certifies once on first start, the exact (no-false-positive) detector is not implemented; remove or correct any sentence that still says a crash window is silent; (b) 07-operations.md: a short operator subsection 'After a leader crash': what the status shows, why, and the exact recovery procedure; (c) keep every file:line citation re-verified against the final text.
Constraints: tests + docs only; imports at the top; tests under `tests/`; deterministic (no wall-clock sleeps; paused time or notify handshakes). Do NOT delegate to child agents or wait on schedules; finish the work yourself in this worktree. No git mutation, no builds/tests (CI only).

REVIEWER DESIGN round 1 (S5c; the 'Follower behaviour' and 'Backup/restore' parts are relevant):

## Verdict: **D — recovery GAP repair now, with durable recovery coverage; defer lossless WAL-to-journal reconstruction. Do not implement A using the current accessor.**

**Approve S5c only with these qualifications:**

1. Recovery must detect **unaccounted committed history**, not merely `journal_tail < max(WAL replay)`.
2. Recovery repair must durably complete **before the repo is served**. Calling `mark_gap()` alone does not suffice.
3. The recovery horizon must survive WAL truncation and cover follower-local commits without WAL.
4. The existing **minimum-only GAP marker cannot represent repeated, disjoint crash gaps safely** for every follower cursor.
5. Followers currently **stop and mark `resync_required`; they do not automatically reseed**.

**Evidence status:** observations below are from the current source. Crash outcomes and the recommended protocol are design deductions, not experimentally verified results. No builds or tests were run.

---

## 1. Why this verdict

### A is not correct with `last_persisted_version()`

**OBSERVED:** this accessor is a maximum of successful event puts:

- Initialized to zero on every feed construction: `crates/shamir-tx/src/changefeed.rs:252`.
- Exposed by an atomic load: `crates/shamir-tx/src/changefeed.rs:418`.
- Advanced with `fetch_max`: `crates/shamir-tx/src/changefeed.rs:881`.

It is **not a contiguous, durable journal prefix**:

- Failed event puts mark a gap but allow later events to proceed: `crates/shamir-tx/src/changefeed.rs:872`.
- Concurrent committers can enqueue versions out of numerical order.
- Empty/index-only/counter-only footprints deliberately emit no event: `crates/shamir-tx/src/changefeed.rs:583`.
- Most importantly, ordinary event `put()` performs `Store::set()` **without an explicit flush**; only GAP-marker puts flush: `crates/shamir-engine/src/repo/changelog_store.rs:28`.

**RECOMMENDATION:** reject `min(existing_truncation_ceiling, last_persisted_version())`. It could truncate across a hole, and its durability claim exceeds the adapter’s actual boundary.

A proper A requires a **flushed, completion-prefix certificate**, including eventless versions and durable GAP coverage—not an event maximum. That would preserve WAL while the writer stalls, but **4096 channel slots bound memory, not retained WAL bytes** (`crates/shamir-tx/src/changefeed.rs:144`). Retention would require an explicit disk quota/admission policy or a durable-GAP fallback. Do not introduce that coupling merely to obtain fail-loud recovery.

### B can reconstruct mutations, but not the original event contract

**OBSERVED:** WAL carries transaction ID, repo token, start time, commit version, operations and interner delta:

`crates/shamir-wal/src/wal_entry_v2.rs:142`.

Put/Delete operations contain the record mutation needed for data replay; table resolution is already used in recovery:

`crates/shamir-engine/src/tx/recovery.rs:58`, `crates/shamir-engine/src/tx/recovery.rs:87`.

However, original projection additionally carries:

- Actor.
- Projection-time timestamp.
- Public `tx_id == 0` for implicit transactions.
- Human-readable table names.

See `crates/shamir-tx/src/changefeed.rs:597` and `crates/shamir-tx/src/changefeed.rs:631`.

**INFERRED:** WAL supports a mutation reconstruction, but **not faithful reconstruction of the current `ChangelogEvent`**. Actor and implicit-transaction identity are absent; `started_at_ns` is not projection time; table rename/drop complicates historical naming.

**RECOMMENDATION:** defer B until WAL explicitly carries the projection or sufficient versioned projection metadata. That later work also needs correct retention coverage. It does not solve already-truncated records or the WAL-less follower path by itself.

### C is the right safety policy, but a literal tail comparison is insufficient

**OBSERVED:** recovery replays and finalizes WAL entries without consulting the journal:

`crates/shamir-engine/src/tx/recovery.rs:334`, `crates/shamir-engine/src/tx/recovery.rs:386`, `crates/shamir-engine/src/tx/recovery.rs:413`.

**INFERRED counterexample:** version 12 can be persisted before version 11 is enqueued. A crash leaves tail 12 and missing 11. Comparing tail against recovered maximum 12 detects nothing.

Also, deleted WAL segments are absent from the recovery input. The existing drainer truncation ceiling depends only on history durability and A5:

`crates/shamir-engine/src/tx/drainer.rs:1350`–`1359`.

### Minimal correct protocol now

Use **conservative recovery GAP certification**, not lossless reconstruction:

- Maintain a **durably recoverable committed horizon** before WAL segments containing those commits disappear.
- At startup, reconcile retained WAL data-bearing versions and durable history/recovery coverage against journal coverage.
- Treat unprovable coverage as a GAP; false-positive reseeding is acceptable.
- Persist the recovery GAP and its affected upper horizon before exposing the repo.
- Leave journal enqueue and background persistence asynchronous.

For the smallest initial implementation, a cold-start scan of durable history versions can supply missing coverage, including WAL-less follower writes. History retains version information even though it does not preserve the event projection; version-key layout is documented at `crates/shamir-tx/src/mvcc_store/mod.rs:60`.

For scalable operation, make the truncation boundary persist a conservative recovery horizon before unlink. It may conservatively include eventless versions; it must never understate what is being reclaimed. This is **recovery metadata**, not a journal-writer gate.

**No additional commit-path fsync, await or lock.** Cold recovery may await durable repair. Any truncation-side metadata flush belongs at the existing segment-reclamation boundary—not per commit.

---

## 2. Crash timeline

Journal persistence and publish are not strictly ordered: enqueue precedes publish, but the background writer can persist before or after publish.

The local normal publish pair is synchronous:

`crates/shamir-engine/src/tx/publish_journaled.rs:21`–`28`.

| Crash point | Recovery-visible state | Required S5c action |
|---|---|---|
| WAL accepted, before event preparation/enqueue | Surviving WAL entry; no journal event | Replay data and durably record GAP coverage for that recovered version. |
| After projection/preparation, before enqueue | Projection existed only in RAM | Same. Preparation is not durable coverage. Projection occurs before Phase 5a at `crates/shamir-engine/src/tx/commit.rs:919`. |
| During/after failed enqueue, before GAP persistence | WAL exists; event absent; RAM GAP may disappear | Recover the omission independently; persist recovery GAP. Full/Closed currently mark only RAM first: `crates/shamir-tx/src/changefeed.rs:338`. |
| Enqueue succeeded, before publish | WAL exists; queued event may or may not have landed | Durable WAL recovery treats the version as committed. Keep an existing valid journal event; GAP any unaccounted coverage. |
| Publish completed, before event put | WAL exists; event only queued | Replay and GAP missing coverage. |
| Event `set()` completed, before backend durability | Accessor may have advanced, but event survival depends on backend/crash | Re-read actual startup storage; never trust the lost RAM accessor or an unflushed certificate. |
| Journal event durably persisted, before publish | WAL and event survive | Replay; no GAP needed for that accounted version. Enqueue-before-publish does not make it an aborted event: WAL recovery defines durable entries as committed. |
| Publish completed, before drainer replay | WAL plus possibly journal | Replay data; certify event or GAP coverage. |
| During drainer replay/history write | WAL remains; history may be partial | Existing idempotent replay completes data; GAP repair is independently repeatable. |
| After history replay/`mark_durable`, before history flush | WAL remains; history durability not yet sufficient for unlink | Replay as necessary; certify journal coverage. |
| After history flush, before WAL deletion | History survives and WAL still exists | Recovery has both sources; certify coverage before serving. |
| Between sealed-segment deletions | History survives; WAL contains only survivors | Use durable reclamation horizon/history coverage, not just surviving WAL maximum. |
| After WAL truncation, before journal durability | History survives; original WAL mutations may be gone | Persist GAP using recoverable committed coverage. This is the essential case a replay-only comparison misses. |
| After both history and journal durability | Data and event survive | No repair for that version; still inspect other holes and earlier coverage. |

The existing truncation sequence is history flush then unlink:

`crates/shamir-engine/src/tx/drainer.rs:1382`–`1388`.

### Ticketed WAL and fail-stop cases

**OBSERVED:**

- `WrittenNotDurable` allows the commit pipeline to continue and later report `WalDurability`: `crates/shamir-engine/src/tx/commit.rs:440`, `crates/shamir-engine/src/tx/commit.rs:1023`.
- `NotWritten` is an error without accepted commit.
- `Unknown`/`Indeterminate` require reopening: `crates/shamir-engine/src/tx/commit.rs:441`–`448`.
- Dropped guards transfer Written/Acked tickets into orphan replay; Pending/Unknown remain unresolved: `crates/shamir-tx/src/version_guard.rs:158`–`190`.

**RECOMMENDATION:** classify cold recovery using **what survives as valid WAL/history**, not the original API’s success/error label. A published `WalDurability` error is not an abort. Conversely, do not invent committed data from a ticket known not to have written.

Warm orphan replay already marks a GAP before completing the orphan:

`crates/shamir-engine/src/tx/drainer.rs:1210`.

That mark is asynchronous. A crash before its flush—or after truncation—still requires S5c’s independent cold recovery coverage.

### Follower apply

**OBSERVED:** follower apply writes history directly without local WAL:

`crates/shamir-engine/src/tx/apply_replicated.rs:70`, `crates/shamir-engine/src/tx/apply_replicated.rs:454`.

Current order is publish, durable mark, best-effort marker persistence, then downstream emission:

`crates/shamir-engine/src/tx/apply_replicated.rs:507`–`538`.

**RECOMMENDATION:** S5b-4 should move downstream enqueue-or-gap immediately before publish. But that **does not close the crash-persistence window**. S5c startup coverage must include surviving follower-local history, even with an empty WAL. A lagging/best-effort commit marker cannot be its sole evidence.

Partial follower apply before successful completion is a separate lifecycle problem: conservatively invalidate downstream coverage if history survived; do not pretend WAL reconstruction can recover the original upstream event.

---

## 3. Startup progress and repair idempotence

### What exists today

**OBSERVED:** there is **no persisted journal-prefix checkpoint loaded at startup**. Feed initialization restores the GAP marker, then constructs an accessor starting at zero:

`crates/shamir-engine/src/repo/repo_instance.rs:1376`–`1421`;  
`crates/shamir-tx/src/changefeed.rs:252`.

The journal’s physical tail must therefore be read from `__changelog__`; the existing trait exposes ascending ranges, not a persisted-prefix certificate:

`crates/shamir-tx/src/changefeed.rs:154`.

The gate floor uses `max(last_committed_marker, max_surviving_WAL_version)`:

`crates/shamir-engine/src/repo/repo_instance.rs:850`–`869`.

That is useful coverage evidence, but not sufficient alone after truncation or marker-lagging follower apply.

### Required startup transaction

1. Read marker/coverage metadata with errors propagated.
2. Recover valid WAL data and interner state.
3. Determine the conservative affected version interval from surviving WAL, durable horizon and, where needed, history.
4. Durably write GAP coverage.
5. Only then allow serving.

Existing recovery already propagates replay/interner persistence errors before finalization:

`crates/shamir-engine/src/tx/recovery.rs:388`, `crates/shamir-engine/src/tx/recovery.rs:398`.

Use that same fail-closed contract for GAP repair.

**Idempotence:**

- Crash before repair flush: next startup derives the same or wider conservative interval and retries.
- Crash after repair flush: startup restores the interval; repeating the write is harmless.
- Write/flush failure: do not serve the repo.
- Repeated recovery must merge coverage monotonically, never shrink an earlier invalidated interval.

Do not seed `new_with_gap()` with an unflushed value: it initializes both the in-memory and “persisted” minimum from its seed:

`crates/shamir-tx/src/changefeed.rs:255`–`257`.

The current initialization code correctly persists a pending minimum before seeding:

`crates/shamir-engine/src/repo/repo_instance.rs:1409`–`1419`.

---

## 4. S5a marker and follower behaviour

### Blocking issue: minimum-only metadata

**OBSERVED:** `mark_gap()` retains the minimum nonzero version:

`crates/shamir-tx/src/changefeed.rs:536`.

Reads report it only when `gap >= from_version`:

`crates/shamir-tx/src/changefeed.rs:512`–`516`.

**INFERRED counterexample:** persisted GAP 10, follower reseeds past 10, later crash loses version 100. Marking 100 leaves minimum 10. A pull from 100 receives **no GAP**.

**RECOMMENDATION:** retain S5a compatibility, but add recovery invalidation coverage—at minimum a conservative interval `[first_missing, affected_through]`. For requests within/before its affected horizon, return a `gap_at` that satisfies the existing consumer condition, for example `max(from_version, first_missing)`. After a snapshot through that horizon, later pulls can proceed. Merge later recovery intervals conservatively; exact disjoint intervals are unnecessary initially.

Merely calling `mark_gap(first_missing)` repeatedly is not a complete S5c design.

### Followers do not actually reseed

**OBSERVED:** the follower returns terminal `JournalGap` **before applying accompanying events**:

`crates/shamir-server/src/replication/follower_loop.rs:311`–`325`.

The supervisor persists `resync_required`:

`crates/shamir-server/src/replication/supervisor.rs:309`–`334`.

The source explicitly says automated snapshot reseed remains R2:

`crates/shamir-server/src/replication/follower_loop.rs:320`.

**Conclusion:** recovery-written GAPs can trigger **safe stop/resync-required**, not automatic reseeding. That is sufficient for “never silent,” but the operational cost is interrupted replication until reseed is performed.

---

## 5. Required CI tests and bench evidence

### Deterministic tests

Use controlled store/channel boundaries, not sleeps:

- WAL accepted, no enqueue; recovery writes durable GAP.
- Enqueued event held before `put`; crash/reopen.
- Event set but unflushed; simulate loss at the backend boundary.
- Persist higher version before lower version; prove tail equality does not conceal the missing event.
- Successfully persisted event: no false repair for that version.
- Eventless, aborted and legacy version-zero entries: no fabricated event obligations.
- Drained-and-truncated version with missing journal; include empty surviving replay range.
- Crash before/after recovery marker flush; reopen twice.
- Repair write/flush failure refuses serving.
- Existing GAP 10 followed by new loss 100; cursor 100 must receive GAP.
- Snapshot/bookmark beyond repaired horizon allows progress.
- Written/Acked orphan replay, WrittenNotDurable committed-with-error, Pending, Unknown, Failed.
- WAL-less follower apply and three-node downstream chain.
- Follower receives repaired GAP plus events: applies none, does not advance bookmark, persists `resync_required`.

Existing seams:

- `PostWalPrePublishHook`: `crates/shamir-engine/src/tx/commit.rs:143`.
- Per-repo journal hooks: `crates/shamir-engine/src/tx/tests/journal_publish_order_tests.rs:20`.
- Crash labels including `drain_replay`, `pre_truncate`, `post_truncate`, `wal_mid_delete`: `crates/shamir-engine/src/tx/commit.rs:273`.
- Crash harness aborts without unwind and flushes only transaction metadata at its general seam: `crates/shamir-engine/src/tx/commit.rs:295`–`305`.

Extend those with explicit **journal durability and recovery-repair boundaries**. Existing hooks do not by themselves establish the needed journal state.

### Owed bench evidence

Benchmark only drainer/reclamation changes; do not add commit-path timing machinery.

Extend the existing fixed-iteration depth harness:

`crates/shamir-engine/benches/drain_cost_vs_depth.rs:3`, `crates/shamir-engine/benches/drain_cost_vs_depth.rs:25`.

Required cells:

- No reclaimable segment.
- Segment-boundary reclamation.
- Many reclaimed segments.
- Normal vs stalled/failed journal writer.

Report drain time per entry, reclamation time per segment, added flush count and retained WAL bytes. Confirm no per-pass full journal/history scan and no per-entry metadata fsync. Execute exclusively in CI, with isolated benchmark target artifacts.

---

## 6. PR-sized implementation order

| PR | Scope | Dependency |
|---|---|---|
| **1. Durable recovery invalidation metadata** | `changefeed.rs`, `repo/changelog_store.rs`, feed initialization, existing changefeed/GAP tests. Add affected-horizon semantics and durable repair API; preserve old-marker decoding. | First. |
| **2. Cold recovery reconciliation** | `tx/recovery.rs`, narrowly scoped history-version inspection, recovery/GAP tests. Include empty WAL and follower history. Fail closed on repair persistence errors. | PR 1. |
| **3. Scalable pre-unlink recovery horizon** | `tx/drainer.rs`, recovery metadata helpers, truncation tests, depth bench. Persist coverage at reclamation boundaries; retain history-scan fallback for legacy state. | PR 1; coordinate format with PR 2. |
| **4. End-to-end consumer proof** | Replication follower/supervisor tests, crash harness, backup/restore e2e. Demonstrate stop/resync-required and post-reseed progress. | PRs 1–3. |

S5b-4 follower enqueue-before-publish can proceed in parallel with PR 1, provided it shares the same enqueue-or-gap contract. Crash-fixture and benchmark preparation can also proceed in parallel after metadata semantics are agreed.

Later: versioned WAL projection plus lossless recovery and a genuine durable journal-prefix retention gate.

---

## 7. Must-not-break constraints

- **History/A5 safety:** preserve the current A5 ceiling and history-flush-before-unlink order.
- **WAL semantics:** surviving accepted records remain committed even when the original caller saw an error; unresolved tickets remain fail-stop.
- **Shutdown:** retain quiesce-before-drain and existing bounded shutdown budgets. Current orchestration is at `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:656`; draining is at `crates/shamir-engine/src/repo/repo_instance.rs:1775`. Do not turn quiescence into an unlimited journal wait.
- **Retention:** C-based fail-loud repair must not make WAL retention depend indefinitely on journal liveness. Any later A needs explicit byte limits and admission/fallback semantics.
- **Backup/restore:** preserve journal invalidation metadata and recovery horizons together with history. Test fresh restore, replacement restore and reopened replication using `crates/shamir-server/tests/backup_restore_e2e.rs:614`.
- **Startup scale:** history scanning is a conservative compatibility fallback, not a new hot-path operation.
- **Marker ownership:** repair before spawning the writer, or serialize through its acknowledged durable repair protocol; never race independent marker writers.
- **No stronger durability promises:** ordinary journal event set success currently does not prove flush completion.
- **No automatic-reseed claim:** current implementation guarantees stop/resync-required only.

**Files changed:** none. **Builds/tests/benches run:** none, as requested. **Recommendation:** land durable, horizon-aware recovery GAP handling now; reject a tail-only comparison or accessor-based truncation gate as incomplete.
