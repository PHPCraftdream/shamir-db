# D2/R2 — Abort-path census (CompletionTracker.mark coverage)

**Date:** 2026-06-15
**Scope:** Read-only audit. No code changes.
**Goal:** For every site that allocates an MVCC version via
`assign_next_version` / `version_counter.fetch_add`, enumerate every exit path
between the allocation and the terminal `completion().mark(V, …)` call. Flag
any path where a version is consumed but never marked — under the D2 refactor
(out-of-order / background materialize), such a leak permanently wedges the
`visibility_watermark` at `V-1`.

---

## 1. Version-allocation sites (production code only)

Tests are excluded. Bench code is excluded.

| # | Site (file:line) | Caller context | Tracker-coupled? |
|---|---|---|---|
| A | `crates/shamir-tx/src/repo_tx_gate.rs:248` (`fetch_add`) | The single allocator. All call-sites below funnel here via `assign_next_version`. | n/a — primitive |
| B | `crates/shamir-engine/src/tx/pre_commit.rs:198` | `pre_commit_locked_validate` (lockfree commit path + group-commit leader path) | YES — explicit abort marks in same fn + caller |
| C | `crates/shamir-engine/src/tx/pre_commit.rs:286` | `pre_commit_locked` (legacy AsyncIndex commit path, single-tx) | YES — explicit abort marks in same fn |
| D | `crates/shamir-tx/src/mvcc_store/mod.rs:247` (`set_versioned`) | **Non-tx** single write | **NO** — never touches `CompletionTracker` |
| E | `crates/shamir-tx/src/mvcc_store/mod.rs:303` (`set_versioned_many`) | **Non-tx** batch write | **NO** — never touches `CompletionTracker` |
| F | `crates/shamir-tx/src/mvcc_store/mod.rs:349` (`delete_versioned`) | **Non-tx** delete | **NO** — never touches `CompletionTracker` |

Sites B and C are the tx commit pipeline. Sites D, E, F are the non-tx write
path; they bypass the tracker entirely and bump `last_committed_version`
directly via `publish_committed_max` (`fetch_max` CAS). See §4 below.

---

## 2. Per-path trace — tx commit (sites B, C)

### 2.1 Site B — `pre_commit_locked_validate` @ `pre_commit.rs:198`

Caller chain:
- `commit_tx_lockfree` (`commit.rs:428`) — the **lockfree path** (default for
  non-AsyncIndex txs).
- `run_leader` / `run_single_tx` (`group_commit.rs:161, 423`) — group-commit
  path.

| Exit path (file:line) | Cause | Marks? | Site |
|---|---|---|---|
| `pre_commit.rs:212` `return Err(SsiConflict)` | SSI read-set conflict | **YES** | `pre_commit.rs:211` `mark(Aborted)` |
| `pre_commit.rs:227` `return Err(PhantomConflict)` | predicate phantom | **YES** | `pre_commit.rs:226` `mark(Aborted)` |
| `pre_commit.rs:235` `return Ok(None)` | C6 empty-tx fast-path | **YES** | `pre_commit.rs:234` `mark(Aborted)` |
| `pre_commit.rs:239` `maybe_crash("pre_commit", ..)` | debug crash injector → `process::abort()` | n/a | HARD crash, no WAL durable yet; recovery sees no inflight marker, the burned version is lost on disk — gate is rebuilt from `max(persisted marker, max inflight)` on restart, so no watermark wedge |
| `pre_commit.rs:260` `return Ok(Some(_))` | normal success | n/a — passes to caller | caller must mark before / after WAL |
| **Caller**: `commit.rs:461` `return Err(Storage)` (lockfree path) on `wal.begin_grouped` failure | WAL begin I/O fail after assign | **YES** | `commit.rs:456` `mark(Aborted)` |
| **Caller**: `commit.rs:477` happy path → `materialize` → `materialize.rs:212` `mark(Materialized)` | normal commit | **YES** | inside `materialize` |
| **Caller (group-commit, leader)**: `group_commit.rs:208` `return Err(PhantomConflict)` on intra-batch phantom for the LEADER | intra-batch phantom | **YES** | leader: `group_commit.rs:191` `mark(Aborted)`; already-validated peers: `group_commit.rs:196` mark + sender notify |
| **Caller (group-commit, follower)**: `group_commit.rs:215` `continue` after intra-batch phantom | follower phantom | **YES** | `group_commit.rs:191` `mark(Aborted)` |
| **Caller (group-commit, leader)**: `group_commit.rs:276` `return Err(e)` on SSI/phantom/unique fail of leader | err out of `pre_commit_locked_validate` for leader | **YES** for leader (already marked inside `pre_commit_locked_validate`) **AND** for all previously-validated peers: `group_commit.rs:263` `mark(Aborted)` |
| **Caller (group-commit, follower)**: `group_commit.rs:282` `continue` on follower err | err for follower | **YES** (marked inside `pre_commit_locked_validate`) |
| **Caller (group-commit)**: `group_commit.rs:309` `return Err(Storage)` on `begin_grouped_many` failure | batch WAL begin I/O fail | **YES** — `group_commit.rs:300` `mark(Aborted)` for every survivor in the batch |
| **Caller (group-commit)**: `Drop` of `PanicGuard` (`group_commit.rs:48-60`) | panic anywhere from `panic_guard.versions.push(vpc.commit_version)` (`group_commit.rs:220`) until `mem::forget(panic_guard)` (`group_commit.rs:350`) | **YES** — for every version recorded in `panic_guard.versions` |
| **Caller**: `materialize` then `post_publish_cleanup` after a successful `wal.begin_grouped` | best-effort projections | **YES** — `materialize.rs:212` `mark(Materialized)` is unconditional (Phase 6 always runs, regardless of `ok`) |

### 2.2 Site C — `pre_commit_locked` @ `pre_commit.rs:286`

Caller chain: `commit_tx_inner_legacy_async` (`commit.rs:333`) — the
**AsyncIndex** commit path. Only `tx.visibility == AsyncIndex` reaches here.

| Exit path (file:line) | Cause | Marks? | Site |
|---|---|---|---|
| `pre_commit.rs:306` `return Err(SsiConflict)` | SSI | **YES** | `pre_commit.rs:305` `mark(Aborted)` |
| `pre_commit.rs:321` `return Err(PhantomConflict)` | phantom | **YES** | `pre_commit.rs:320` `mark(Aborted)` |
| `pre_commit.rs:337` `return Ok(None)` | C6 empty-tx | **YES** | `pre_commit.rs:336` `mark(Aborted)` |
| `pre_commit.rs:343` `maybe_crash("pre_commit", ..)` | debug-only HARD crash | n/a | nothing durable; recovery seeds gate from persisted marker |
| `pre_commit.rs:376` `return Err(Storage)` on `wal.begin_grouped` failure | WAL begin I/O fail | **YES** | `pre_commit.rs:375` `mark(Aborted)` |
| `pre_commit.rs:383` `maybe_crash("phase4", ..)` | debug-only HARD crash AFTER WAL durable | n/a | WAL entry survives → recovery replays + marks Materialized (`recovery.rs:268`) |
| `pre_commit.rs:388` `return Ok(Some(_))` | success | n/a | passes to caller |
| **Caller**: `commit.rs:374` `mark(Materialized)` | normal success after `apply_data_phase` + `apply_counter_phase` | **YES** | unconditional |

#### CAVEAT for site C — panic between WAL durable and `mark(Materialized)`

`commit_tx_inner_legacy_async` (`commit.rs:333-407`) has **no `PanicGuard`**.
Between `pre_commit_locked` returning `Ok(Some)` (WAL is durable, version is
allocated) and the explicit `mark(Materialized)` at `commit.rs:374`, the
following calls run:

- `commit.rs:365` `release_pessimistic_locks` — async iteration; could in
  principle panic on a poisoned `MvccStore` invariant (very unlikely).
- `commit.rs:369` `project_event` — pure projection.
- `commit.rs:371` `apply_data_phase` — async; logs+ swallows DbError (does
  NOT propagate or panic on normal failures).
- `commit.rs:372` `apply_counter_phase` — same.

Under the current design a panic on this path would leak the version
(`mark(Materialized)` never runs; no PanicGuard catches it). In practice
`apply_data_phase`/`apply_counter_phase` are designed not to panic on I/O
failure (they log and flip `async_prefix_failed`), but the **invariant is
not enforced by the type system**. For D2 this becomes a stronger concern
because background materialize widens the window between assign and mark.

**Verdict for site C:** ALL non-panic paths covered. Panic path:
RISK — same risk as group-commit had before `PanicGuard` was added.

### 2.3 Recovery (post-restart, `recovery.rs:245`)

`recover_inflight_v2` builds the gate first (so the tracker watermark seeds
at `max(persisted marker, max inflight commit_version)`), then for every
inflight WAL entry it calls `gate.completion().mark(commit_version,
Materialized)` at `recovery.rs:268` and finally
`sync_last_committed_from_watermark` at `recovery.rs:279`. Versions burned
in-flight by aborts (which never produced a durable WAL entry) are
implicitly absorbed by the gate's initial watermark seed — the
`CompletionTracker::with_watermark(initial)` at `repo_tx_gate.rs:155` jumps
past any sub-`initial` gap. **No leak across restart.**

---

## 3. HOLES (CRITICAL for D2)

The following paths consume a version through `assign_next_version` but rely
on conditions outside the type system to mark it terminally:

### H1 — `commit_tx_inner_legacy_async` panic between WAL-durable and `mark(Materialized)`

- **Assign site:** `pre_commit.rs:286` (via call at `commit.rs:345`).
- **Window of risk:** `commit.rs:345` success path → `commit.rs:374` mark.
- **Trigger:** any panic in `release_pessimistic_locks`, `project_event`,
  `apply_data_phase`, `apply_counter_phase`.
- **Today:** rare (functions are panic-conservative). Under D2 the window
  widens because materialize moves background → larger panic surface.
- **Fix for D2:** wrap site C's post-pre_commit body in a `PanicGuard` mirroring
  `group_commit.rs:38-60`, OR migrate AsyncIndex visibility through the same
  lockfree path that already has the panic-disarm pattern in `run_leader`.

### H2 — `commit_tx_lockfree`: panic between WAL-durable and `materialize`'s `mark(Materialized)`

- **Assign site:** `pre_commit.rs:198` (via `commit.rs:428`).
- **Window of risk:** `commit.rs:462` success of `wal.begin_grouped` →
  `materialize.rs:212` `mark(Materialized)`.
- **Trigger:** panic in `record_commit_writes`, `release_pessimistic_locks`,
  `project_event`, `materialize` body.
- **Today:** same low panic probability. Under D2 this window is the
  background task — its panic does NOT propagate to the caller, so a silent
  watermark wedge is the most likely manifestation.
- **Fix for D2:** wrap the background materialize body in a `PanicGuard`
  whose `Drop` marks the version `Aborted` if `std::thread::panicking()`,
  symmetric to `group_commit.rs:48`.

### H3 — `materialize` returns without marking (theoretical)

- `materialize.rs:212` `mark(Materialized)` is unconditional today (no
  early-return between the function entry and the mark site). A future
  refactor that adds an early `return` between the function entry and
  line 212 would silently leak.
- **Fix for D2:** convert `materialize` to take a RAII completion guard
  (constructed at function entry, with `Drop` defaulting to
  `mark(Aborted)`; the success path calls `guard.commit()` to flip the
  recorded state to `Materialized` before the guard drops).

### H4 — Group-commit `PanicGuard` versions vector window

- `panic_guard.versions.push(vpc.commit_version)` happens at
  `group_commit.rs:220` — i.e. AFTER `pre_commit_locked_validate` has
  already returned `Ok(Some(vpc))` and the version is live.
- Between `pre_commit_locked_validate` return and `panic_guard.versions.push`
  there is no `.await`; the gap is panic-only. Currently safe (the gap is
  three lines of pure code).
- **Fix for D2:** push the version into `panic_guard.versions` BEFORE
  building any per-survivor state — or have `pre_commit_locked_validate`
  return a `VersionGuard` that holds the abort-mark obligation by RAII.

---

## 4. Non-tx writes vs. CompletionTracker

Non-tx writes (`set_versioned`, `set_versioned_many`, `delete_versioned` —
sites D, E, F) **bypass the `CompletionTracker` entirely**. They:

1. `assign_next_version` (`mvcc_store/mod.rs:247, 303, 349`).
2. `publish_cell` (versioned-cell map).
3. Write to `history` store (sole durable op).
4. `record_ts` (best-effort).
5. `publish_committed_max(new_v)` — direct atomic `fetch_max` on
   `last_committed_version` (`repo_tx_gate.rs:276`).
6. `vacuum_key`.

There is no `completion().mark` call anywhere in `mvcc_store`. The non-tx
path advances `last_committed_version` **independently** of the
`CompletionTracker.watermark`.

### Why it currently works (under inline materialize)

`sync_last_committed_from_watermark` only ever **moves the atomic forward**
(it goes through `publish_committed_max`, which is a CAS-bounded `fetch_max`).
So under today's inline materialize:

- tx path: mark → watermark advances → CAS pushes `last_committed_version`
  upward.
- non-tx path: directly CAS-bumps `last_committed_version` upward.
- The two streams **commute** because both use `fetch_max`, and version
  allocation is from a single monotonic counter — so a non-tx version is
  never re-allocated by a tx and vice versa.

### Why it breaks under D2

D2's contract is "readers gate on `CompletionTracker.watermark`, not on
`last_committed_version`" (the watermark gives the contiguous-prefix
invariant that out-of-order materialize requires). Once readers consult the
watermark:

- A non-tx write at version V advances `last_committed_version` but does
  NOT touch the watermark. The watermark stays at V-1.
- If reads gate on `watermark`, every non-tx write becomes invisible to
  readers until SOME tx happens to mark a version ≥ V on the tracker (which
  may never happen on a non-tx-heavy workload).
- Conversely, if reads gate on `last_committed_version` (today's
  behaviour preserved), then a tx version V allocated but stuck in
  background materialize is observable by readers as visible because a
  later non-tx write at V+1 will CAS `last_committed_version` to V+1 — but
  the tx at V hasn't projected yet (data isn't in `main`). **That is the
  D2 visibility hazard in the other direction.**

**Verdict:** D2 cannot ship without unifying the two streams. The non-tx
path MUST go through `completion().mark(V, Materialized)` (after its
durable history write), OR the non-tx path MUST stop advancing
`last_committed_version` directly and instead funnel its publish through
the tracker. The two-watermark design is a D2 blocker.

---

## 5. Recommendations for D2 — required `mark(Aborted)` sites

### 5.1 New mandatory marks

1. **Background materialize panic guard.** In whatever future function
   takes over from `materialize` as the background body, wrap it in a
   `PanicGuard` that on `Drop` while `thread::panicking()` calls
   `mark(Aborted)` for the version. Mirror `group_commit.rs:38-60`.

2. **`commit_tx_inner_legacy_async` panic guard.** Same RAII pattern in
   `commit.rs:333-407` between WAL begin success and `mark(Materialized)`.

3. **RAII `VersionGuard` for the allocation itself.** Stronger than #1/#2.
   Have `assign_next_version` return a `VersionGuard { version, gate }`
   whose `Drop` calls `mark(Aborted)` unless `guard.commit()` was invoked
   first. Every site that allocates a version then becomes statically
   leak-safe; the compiler enforces the obligation. Migrate sites B, C, D,
   E, F to this guard.

4. **Non-tx path: mark Materialized.** After the durable history write in
   `set_versioned` / `set_versioned_many` / `delete_versioned`, call
   `self.gate.completion().mark(new_v, Materialized)` and replace the direct
   `publish_committed_max` with `sync_last_committed_from_watermark`.
   This is the D2 unification required by §4.

5. **WAL begin failure in batch.** Already covered
   (`group_commit.rs:300`); preserve under D2 batcher.

### 5.2 Defer assign as late as possible

Today both `pre_commit_locked` and `pre_commit_locked_validate` allocate the
version BEFORE Phase 2 (SSI validate) and Phase 2-bis (predicate validate).
This was a deliberate P2a change — it lets the version-counter advance
lock-free even on conflict aborts. The cost is that the SSI/phantom abort
paths burn a version each.

Under D2 we want fewer abort paths between assign and mark, not more. Two
options:

- **Option A — keep early assign.** Pay the cost (cheap: SSI/phantom marks
  are already wired). Match every new exit path with a mark. The RAII
  `VersionGuard` from §5.1#3 enforces this.

- **Option B — defer assign to after SSI/phantom validation.** Move
  `gate.assign_next_version()` to immediately before the WAL-entry build
  (`pre_commit.rs:252` / `pre_commit.rs:369`). Pros: SSI/phantom abort
  paths consume no version, watermark progresses faster. Cons: the
  validation phases lose access to `commit_version` (currently unused
  during validation — no observed regression). The change is purely
  mechanical at the assign-site level.

**Recommended for D2:** **Option B + RAII VersionGuard**. The assign moves
to the latest possible point (immediately before WAL begin), and the guard
makes the remaining short window (assign → WAL begin → enqueue background
materialize) statically leak-proof. Combined with §5.1#4, the watermark
becomes the single source of truth for visibility.

---

## 6. Summary table — count of holes

| Hole | File:line | D2 severity | Required fix |
|---|---|---|---|
| H1 | `commit.rs:345-374` panic window in `commit_tx_inner_legacy_async` | HIGH — widens under D2 background tail | `PanicGuard` or RAII `VersionGuard` |
| H2 | `commit.rs:462` → `materialize.rs:212` panic window in `commit_tx_lockfree` | CRITICAL — D2 moves this to background task; panic = silent wedge | `PanicGuard` inside background body |
| H3 | future-refactor early-return inside `materialize` | LOW today, MEDIUM under D2 | RAII `VersionGuard` |
| H4 | `group_commit.rs:161-220` pre-`panic_guard.versions.push` window | LOW (no `.await`, 3 lines) | RAII `VersionGuard` |
| H5 | non-tx path (sites D, E, F) never marks tracker | **BLOCKER** for D2 | non-tx must call `mark(Materialized)`; remove direct `publish_committed_max` |

**Holes that are LEAKS today (pre-D2):** zero on the happy path; all
identified holes require a panic in a panic-conservative region to manifest.

**Holes that BECOME leaks under D2:** H2 (background panic silent), H5
(two-watermark design).
