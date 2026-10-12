# Brief — S5c-2 unclean-shutdown certification: clean horizon J and fail-closed cold-start gap

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

- `crates/shamir-tx/src/changefeed.rs`: a writer-owned durable clean-horizon record next to the S5c-1 gap interval: the key, `record_clean_horizon(through)` (acknowledged through the sole writer like `record_recovery_gap`), its loader/decoder, and excluding the new metadata key from journal iteration; + tests under `crates/shamir-tx/src/changefeed/tests/` (new `clean_horizon_tests.rs` + one-line `pub mod`)
- `crates/shamir-engine/src/repo/changelog_store.rs`: persistence/loading of the clean horizon, and the iteration filter that must skip the new metadata key (today only the gap key is skipped, ~:70-73)
- `crates/shamir-engine/src/repo/repo_instance.rs`: the `changefeed()` init (load J), the clean-shutdown write of J, and the cold-start certification entry point; another agent edits the journal read region (N1L-1 is merged by then) - stay out of `read_changelog_*`
- the repo open/recovery path: `crates/shamir-engine/src/tx/recovery.rs` (call site only) and the facade open paths `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs` (~:378-386 `add_repo_as`) and `crates/shamir-engine/src/db_instance/db_instance.rs` (`add_repo`, ~:52-57) ONLY as far as needed so that certification completes BEFORE the repo can be reached by any caller (reorder the registration, or add an explicit readiness gate; cover the system-store and config-driven open paths - grep every caller of the repo constructor/open + recovery)
- the clean-shutdown hook: wherever quiesce_commits + drain + journal writer drain happen at shutdown (`db_management.rs` `flush_all_with_quiesce_timeout` / `RepoInstance` drain ~:1775): write J there, after everything is settled
- tests in the existing engine/db test dirs (new files + one-line `pub mod`), deterministic, hard-reopen based
- NO WAL format change, NO obligation ledger, NO history scan, NO drainer changes, NO follower/server changes, NO docs (S5c-4 owns the wording)

## Task

S5c-2 (task #1312; umbrella #1296). S5c-1 (#123: the gap marker is an interval `[first_missing, affected_through]` and `RepoChangefeed::record_recovery_gap(first, through)` durably certifies it through the sole marker writer, acknowledged after put + flush) is MERGED on master, your base. Read it first (`changefeed.rs`, `repo_instance.rs` `changefeed()` init, `changelog_store.rs`).

DECISION (binding). The reviewer's second round (appended below) established that an EXACT crash-gap detector needs a per-version obligation ledger, a WAL requirement bit and follower provenance (several PRs, a WAL format change) and that a cheap exact algorithm does not exist. The program decision is to ship the CONSERVATIVE FAIL-CLOSED variant now (this task) and keep the exact path as a deferred, separately approved task (#1313). So this task implements 'unclean-shutdown certification':
1. Persisted clean horizon J: a versioned, validated record in the changelog store (its own metadata key, excluded from journal iteration everywhere the journal is iterated - verify EVERY iteration site, not only the gap key's), owned by the sole journal writer (same acknowledged-through-the-writer protocol as `record_recovery_gap`: write, flush; never two independent writers). Semantics: 'every version <= J was fully accounted when the process stopped cleanly'.
2. Clean shutdown writes J: after admission is stopped and commit/apply work is quiescent (`quiesce_commits`), the drainer has drained, and the journal writer has drained its queue and flushed - set J = the allocation high-water mark / completion watermark at that point (all versions settled; state which and why they are equal after quiesce), flush, and only then report shutdown done. If any step fails or times out, DO NOT write J (the next start treats the stop as unclean).
3. Cold start (every repo open, after WAL recovery and gate-floor seeding; H = the recovered gate floor = the highest version a commit could have used): force the changefeed to be created; load J (absent = 0); if H > J then await `record_recovery_gap(J + 1, H)` (acknowledged = durable) and then durably advance J to H through the writer; if H <= J nothing is certified. A fresh repo (H == 0) certifies nothing. A pre-S5c data dir (no J, H > 0) is certified once. Idempotent: a crash between the gap flush and the J write recomputes the same or a wider interval on the next start; a second clean restart afterwards certifies nothing.
4. Fail closed: any failure of loading J, certifying the gap or advancing J makes the repo OPEN FAIL with an error (never a RAM-only gap, never serve). Certification must be COMPLETE before the repo can be reached by any caller: the reviewer found that the facade registers the repo (`db_management.rs` ~:378-386 -> `DbInstance::add_repo`, `db_instance.rs` ~:52-57) BEFORE recovery runs, so an Err from recovery does not prove the instance is unreachable. Make recovery + certification happen before registration (preferred) or enforce an explicit readiness gate on every access path; cover the constructor/config opening path and the system-store opening path too (grep every place that builds a `RepoInstance` and runs recovery). State what you chose and list the paths audited.
5. The gate floor must be seeded >= H BEFORE any allocation (the reviewer notes it uses only marker + surviving WAL and swallows errors, `repo_instance.rs` ~:856-875). Do not widen the scope into redesigning that; if H is not already guaranteed to be the true maximum committed version, record it as a limitation in your report with file:line instead of guessing.
Known cost (state it in code docs and the report): every unclean stop with commits since the last clean start certifies `[J+1, H]`, and followers whose cursor is `<= H` then stop with `JournalGap`/`resync_required` even if the journal happened to be complete (followers cannot reseed automatically); a pre-S5c leader certifies once on upgrade.
Tests (deterministic, no sleeps; hard reopen = drop the instance without the clean shutdown path, then open again on the same directory/store): clean shutdown + reopen -> no gap and J == H; hard drop after commits -> reopen certifies `[J+1, H]`, a pull from any cursor `<= H` reports `gap_at = max(from, J+1)`, a pull from `H + 1` after new commits reports none; legacy dir without J -> certified once, then a clean cycle certifies nothing; a crash between the gap flush and the J write (injected failure of the J write) -> the next open re-certifies idempotently; failure of the gap put/flush or of the J write at open -> the open returns Err and no registry entry exists / the repo is unreachable; clean-shutdown step failure -> J not written -> next start certifies; the new metadata key never appears in journal iteration/pulls; existing journal/gap tests unchanged and green; an old repo with an existing S5c-1 interval keeps it (merge, never shrink).
Constraints: no per-commit cost (certification is cold-start + shutdown only), no new Mutex/RwLock, one primary export per file, imports at the top, tests under `tests/`. Do NOT run benches. Do NOT delegate to child agents or wait on schedules; finish the work yourself in this worktree.

REVIEWER DESIGN round 1 (S5c):

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

REVIEWER DESIGN round 2 (S5c-2 exact algorithm; context only - the exact path is deferred, see DECISION):

## Verdict — **Do not implement S5c-2 as “persist J, then compare history versions with journal versions.” That algorithm is neither exact nor bounded.**

**Approve a revised design: durable event-obligation evidence plus a flushed, accounted prefix J, followed by cold reconciliation.** Keep conservative GAP certification for legacy or incomplete evidence, but explicitly accept its one-time false-positive cost.

**A cheap, fully correct algorithm does not exist using the current persisted information alone.** In particular, it cannot guarantee both:

1. Never miss a lost required event after WAL truncation/history GC.
2. Certify no GAP whenever all genuinely required events were persisted.

J reduces an uncertainty window; **it does not identify obligations inside that window**. A scalar reclamation horizon has the same limitation.

My earlier design was incomplete on this point. Its history-scan proposal must be corrected before implementation.

**Evidence convention:** **OBSERVED** below means inspected source, not experimentally established crash behavior. **INFERRED** identifies consequences and proposed protocol guarantees. All unqualified references are to master `c07b191d`; **[S5c-1]** references are to `origin/feat/recovery-gap-horizon`. The brief is absent from master here; I read its appended design from that branch.

---

## 1. Precisely what requires an event

### The semantic predicate

**RECOMMENDATION:**

For a positive committed/recovered version v:

```text
REQUIRES(v) =
    local transaction: project_event(staging_snapshot_at_projection, v) != None
    successful follower apply: downstream event was required at local version v
```

This is **not** “v was allocated,” “v appears in a timestamp namespace,” or “v is below the committed watermark.”

**OBSERVED:** `project_event` visits every Set/Remove in `tx.write_set`, creates record changes without filtering by table name, actor, or whether a Delete actually removed an existing record, and returns None only when the resulting changes are empty:

- `crates/shamir-tx/src/changefeed.rs:594`
- `crates/shamir-tx/src/changefeed.rs:602`
- `crates/shamir-tx/src/changefeed.rs:622`

### Exact classification

| Input | Event obligation |
|---|---|
| Any staged data Set | Required, including overwrites/no-op-looking replacements. |
| Any staged data Remove | Required, including removal of a nonexistent record. |
| Empty data staging | Not required. |
| Index-only/counter-only/interner-only footprint | Not required **if data staging is known empty**. |
| Implicit transaction with data staging | Required. `implicit` changes public `tx_id` to zero, not the predicate. |
| System-table data mutation through the transaction pipeline | Required. There is no system-table exemption in projection. |
| DDL metadata/index operation outside data staging | Does not acquire an obligation merely because it consumes a version. |
| Allocated then aborted/dropped transaction, without surviving accepted WAL/data | No committed event obligation. |
| Positive surviving accepted WAL entry | Recovery treats it as committed; classify its original data footprint. |
| Legacy version-zero entry | No identifiable positive-version obligation can be fabricated from v=0. Its legacy recovery is separate from `(L,H]`. |

**OBSERVED:** implicit handling is at `crates/shamir-tx/src/changefeed.rs:634`. System-store Set actually routes through implicit transactions at `crates/shamir-db/src/shamir_db/system_store.rs:191`–207. Consequently, the premise that **all system writes are eventless is false**.

**OBSERVED:** recovery treats positive surviving WAL entries as Materialized and explicitly has no commit/abort distinction in WAL: `crates/shamir-engine/src/tx/recovery.rs:413`–430.

### Is it decidable from WAL alone?

**Not universally with the current format.**

A surviving positive WAL entry containing Put/Delete is affirmative obligation evidence. But absence of Put/Delete is **not universally negative evidence**:

**OBSERVED:** WAL construction copies staged data operations **only if their keys parse as RecordId**:

- `crates/shamir-engine/src/tx/commit.rs:528`–549.

Projection has no corresponding key restriction.

**INFERRED:** a transaction containing staged non-RecordId operations can project an event while its WAL contains no Put/Delete. Therefore:

```text
has Put/Delete => REQUIRED
no Put/Delete  => UNKNOWN, unless a proven producer contract excludes omitted data ops
```

The minimal format correction is a versioned `requires_journal_event` field computed from the same staging snapshot as projection. A presence bit is necessary: **old records without the field mean UNKNOWN, not false**.

Actor, timestamp, and implicit/public transaction identity are unnecessary for detecting the obligation, although necessary for faithful reconstruction. Current WAL fields are at `crates/shamir-wal/src/wal_entry_v2.rs:142`–159.

### Is it decidable from history alone?

**No.**

**OBSERVED:**

- History encodes record key plus version, not transaction origin or event requirement: `crates/shamir-tx/src/mvcc_store/mod.rs:3`–6.
- Timestamp keys are a different namespace: `crates/shamir-tx/src/mvcc_store/mod.rs:55`–63.
- Direct MvccStore writes allocate versions and write history without themselves calling projection: `crates/shamir-tx/src/mvcc_store/mod.rs:985`–1017.
- Follower failure can leave earlier table writes while the guard marks the version aborted: `crates/shamir-engine/src/tx/apply_replicated.rs:458`–488.

Thus a surviving history row supplies a version and mutation, but not a universally valid answer about successful publication or projection. Missing rows supply **no negative evidence**.

---

## 2. Recovery bounds and durable metadata

### Lower bound L

**RECOMMENDATION:** `L = durable JournalAccountedThrough`, abbreviated J.

Define J as:

> For every required event at a version ≤ J, either its journal event has crossed the explicit durability boundary, or durable GAP metadata invalidates that version.

“Accounted” is deliberate: J is not necessarily a lossless journal prefix.

**OBSERVED:** master has no such certificate:

- `last_persisted_version` starts at zero: `crates/shamir-tx/src/changefeed.rs:252`.
- It is a maximum of successful puts, not a prefix.
- Event puts do not flush; marker puts do: `crates/shamir-engine/src/repo/changelog_store.rs:28`–41.
- The existing barrier confirms processing, not backend flush durability: `crates/shamir-tx/src/changefeed.rs:370`–393.

### Minimal scalar metadata

Introduce a non-event key in `__changelog__`, for example:

```text
key:   b"\0journal-accounted-v1"
value: versioned, validated encoding of u64 J
```

Exclude **all** metadata keys from journal iteration; today the adapter excludes only the GAP key at `crates/shamir-engine/src/repo/changelog_store.rs:70`–73.

**Sole journal writer owns J**, just as S5c-1 owns GAP writes.

Advance it periodically, at explicit checkpoint, and during clean shutdown **after admission is stopped and commit/apply work is quiescent**. No commit-path fsync.

For a sampled contiguous completion watermark w:

1. Admit a durable-checkpoint message after sampling w.
2. Process prior enqueues; account for enqueue/serialization/put failures through GAP state.
3. Persist any dirty GAP interval.
4. Flush the journal store.
5. Set J=w.
6. Flush again.
7. Acknowledge.

Do not advance J on timeout, error, or lost acknowledgement.

**INFERRED proof:** enqueue-or-gap precedes completion publication. Therefore a sampled completed prefix cannot acquire a new ordinary event obligation ≤w afterward. Processing the subsequent checkpoint, persisting gaps, and flushing events establishes coverage before the J write. A crash leaves either an older J or a J backed by already-durable coverage.

This requires auditing **all terminal paths**, including orphan completion and follower failed-partial-apply handling; a barrier alone is not that audit.

**[S5c-1] OBSERVED:** its acknowledged repair is already serialized through the writer:

- `crates/shamir-tx/src/changefeed.rs:458`–477.
- Durable merge precedes RAM publication: `crates/shamir-tx/src/changefeed.rs:658`–672.

### J is necessary but insufficient

To avoid permanent ambiguity after evidence destruction, also persist **required-version obligations** before their source evidence can disappear:

```text
version-indexed recovery obligation:
    version v
    REQUIRED, or UNKNOWN/PARTIAL
    format/origin discriminator
```

For local transactions, retain the WAL requirement bit until a **batched obligation ledger** is durable. Persist ledger entries before WAL unlink. Ledger entries survive history GC and table drop/rename; delete them only after durable J covers them.

A scalar “reclaimed through R” cannot replace this ledger: it cannot distinguish required versions from eventless/aborted allocations.

**OBSERVED:** current reclamation flushes history and then unlinks WAL, with no journal obligation certificate: `crates/shamir-engine/src/tx/drainer.rs:1377`–1388.

### Upper bound H

Do not call the existing marker the exact maximum recovered committed version.

Use:

```text
H = max(
    J,
    strictly loaded committed/recovery marker,
    surviving positive WAL versions,
    obligation-ledger versions,
    surviving history versions,
    validated surviving journal versions
)
```

For exact new-format paths, obligations identify which members require events. For legacy/partial paths, H is an **affected/allocation floor**, potentially including aborted versions.

The gate floor must be seeded above H **before allocation**, not just above WAL and the marker. Today it uses only those two, with best-effort error swallowing:

`crates/shamir-engine/src/repo/repo_instance.rs:856`–875.

A periodic J does not impose a hard window-size bound if its writer stalls. Bound the **metadata/disk workload**, not falsely promise a fixed number of versions.

---

## 3. Exact reconciliation algorithm

### Inputs and preconditions

- Exclusive startup: no commits, follower applies, GC, table deletion, or WAL truncation.
- Strictly loaded J and GAP interval.
- Surviving validated WAL entries.
- Durable obligation ledger.
- History fallback for legacy/incomplete provenance.
- Actual stored journal records, including physical keys.
- Recovered floor H.

Maintain:

- `R`: definitely REQUIRED versions.
- `N`: versions definitively known not to require an event.
- `U`: unresolved versions/ranges.
- `A`: versions with valid actual journal events.
- `G`: already durably invalidated interval.

### Enumeration

**New-format local paths**

1. Scan surviving WAL; use its explicit requirement bit.
2. Range-read obligation-ledger keys in `(J,H]`.
3. Union REQUIRED versions; preserve UNKNOWN/PARTIAL evidence.
4. A surviving REQUIRED record overrides weaker legacy uncertainty.
5. Do not derive N from mere absence of history or journal rows.

**Legacy paths**

History supplies positive **candidate/uncertain** evidence, not an exact obligation oracle. With no complete obligation provenance, unresolved numeric portions of `(J,H]` must remain U except where authoritative evidence proves N or A.

This intentionally distinguishes “enumerated candidates” from “complete enumeration.” Claiming completeness from a history scan would be incorrect.

### History scan cost

There is **no global version-range seek for record history**. Keys are record-major; the version is a suffix:

- `crates/shamir-tx/src/mvcc_store/mod.rs:61`.
- `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:19`–30.

The timestamp namespace is version-seekable, but it is neither complete obligation evidence nor GC-stable. GC deletes timestamps as well as rows:

`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:109`–117.

**RECOMMENDATION:** a legacy scan streams every history store and filters decoded suffix versions. Cost is **O(total retained history rows/bytes)**, not O(H−J). Memory can be bounded by batches plus interval/set state; I/O cannot.

Do this once for migration, not on every modern restart. A startup budget may **refuse open or require explicit migration**, but must not stop scanning and pretend the remaining history is eventless.

### Reading ACTUAL journal events

Use bounded ascending pages, stopping at H, but add a recovery-facing reader that returns:

```text
physical key
raw value / decode status
next physical cursor
EOF
```

The current values-only API is insufficient for robust recovery enumeration.

**OBSERVED:**

- Byte budget can return a nonempty short page: `crates/shamir-engine/src/repo/changelog_store.rs:63`–78.
- The public reader skips corrupt values and marks the requested cursor, losing the corrupt physical version: **[S5c-1]** `crates/shamir-tx/src/changefeed.rs:543`–554.

Rules:

1. A short page is **not EOF**.
2. Resume strictly after the last physical key consumed.
3. Check physical key length and key-version/payload-version equality.
4. Validate decoding and basic event shape/repository identity.
5. Corrupt records do not enter A.
6. A valid event above H is a stopping condition only after validating its physical ordering.
7. Storage I/O errors refuse open; they are not a successful empty page.
8. An oversized record must be explicitly classified/read or cause refusal—not an endless retry at the same cursor.

Do not use the public `gap_at` result as the enumeration oracle.

### Difference and certification

For complete new-format evidence:

```text
M = R − A − versions already invalidated by durable G
```

For incomplete evidence:

```text
M_conservative =
    M ∪ unresolved candidates/ranges not satisfied by A, N, or durable G
```

- **Empty:** certify no new GAP. Flush actual coverage and advance J to H.
- **Nonempty:** call  
  `record_recovery_gap(min(M_conservative), max(M_conservative))`.
- Then advance J to H through the acknowledged durable checkpoint path.
- Never advance J first.
- Preserve existing G; do not clear it because an event later appears.

**[S5c-1] OBSERVED:** merging widens monotonically, and pulls beyond the affected horizon escape the GAP:

- `crates/shamir-tx/src/changefeed.rs:560`–563.
- `crates/shamir-tx/src/changefeed.rs:605`–608.

The interval spans accounted versions between first and last omission; that is an accepted conservative false-positive effect.

### GC and undecidable requirements

If GC/truncation erased evidence before an obligation ledger was durable, there is no exact repair of the missing knowledge.

**Certify the unresolved portion conservatively.** For an entirely uncertified legacy range this may mean `[J+1,H]`, reduced only by authoritative classifications—not by assuming every absent journal version is an abort.

**Important limitation:** in that state, even a fully persisted journal can receive a false GAP because its absent numeric versions might have been eventless. The no-false-GAP guarantee is achievable only after complete obligation provenance is installed.

---

## 4. Follower-applied versions

**Same comparison model, different evidence producer. J uses local versions on followers too.**

**OBSERVED:**

- Follower allocates a local version: `crates/shamir-engine/src/tx/apply_replicated.rs:391`–411.
- It writes history before downstream preparation: `crates/shamir-engine/src/tx/apply_replicated.rs:458`.
- Downstream enqueue-or-gap precedes publish: `crates/shamir-engine/src/tx/apply_replicated.rs:499`–518.
- The persisted marker is best-effort: `crates/shamir-engine/src/tx/apply_replicated.rs:526`–538.

The relevant windows are:

1. Partial history writes before success/publication.
2. Complete history, before downstream enqueue.
3. Enqueued/published, before journal flush.

S5b-4 closes none of these **persistence** windows.

**RECOMMENDATION:** attach local-version obligation/provenance to follower history writes and retain it until J. Successful nonempty applies require an event; partial attempts require conservative invalidation if their rows survive.

For a no-per-apply-fsync implementation, obligation evidence must be **crash-atomic with each surviving history batch**, including the first partial batch. A separate best-effort metadata write is unsafe.

Do not assume `Store::transact` provides that universally:

`crates/shamir-storage/src/types.rs:179`–198 documents non-atomic defaults; its capability is explicitly **visibility** atomicity at `:257`–274, not a universal cross-store crash-durability promise.

Therefore exact follower coverage requires one of:

- A verified same-store crash-atomic row-plus-obligation implementation.
- A batched write-ahead obligation protocol durably completed before exposing/writing the corresponding data.
- Otherwise the conservative history fallback.

**Decisive choice:** require the first capability for the exact supported path; retain conservative behavior for backends without it. Do not claim identical exactness across all existing adapters.

---

## 5. Open ordering and failure policy

**RECOMMENDATION:**

1. Strictly read recovery metadata and collect WAL/obligation/history evidence.
2. Determine H; seed gate floor before replay/allocation.
3. Replay WAL and persist interner state.
4. Force `changefeed()` creation.
5. Enumerate actual journal coverage.
6. Await `record_recovery_gap` if needed.
7. Await durable J advancement.
8. Only then register/serve the repo or start competing background work.

Use the S5c-1 writer, **not an independent direct marker writer**.

**OBSERVED:** existing recovery already propagates history/interner failures:

`crates/shamir-engine/src/tx/recovery.rs:388`–405.

**[S5c-1] OBSERVED:** feed initialization loads the interval, flushes pending/corrupt replacements before seeding, and then spawns its sole writer:

`crates/shamir-engine/src/repo/repo_instance.rs:1382`–1423.

### The present “before reachable” comment is insufficient

**OBSERVED:** facade `add_repo_as` calls `db.add_repo` **before** recovery:

`crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:378`–386.

`DbInstance::add_repo` inserts into its registry before returning:

`crates/shamir-engine/src/db_instance/db_instance.rs:52`–57.

**INFERRED:** this is not a demonstrated readiness barrier for concurrent access. On recovery failure, returning Err alone does not establish that an already-inserted instance is unreachable.

Move recovery/certification before registry insertion, or enforce an explicit recovering/failed readiness gate on every access path. Cover constructor/config opening and system-store opening as well, not just facade add-repo.

### Failure policy

**Refuse open.**

- Storage/metadata read failures: refuse.
- GAP put/flush failures: refuse.
- J persistence failures: refuse.
- Writer closed/lost acknowledgement: refuse.

Do not proceed with RAM-only GAP. “Read-only” is insufficient unless it also excludes subscriptions, replication pulls, and downstream service; it is additional lifecycle work, not the minimal safe fallback.

---

## 6. Repeated repair, backups, and legacy migration

### Idempotence

- Crash before GAP flush: retry from unchanged J.
- Crash after GAP flush but before J: recompute; existing GAP covers the omissions.
- Crash after J: coverage was already durable.
- Crash during ledger cleanup: leftover obligations are harmless.
- Cleanup must follow durable J, never precede it.

Keep obligations/evidence stable during recovery. Do not allow startup replay to destroy the only evidence before certification.

### Backup/restore

Carry together:

- History and interner.
- Journal.
- GAP interval.
- J.
- Obligation ledger and its format/provenance state.
- Version floor metadata.

A copied J with an older/missing journal is an **unsafe false certificate**. Backup must establish a coherent checkpoint or restore must invalidate J and reconcile conservatively. Merely copying all filenames at unrelated times is insufficient.

Do not lower the allocation floor during restore.

### First pre-S5c start without J

Use `J=0`; perform the full legacy history/WAL/journal inventory.

That inventory cannot recover GC-erased obligations. Therefore the migration must either:

1. Certify its unresolved positive-version interval once and require affected followers to resync; **recommended conservative default**, or
2. Refuse activation until an operator establishes a trusted baseline snapshot.

After durable certification/checkpoint, write J=H and provenance-enabled metadata. Future exact-format restarts need not repeat the legacy full scan.

**The one-time legacy cost cannot be bounded by a version-window seek in the present layout.** It can be memory-bounded, admission-budgeted, and operator-scheduled.

Also, existing GAP remains durable; writing J does not unblock old follower bookmarks.

**OBSERVED:** followers stop before applying accompanying events and automated reseeding remains R2:

`crates/shamir-server/src/replication/follower_loop.rs:308`–325.

The supervisor attempts to persist `resync_required`:

`crates/shamir-server/src/replication/supervisor.rs:309`–334.

---

## 7. Deterministic CI test matrix

Use persisted/staged store separation and hard reopen, not sleeps or assertions about RAM maxima.

Existing seams:

- `PostWalPrePublishHook`: `crates/shamir-engine/src/tx/commit.rs:143`.
- Per-repo `JournalPublishHook`: `crates/shamir-engine/src/tx/tests/journal_publish_order_tests.rs:18`–25.
- `drain_replay`: `crates/shamir-engine/src/tx/drainer.rs:1107`.
- `pre_truncate`/`post_truncate`: `crates/shamir-engine/src/tx/drainer.rs:1382`, `:1388`.
- History-seed failure injection: `crates/shamir-engine/src/tx/recovery.rs:23`–41.
- **[S5c-1]** actual set/flush failure fixture: `crates/shamir-engine/src/repo/tests/recovery_gap_marker_tests.rs:23`.

Add controlled journal-flush, J-flush, obligation-flush, and recovery-ack boundaries.

| Crash-table row | Required assertion |
|---|---|
| WAL accepted, before event preparation | Recovered REQUIRED v gets durable GAP. |
| Prepared, before enqueue | Same; RAM projection does not count as coverage. |
| Failed enqueue, before GAP persistence | Cold reconciliation independently discovers omission. |
| Enqueue succeeded, before publish | Event lost ⇒ GAP; event survives validly ⇒ no new GAP. |
| Published, before event put | Durable GAP after reopen. |
| Event set, before flush | Discard staged event on crash; GAP, regardless of old RAM maximum. |
| Event flushed, before publish | Replay; no new GAP for that version. |
| Published, before drainer | WAL obligation compared against actual journal. |
| During history replay | Partial rows plus retained WAL reconcile correctly. |
| Marked durable, before history flush | WAL remains authoritative evidence. |
| History flushed, before unlink | Same result with duplicate history/WAL evidence. |
| Between sealed-segment deletions | Durable obligation ledger covers deleted entries. |
| After truncation, before journal durability | Empty WAL still produces correct GAP. |
| History and journal both durable | **No new GAP**, including noncontiguous required versions. |

Additional mandatory controls:

- Higher event persisted before lower: detect missing lower event.
- Explicit eventless/aborted versions: no fabricated obligations.
- Non-RecordId staging: requirement bit catches the WAL-projection mismatch.
- System implicit Put/Delete: required, public tx_id zero irrelevant.
- Legacy v=0: no fabricated positive obligation.
- GC removes all original rows: ledger still detects missing event.
- Follower successful and partial apply, with empty WAL.
- Short byte-budget page followed by more events.
- Corrupt value, key/payload mismatch, oversized record, range-read error.
- GAP flush failure/J flush failure: instance never served.
- Crashes before/after GAP and J flush; reopen twice.
- Existing `[10,10]`, new missing 100: cursor 100 gets GAP; cursor 101 does not.
- Backup with coherent coverage versus deliberately mismatched J/journal.
- CI negative controls must remove the relevant flush/provenance write and make tests fail.

Ticket outcomes should be tested against **surviving accepted WAL**, not caller success/error labels.

---

## 8. Smallest defensible PR sequence

| Step | Scope | Purpose |
|---|---|---|
| **S5c-2a: durable accounted J** | `shamir-tx/changefeed.rs`, `engine/repo/changelog_store.rs`, feed initialization, shutdown checkpoint, tests | Explicit writer-owned flush/checkpoint acknowledgement. Not yet sufficient for exact cold detection. |
| **S5c-2b: local obligation provenance** | Versioned WAL format, `tx/commit.rs`, obligation storage helper, `tx/drainer.rs`, truncation tests | Record exact requirement bit; batch durable obligation transfer before unlink; preserve A5/history order. |
| **S5c-2c: follower obligation provenance** | `tx/apply_replicated.rs`, narrow MVCC history-write seam, backend capability/protocol tests | Ensure surviving WAL-less data cannot outlive its obligation evidence. |
| **S5c-2d: reconciliation and readiness** | `tx/recovery.rs`, gate initialization, repo registration/open paths, keyed bounded journal reader, legacy scan, tests | Difference algorithm; durable certification before service; correct H seeding. |
| **S5c-2e: consumer/restore proof** | Replication and backup/restore integration tests | Stop/resync-required, coherent restore, cursor beyond affected horizon. |

This revises the earlier split: **obligation preservation cannot be deferred behind a supposedly exact reconciliation PR**. A pre-unlink scalar horizon alone is not enough.

### Final recommendation

Land S5c-1’s interval and acknowledged writer API. Then implement the revised sequence above.

If the program insists on a smaller immediate cold-recovery PR, ship only the explicitly conservative compatibility algorithm: reconcile authoritative obligations where available, invalidate unresolved coverage otherwise, fail closed on persistence errors, and document that legacy/unsupported follower provenance can force manual resync even when the journal was actually complete. **Do not label that algorithm exact or operationally bounded.**

**Files changed:** none. **Builds/tests/benchmarks run:** none, as requested; verification remains in GitHub CI.
