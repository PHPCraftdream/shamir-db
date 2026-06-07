בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# The record cell — one atomic write-tact (MVCC core)

**Status:** design / proposed (revision 2026-06-07). The keystone that
folds three "remaining" tasks into one: **P2** (MVCC-2 fast-path TOCTOU),
**P4** (Level-3 pessimistic locking), and the write-side half of **P3.2/3**
(covering-index projection maintenance). Companions:
[`TRANSACTIONS.md`](./TRANSACTIONS.md), [`PLAN.md`](./PLAN.md) §2.2,
[`MOVEMENT_B_PERF.md`](./MOVEMENT_B_PERF.md) (covering index).

---

## 0. The one diagnosis

Per-key state in ShamirDB is **scattered across parallel structures**,
reconciled by ad-hoc step sequences and a fast/slow path fork:

| Per-key state | Where it lives today | Updated by |
|---|---|---|
| current value | `main: Arc<dyn Store>` | `main.set` |
| old versions | `history: Arc<dyn Store>` (`<key>::0xFF::<ver>`) | archive on slow path |
| **version / visibility** | `version_cache: SccHashMap<Bytes,u64>` | `upsert_async` (slow path only) |
| index postings | sorted/secondary index entries | index managers |
| *(future)* covered projection | index entry `physical_value` (P3.2) | — not yet |
| *(future)* lock | — (no home) (P4) | — not yet |

A write (`MvccStore::set_versioned`) performs a **sequence**: read-old →
archive-history → main.set → assign-version → version_cache.upsert. The
**fast path** (`gate.active_snapshots_empty()`) skips history + the
version_cache update entirely.

**Every remaining bug/risk lives in that scattering + fork:**
- **MVCC-2 (P2):** the fast path skips `version_cache`, so a snapshot that
  opens in the window between the `active_snapshots_empty()` check and the
  `main.set` sees `current_version() == 0`, routes to `main`, and reads a
  value written *after* it opened. The "naive fix" (always write
  `version_cache`) is **worse**: it would route that snapshot to `history`
  for the old value — which the fast path never archived — so it reads
  *missing* instead of *stale*. (Verified analysis, #232.)
- **Covering staleness (P3.2):** a projection cached in the index entry must
  stay byte-identical to the record; a write that updates `main` but not the
  projection (or vice-versa, or torn) diverges.
- **No lock home (P4):** pessimistic Level-3 needs a per-key place to hold
  the lock + the owner's priority.

These are three faces of one gap: **the per-key write is not a single
atomic transition.**

This is the session's recurring lesson again — *two diverged paths, the
cheap one skips the invariant* — applied to the MVCC core.

---

## 1. The idea: the record cell

Co-locate per-key **coordination state** into one structure — the *cell* —
and make the write **one atomic per-key transition** under the cell's latch.
The bulk data stays in `main`/`history` (dumb-KV); the cell is the in-memory
coordination layer (what `version_cache` already is, grown up).

```text
RecordCell {                       // keyed by record key, in-memory
    version: u64,                  // latest committed version (was version_cache)
    // visibility hint: does `main` hold `version`, and is the prior
    // version archived in `history`? (lets get_at route without a guess)
    lock: Option<LockSlot>,        // P4: owner tx + waiters (None = unlocked)
}
```

> This is the **tuple-header** model real MVCC engines use: Postgres carries
> `xmin`/`xmax` + infomask in the tuple header and the row lock in the lock
> manager keyed by the same tuple — version, visibility, and lock
> **co-located**, which is *why* they have no TOCTOU/staleness by
> construction. ShamirDB grew these apart for early simplicity; converging
> them is the natural maturation of the core.

### The atomic write-tact
A non-tx (or commit-time) write of `key → value` runs **under the cell's
per-key latch**:

```text
latch(cell[key]):
    snap = gate needs the prior version?  ← decided atomically under the latch
    if snap: archive main.get(key) → history at cell.version
    main.set(key, value)
    cell.version = assign_next_version()
release
```

The per-key latch makes the write atomic *for this key* — a reader that
also takes the latch sees the whole pre- or post-state, never a torn one.
The latch is the user's **#4 per-entity atomic**, given a precise home.

> **⚠️ Reality check (2026-06-07): the latch alone is NOT sufficient.** See
> §4.1 — the latch closes torn per-key reads, but **not** the *archive-
> completeness* problem (a snapshot that opens after the write's
> archive-decision but reads at `snap_v < new_v`). That is a cross-cutting
> write-vs-snapshot-open race the per-key latch does not order. The clean
> fix has a real cost; MVCC-2 is LOW and is deferred. Read §4.1 before
> implementing S1.1.

---

## 2. How each "remaining" task falls out

### P2 — MVCC-2 TOCTOU → **dissolved**
`get_at` and the write coordinate through the cell latch. The
`active_snapshots_empty()`-then-`main.set` race is gone: the snapshot
decision is atomic with the write. `version_cache` becomes `cell.version`;
its fast-path skip becomes a *correct* in-tact decision (archive-or-not),
not an *omission*.

### P4 — Level-3 pessimistic locking → **natural**
`cell.lock` is the home. A Level-3 tx acquires the cell's lock before
writing/reading the key; others wait. **Wound-wait on the monotonic
version** (each tx's version is its priority — a total order ⇒ no wait-cycle
⇒ deadlock-free *by construction*, no detector). Lock granularity coarser
than a key (table / index-range, for predicate locks) reuses the SSI
**footprint** vocabulary. Honest caveat unchanged: wound-wait still wounds
younger txns (rare) — "always completes" = "block-not-abort where possible."

### P3.2/3 — covering projection → **rides the same tact (sibling)**
The covered projection is *index-side* (the sorted-index entry's
`physical_value`), not in the record cell — but it is maintained by the
**same write transaction envelope**: when `key` is written at version `V`,
the one ordered tact updates `{ main, cell.version, history?, every index
posting for key, every covered projection for key, counter, changefeed
footprint }` consistently (or in a recovery-safe order). The record cell is
the **heart** (version + visibility + lock); the index postings + covered
projections are the **limbs** — one body, one heartbeat. P3.3 (planner +
index-only read reusing M2 streaming) is read-side and unaffected by the
cell; it just trusts the now-consistent projection.

---

## 3. Recovery & durability (unchanged truth)

The cell is **in-memory coordination only** — exactly like `version_cache`
today. The durable truth stays in `main` / `history` / the WAL. On restart,
a cell is rebuilt lazily by a cold-start range scan (the same mechanism
`current_version` uses now: absent ⇒ `0` ⇒ scan populates). Locks are
in-memory and released on crash (txns abort on recovery). So the cell adds
**no new durability surface** — it reorganizes the in-memory coordination
that already exists, and adds the latch + lock slot.

---

## 4. Slices (TDD, zero-trust, one focused arc)

The ~100 existing `mvcc_store` tests + the `mvcc1_*`/`mvcc2_*` characterization
tests are the safety net.

1. **S1.0 — `RecordCell` wraps `version_cache`** ✅ DONE (`2f12ba5`):
   behaviour-identical foundation; the cell now carries `version`, ready to
   grow a lock slot.
   **S1.1 — the atomic write-tact that closes MVCC-2 — DEFERRED.** See §4.1:
   the sound fix has an irreducible cost (always-archive / commit-barrier /
   version-log restructure), unjustified for a LOW bug that does not bite
   in-memory. The deterministic harness (`c98b343`) documents the repro for
   when a real need (async-disk workload) or restructure (c) arrives.
2. **S2 — lock slot + Level-3.** Add `cell.lock`; a new `IsolationLevel`
   variant (or per-batch flag) for Level-3; acquire/release on the cell;
   **wound-wait on the tx version**. (Supersedes the standalone `LOCKING.md`
   from #234 — the design lives here; build on real contention need.)
3. **S3.2 — covering projection write-side** ✅ DONE (`e288976`): the
   sorted-index entry's `physical_value` is populated from `included_fields`
   and maintained on insert/update/delete + backfill; read path untouched.
   **S3.3 — planner + index-only read — GATED on the atomic write-envelope.**
   See §4.2: the non-tx write path orders `data.delete` *before*
   `on_record_deleted` (and `data.set` before `on_record_updated`), so a
   sorted posting can be **stale w.r.t. the record** in a concurrent window.
   The full-fetch path closes this with `get_many → None → skip` + residual
   re-filter; an index-only read **skips that invariant** and would return a
   phantom (deleted) or stale-projection record. Sound only once non-tx
   writes apply data+postings in one atomic envelope (the cell tact, S1.1's
   option (a)/(b)) — i.e. S3.3 is gated on the same write-atomicity S1.1 needs.

**Sequence:** S3.2 (covering write-side) ✅ → S2 (Level-3, on real
contention need) → the **atomic write-envelope** (S1.1 tact) — which now
has a *second* justification beyond the LOW MVCC-2 bug: it is the
prerequisite that unlocks the covering-index read-side payoff (S3.3) by
making postings non-stale w.r.t. data. → S3.3 (index-only read) rides on
top of that envelope.

---

## 4.1 — S1.1 reality check: closing MVCC-2 has an irreducible cost

Moving to implement S1.1, two "elegant" lock-free designs were
stress-checked against their own interleavings and **both failed** — a
worked example of *prove, don't guess* applied to our own design:

1. **Per-key latch alone (§1's first sketch).** It closes torn per-key
   reads, but not **archive-completeness**: the write decides "archive
   `old`?" by checking `active_snapshots`, but a snapshot can open *after*
   that check and still read at `snap_v < new_v`. The latch is per-key;
   `open_snapshot` is global and does not take it, so the latch does not
   order the write's archive-decision against a later snapshot-open.

2. **Snapshot-epoch + capture-old + recheck (the "beautiful" lock-free
   protocol).** Archive happens *after* `cell.version = new_v` is visible.
   In the window between the cell bump and the archive, a reader with
   `snap_v < new_v` routes to `history` and finds **nothing** → `None`
   (worse than the original stale read). Moving the archive *before* the
   cell bump re-opens the completeness hole (late snapshots uncovered).
   The two requirements — *archive-before-cell-bump* (reader safety) and
   *decide-archive-after-the-write-window* (completeness) — **contradict**;
   one epoch cannot satisfy both.

**Why it's irreducible here:** the write *overwrites* `main` in place, so
the prior value's only refuge is `history`, and the "who needs the prior?"
question is inherently cross-cutting (it depends on snapshots that may open
at any time before the commit becomes visible). Closing it soundly needs
**one** of:
- **(a) always-archive** — copy prior → history on every overwrite,
  unconditionally. Correct by construction (no race), but **2× write per
  overwrite** (GC reclaims it when unwatched — but the write already paid
  the cost). Bad trade for a LOW bug.
- **(b) a commit barrier** — a brief *exclusive* section per non-tx write
  that `open_snapshot` also respects, so the archive-decision and the
  version-publish are atomic against snapshot-open. Correct, but adds a
  global serialization point to the hot write path — the very thing the
  fast path exists to avoid.
- **(c) a write-new-aside / version-log restructure** — never overwrite
  `main`; write the new version to a version-keyed log and install the
  read-cache atomically (the canonical MVCC layout). Correct and fast, but
  a **significant architectural change** to the storage model.

**Decision.** MVCC-2 is **LOW** — it does not reproduce on the in-memory
backend (no `.await` in the fast-path window ⇒ no task switch under
single-threaded cooperative tokio) and bites only an async-disk backend
under true parallelism. Paying (a)'s write-amplification or (b)'s hot-path
barrier to fix a LOW bug is the wrong trade; (c) is a real milestone, not a
slice. **So S1.1's TOCTOU fix is deferred** until either an async-disk
workload makes it real or we undertake (c) deliberately (with loom-grade
verification — clever reasoning is demonstrably not enough here, as the two
failed designs above show). The deterministic harness (`c98b343`) stays as
the documented repro. The **rest of the cell** (S1.0 done; S3.2 covering
write-side done; S2 lock slot) does **not** depend on closing MVCC-2 and
proceeds; **S3.3 (index-only read) does** — see §4.2.

---

## 4.2 — S3.3 reality check: index-only read is gated on write-atomicity

Moving to implement S3.3 (the read-side payoff — skip the record fetch and
serve a covered query straight from the index entry's `physical_value`),
the non-tx write path was traced and **proves a stale-posting window** that
index-only reads cannot tolerate. Another *prove, don't guess* result.

**The proof (non-tx path, `table_manager.rs`).** Delete:

```text
1595  let r = self.table.delete(id).await?;          // 1. data gone
1601  self.index_manager.on_record_deleted(...).await?;     // 2. postings removed
1605  self.sorted_indexes.on_record_deleted(...).await?;    // 3. sorted postings removed
```

Update with a changed indexed key is the mirror image: `data_store.set`
(line ~1670) runs *before* `sorted_indexes.on_record_updated` (line ~1694),
which emits `RemovePosting(old) + SetPosting(new)`. In both cases the data
store is mutated **before** the sorted posting is reconciled, so between the
two steps a concurrent reader observes a posting whose record is **deleted**
(delete) or **changed** (update). This is a normal-operation concurrency
window, not merely a crash-recovery one.

**Why the full-fetch path is safe and index-only is not.** The full-fetch
read (`read_sorted_index_scan`) does `get_many` on the looked-up ids:
> "stale index entries materialise as `None` and are silently skipped"
> (read_exec.rs ~712), and a surviving record is re-checked by the residual
> filter.

That `None`-skip + residual re-filter **is the invariant** that closes the
window. An index-only read serves the row from the posting's
`physical_value` and **never fetches the record**, so it skips the
invariant and would return a phantom (deleted) row or a stale projection.
This is the session's recurring bug-class exactly — *two diverged paths,
the cheap one skips the invariant* — and building index-only now would
re-introduce it.

**Why it's irreducible here.** The transactional commit path already applies
data + index ops **atomically** (commit pipeline, "a dropped tx leaves no
ghost postings", `table_manager.rs` ~906) — so under tx writes there is no
window. The **non-tx** path does not. Making index-only sound therefore
requires the non-tx write to apply `{ data, version, postings, covered
projection }` in **one atomic envelope** — which is precisely the cell's
**atomic write-tact** (§1, S1.1). So:

- **S3.3 is gated on the atomic write-envelope** (S1.1 option (a) always-
  archive-style ordering / (b) commit-barrier / (c) version-log restructure).
- This **raises the value** of building that envelope: §4.1 deferred it as
  serving only a LOW bug (MVCC-2); it now *also* unlocks the covering-index
  read-side perf payoff (S3.3) and removes the non-tx phantom window. The
  envelope is the shared keystone for **three** wins (MVCC-2, index-only
  reads, non-tx posting consistency), which changes its cost/benefit.

**Decision.** S3.3's index-only read is **deferred**. The covering projection
is already written and maintained (S3.2) and persists for free, so when the
gating lands the read-side is a clean, independent slice on a sound
foundation. Shipping index-only before it would be unsound under concurrent
non-tx writes.

The deterministic regression that pins this window —
`covering_delete_window_exposes_stale_posting` (`7c09ee9`) — runs the real
`TableManager::delete` against a `PausableInfoStore` that suspends the
sorted-posting removal, and asserts that inside the window the record is gone
(`count() == 0`) while the posting + its covering projection are still
present. Green today (characterizes the bug); flips when the gating closes
the window.

### 4.2.1 — the gate is the *cheap* read-validation, not the MVCC-2 fix

Refinement (2026-06-07): closing this window for index-only reads does **not**
require the expensive MVCC-2 remedies of §4.1 (always-archive / commit-barrier
/ version-log restructure). It requires only the **cheap, lock-free read half
of the cell** — an **optimistic version stamp**, the in-memory analogue of
Postgres's visibility map:

- `RecordCell.version` already exists (S1.0, `2f12ba5`) — the authoritative
  current version of the key.
- The covering posting **embeds the version** it was written at (alongside the
  projection in `physical_value`).
- **Writer:** bump `cell.version` (one atomic store) as the *first* step, then
  write data + postings. From that instant every stale posting is marked
  distrusted.
- **Index-only reader:** decode the posting, compare `posting.version` to
  `cell.version` (one atomic load). Match + live ⇒ trust the projection (fast
  path, no fetch); mismatch / tombstone ⇒ fall back to `get_many` (the existing
  `None`-skip). In the delete window the version bump precedes the data delete,
  so the lingering posting's version no longer matches ⇒ the reader falls back
  ⇒ **no phantom**.

This is per-key and lock-free — one atomic increment per write, one atomic load
per read — so it preserves the zero-overhead-when-unwatched property and does
**not** add the global barrier the §4.1 MVCC-2 fix would. It is a direct
continuation of the already-landed S1.0 (`RecordCell.version`) + S3.2
(projection). **So S3.3 is gated on this optimistic-version validation, which
is cheap and buildable now — distinct from the heavier, still-deferred MVCC-2
remedy (§4.1).** The full atomic write-tact (§1 latch) remains the deeper
keystone for Level-3 (S2) and MVCC-2, but index-only reads do not need to wait
for it. *(Contemplated; implementation deferred by decision — recorded here so
the path is unambiguous when it resumes.)*

---

## 5. Discipline (this is the MVCC core — where subtle bugs breed)

- **S1 is a write-path/MVCC-core refactor.** P2 already showed a one-line
  "fix" makes it *worse*. Move in small, test-pinned steps; never weaken a
  green `mvcc_store` test without a written reason.
- The cell must not regress the **zero-overhead-when-unwatched** property
  (P1/#233): the latch is per-key and uncontended on the common path; the
  lock slot is `None` until Level-3 is used.
- Keep the durable truth in `main`/`history`/WAL — the cell stays in-memory,
  rebuildable, adding no recovery surface.

---

## 6. Per-table write strategy: consensus (seqlock) vs escape (version-log)

> **⚠️ Superseded / refined by [`TEMPORAL.md`](./TEMPORAL.md) §1 (2026-06-07).**
> The "on-the-fly switchable type" requirement resolved this fork: the
> substrate is a **single version-log** (escape), and a table's "type" is a
> runtime **retention policy** on top — not two physical layouts behind a
> trait. So the seqlock/consensus handshake is **not built** (the version-log
> dissolves MVCC-2 by construction, leaving nothing to coordinate), and §7's
> R2 (extract a two-impl `WriteStrategy` trait) is **dropped**. The framing
> below is kept for the reasoning trail; the decision lives in `TEMPORAL.md`.

Closing MVCC-2 has two legitimate shapes, and they express different souls.

**Seqlock — consensus.** Keep overwrite-in-place + `history`, but make the
fast-path archive decision *correct* with an optimistic per-key handshake:
the writer reads the old value, marks the key "write-in-flight", records a
global snapshot-epoch, overwrites `main`, publishes the version, then
re-reads the epoch — if a snapshot opened during the window the writer
retroactively archives the old value; a reader that needs the key **waits on
the in-flight flag** until the writer (and its retro-archive) completes. The
contested moment is *met and resolved*: two actors detect each other and
agree (archive-or-not, wait-or-proceed). Cost is paid **only on a true
same-key collision** (rare); it is the per-key, optimistic form of §4.1's
"commit-barrier" (b). The catch: this is a consensus protocol → memory
ordering / ABA / lost-wakeup corners (we hit a real lost-wakeup in S2's
`lock_key`) → **loom-verified, no hand-waving**.

**Version-log — escape.** Never overwrite; append each write as a new
version (`key::version → value`); "current" = max version; a snapshot at `Vs`
reads the largest version `≤ Vs`. There is no contested point → the race is
**structurally impossible**. Visibility ("which version?") is no new
complexity — it is exactly what `get_at` already computes; append merely
*merges* `main`+`history` into one log so the "main or history?" fork
disappears. The genuine new cost is **background vacuum** (reclaim versions
no snapshot needs), off the hot path. The canonical MVCC layout (Postgres
heap / log-structured).

**The optionality (decision).** Both are valid; the choice is a property of
the **table's physical layout**, therefore:
- **NOT per-transaction** — a tx chooses *isolation* (Snapshot / Serializable
  / Pessimistic), which rides on top of *either* substrate. The substrate is
  the store, not the tx.
- **NOT runtime-auto** — switching a live table's strategy is a data
  migration (reshape main+history ↔ log) + adaptive unpredictability.
- **YES per-table, chosen by the schema author at DDL** — like a storage
  engine or index type; same opt-in philosophy as the covering index.

| pick **seqlock** (consensus / overwrite) | pick **version-log** (escape / append) |
|---|---|
| live mutable state | immutable ledger / facts |
| frequent writes, short/rare snapshots | long/concurrent snapshots, time-travel |
| point reads of "current" | "as-of" reads, audit, historical analytics |
| want minimal storage | append-natural data, accept vacuum |
| *e.g.* presence, balance, session | *e.g.* chat history, event log, audit-trail |

**Domain alignment.** An **Interconnected** DB (the *I* in S.H.A.M.I.R. —
chat / P2P) naturally holds *both*: ephemeral live state (presence →
seqlock) and immutable history (messages → version-log). Per-table
optionality is not a crutch — it acknowledges the system has two natures and
lets each table take the one matching its data.

**Mechanism (the seam):** one `WriteStrategy` trait, two impls
(`OverwriteSeqlock`, `AppendVersionLog`), selected from table config at open.
Everything above — tx, Level-3 locks, covering, cell version/`hwm`, the
visibility rule "largest version ≤ snapshot" — is **identical** for both, so
the rest of the engine never knows which is underneath.

**Cost honesty.** Two strategies = **2× the most bug-prone core** to
loom/test/maintain. So: **design the seam now (§7); build the second impl
only when a real table needs it.** Today's overwrite+`history` lives; seqlock
*completes* it (closes MVCC-2). Version-log is added when a table genuinely
needs time-travel / long snapshots. An optional future "advisor" may *measure*
contention / snapshot-longevity and *recommend* a switch — but the decision
and migration stay with the human; no auto-switching of a live store.

## 7. Refactor to the strategy seam — no new functionality

Before either strategy is built, shape `MvccStore` so the future
`WriteStrategy` cut is clean and obvious — **behaviour-identical, zero new
features, every `mvcc_store` test green unchanged.**

**The seam line.** Three layers, today tangled in `MvccStore`:
1. **Coordination** — `cell` (version/`hwm`), the `gate` (version assignment,
   snapshots). *Shared by both strategies — stays.*
2. **Level-3 locks** — `locks` map, `lock_key`/`release_locks`. *Strategy-
   agnostic (locks keys, not values) — stays on `MvccStore`.*
3. **Versioning substrate** — how a value is persisted and how a versioned
   read resolves: `set_versioned[_many]`, `delete_versioned`, `get_at`,
   `scan_history_for_version`, `apply_committed_ops`, the `main`/`history`
   stores. *This is the strategy.*

**R1 — clarify in place (do first; pure health + seam-readiness).**
The four write paths (`set_versioned`, `set_versioned_many`,
`delete_versioned`, `apply_committed_ops`) duplicate the same shape:
archive-prior-if-needed → write → assign version → publish cell (bump-first).
Extract that duplication into named private helpers —
`archive_prior(key, prev_v)`, `publish_cell(key, v, slow)`,
`resolve_read(key, snap_v, cur_v)` — and group the substrate methods into one
clearly-marked region, separate from coordination and locks. This **reduces
the bug surface** of the most dangerous subsystem (one archive path, not
four) *and* makes each helper a ready future strategy-method. Behaviour-
identical; no trait yet.

**R2 — extract the trait.** ⚠️ **DROPPED** (see `TEMPORAL.md` §1): there are
not two layouts to abstract over — the substrate is a single version-log and
"type" is a runtime retention policy. R2 is replaced by the version-log +
retention substrate work (TEMPORAL.md T1). Original (now-moot) framing:
Lift the substrate region behind `trait WriteStrategy`; move today's bodies
verbatim into `OverwriteHistoryStrategy`; `MvccStore` holds
`Box<dyn WriteStrategy>` + `locks` + delegates. Keep the GC/recovery
accessors (`main_store`/`history_store`) concrete on the overwrite impl for
now — they differ per strategy and genericizing them blind risks the wrong
boundary (the classic "don't extract an interface until you have two impls").
R1 makes R2 mechanical; doing R2 only with the second impl in hand avoids a
mis-cut seam.

**Discipline.** R1/R2 add **no behaviour** — they are the preparation, not
the feature. The features (seqlock completion of MVCC-2, version-log) land
*after*, each loom-verified. Keep the durable truth in the stores/WAL; the
cell stays in-memory and rebuildable.

---

_Plan revision 2026-06-07 — the record cell folds P2 + P4 + P3.2's
write-side into one atomic per-key write-tact. Supersedes the standalone
MVCC-2 (#232) and LOCKING.md (#234) framings; covering index (#218)
continues on the P3.1 foundation. §6 records the per-table consensus/escape
write-strategy optionality; §7 the behaviour-preserving refactor to its seam
(R1 clarify now, R2 extract on need). Next: slice S1 (cell + atomic tact,
TOCTOU dissolved) via the chosen strategy._
