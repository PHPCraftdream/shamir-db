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
3. **S3 — covering projection rides the tact** (P3.2): the index write +
   covered-projection update join the same ordered write envelope; measure
   write-amplification. Then **P3.3** (planner + index-only read via M2) is a
   separate read-side slice.

**Sequence:** S3 (covering, the perf payoff, building on P3.1's
already-landed DDL/meta — write-side is independent of the TOCTOU) → S2
(Level-3, on real contention need) → S1's TOCTOU fix **only when a real
need justifies its cost** (see §4.1).

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
the documented repro. The **rest of the cell** (S1.0 done; S3 covering;
S2 lock slot) does **not** depend on closing MVCC-2 and proceeds.

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

_Plan revision 2026-06-07 — the record cell folds P2 + P4 + P3.2's
write-side into one atomic per-key write-tact. Supersedes the standalone
MVCC-2 (#232) and LOCKING.md (#234) framings; covering index (#218)
continues on the P3.1 foundation. Next: slice S1 (cell + atomic tact,
TOCTOU dissolved)._
