# Brief 50 — #1055: RFC v2 revision for online CREATE INDEX

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

This is a **design-document revision task, NOT an implementation task**. Do
not write or modify any production code. The only file you edit is the RFC.

## File to revise

`docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md` (839
lines, status "DRAFT — pending review"). Edit it in place to become v2 —
update the header to note "v2 — revised 2026-08-09, see revision notes
below" and add a short revision-log section near the top summarizing what
changed and why (2-3 sentences per point, pointing at the sections below for
detail). Do not delete the original content's substance — correct it in
place, keeping the document's existing structure and numbering.

## Input document — read this FIRST, in full

`docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md` — the
completed, corrected write-path audit. Its conclusion is the authoritative
input for §2.3's rewrite (point 4 below): tx-staged writes and non-tx CRUD
writes are NOT independent mechanisms — both funnel through
`IndexManager::plan_record_created`/`plan_record_updated`/
`plan_record_deleted` (and `SortedIndexManager`'s equivalents), which
iterate ALL registered index defs with NO `IndexState` filter and produce
`SetPosting`/`RemovePosting` ops directly. This is safe today only because
`begin_write_barrier` holds across the whole Phase 1→2→3 sequence; removing
that barrier is exactly what reopens the race. The audit recommends the
capture point live INSIDE those shared planning methods (a single choke
point), not scattered across ~15 call sites in
`table_manager_tx_ops.rs`/`table_manager_crud.rs`.

## The 4 corrections to make (each independently verified against the code on 2026-08-09)

### 1. §6.1 / Open Question 1 — already closed, remove it

The RFC's OQ1 claims no `pub` table-wide "current committed version"
accessor exists at the `TableManager` layer. False:
`MvccStore::current_committed_version()` is `pub`
(`crates/shamir-tx/src/mvcc_store/mod.rs:266`, added by F-71/#898).
`TableManager::mvcc_store()` is `pub`
(`crates/shamir-engine/src/table/table_manager.rs:1290`). It is ALREADY used
for exactly this purpose in sorted-index backfill
(`crates/shamir-engine/src/table/table_manager_sorted_index.rs:286`). Remove
OQ1 from §6, and update §2.2's "What the pinned version is" subsection to
cite this existing accessor instead of describing it as a gap.

### 2. §2.2 — `snapshot_stream` is a parameterization, not new machinery

The RFC calls `snapshot_stream(batch_size, at_version)` "the single largest
new piece of engine machinery this RFC proposes". False:
`MvccStore::current_stream_impl`
(`crates/shamir-tx/src/mvcc_store/mod.rs:1347-1372`) is ALREADY
version-pinned — it captures `floor = self.gate.last_committed()` at
stream-open (`:1356`), filters "newest version ≤ floor" in its group-by
state machine, and merges the overlay via `self.overlay.snapshot_le(floor)`
(`:1367-1372`). The only change needed is exposing `floor` as a parameter
instead of hardcoding it to `last_committed()` — `current_stream(batch)`
becomes a thin wrapper calling `snapshot_stream(batch,
self.gate.last_committed())`. Rewrite §2.2 to describe this as a small
parameterization (tens of lines), not new machinery, and correct any
"largest new piece" framing elsewhere in the document (check §5's slice-1
scope list too).

### 3. Missing entirely — Phase A needs a SnapshotGuard against MVCC GC

The RFC has NO mechanism protecting the pinned scan version from garbage
collection. Phase A runs for potentially minutes without a barrier. GC uses
`min_alive()` (`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:286`, doc: "oldest
live snapshot, or `last_committed` when no snapshot is open") — without a
registered snapshot, `min_alive` tracks the moving watermark and can collect
versions Phase A's scan still needs, mid-scan. The existing primitive:
`RepoTxGate::open_snapshot()` returns a RAII `SnapshotGuard`
(`crates/shamir-tx/src/repo_tx_gate.rs:356`, type exported at
`crates/shamir-tx/src/lib.rs:74`) that registers a version in
`active_snapshots`, keeping it alive for GC purposes until the guard drops.

Add this to §2.2 (Phase A must acquire and hold a `SnapshotGuard` for the
version being pinned, for the ENTIRE duration of the scan — released only
when Phase A completes or the build aborts), to §3's Claim 2 (the
correctness argument needs this guard to hold, otherwise "every write before
the pin is captured by the scan" is false if GC has already removed the
needed version), and to §4 (crash recovery: what happens if the guard is
lost mid-scan due to a crash — answer should be straightforward given §4.2's
already-conservative restart-from-scratch policy, but state it explicitly:
a crash drops the guard along with everything else, and restart re-pins a
FRESH version, so this isn't a new hazard beyond what §4.2 already handles —
confirm this reasoning holds or flag if it doesn't).

### 4. §2.3 (Phase B) rewrite — no "live write-hook" exists; two paths converge on one choke point

The RFC's Phase B is built around "a live write-hook activated when the
index is registered at `Building`". This abstraction doesn't exist as
described. The actual mechanism (per the audit, see above): tx-staged writes
(`table_manager_tx_ops.rs`) and non-tx CRUD writes
(`table_manager_crud.rs`) both call into `IndexManager::plan_record_created`/
`plan_record_updated`/`plan_record_deleted` (and `SortedIndexManager`'s
equivalents), which iterate every registered def with no state filter and
write directly. Rewrite §2.3 to describe capture as an
`IndexState`-conditional inside THESE shared planning methods — when a def
is `Building` AND has an active in-flight-build registry entry (§2.3's new
in-flight-build registry proposal, kept from the original design), the
method routes to dirty-set capture instead of producing a direct
`SetPosting`/`RemovePosting` for that specific def, while still producing
normal ops for any OTHER `Ready` defs on the same write. This is a single
choke point, not ~15 scattered call sites — say so explicitly, citing the
audit's reasoning.

## Operator's dirty-set decision — already made, thread it through

Per an operator decision on 2026-08-09, THIS RFC does NOT use the original
§2.3's CDC log design (`(build_id, seq)` → `(RecordId, DeltaOp)` with values,
monotonic `seq`, `last_applied_seq` tracking). Instead: **dirty-set** — only
the set of touched `RecordId`s is recorded (no values). Phase C re-reads
each id at the current version and recomputes its posting directly.
Idempotency and last-write-wins fall out by construction (recompute-from-
current-state is inherently idempotent); no `seq`/`last_applied_seq`
bookkeeping is needed. Storage cost is O(distinct rows touched) instead of
O(writes × value size).

Rewrite, consistently:
- §2.3 (mechanism — see point 4 above, now describing dirty-set capture
  instead of a CDC log)
- §2.4 (Phase C — describe draining the dirty-set, re-reading each id at
  current version via the SAME planning methods used for a live `Ready`
  index — this is NOT new posting-maintenance logic, same idea the original
  RFC already argued for its CDC-replay approach, just simpler)
- §3 Claim 2 (the concurrency argument — idempotent recompute-from-current-
  state replaces the original "last-write-wins by seq" argument; the
  argument becomes simpler, not weaker — say so)
- §4.2 (crash recovery matrix — the Phase-C-crash row's tradeoff changes:
  resumable catch-up becomes cheaper/near-free with a dirty-set (no seq
  bookkeeping to reconcile), but the CDC log's per-op replay detail is lost.
  State this tradeoff explicitly as an accepted, deliberate choice, not an
  oversight — slice 1 still ships conservative restart-from-scratch
  regardless per the existing §4.2 policy, so this tradeoff mostly affects a
  FUTURE optimization, not slice 1's correctness surface)

## Explicitly out of scope for this revision — leave as open questions in v2

- §2.4/§6.2 exact convergence-criterion thresholds — ship a fixed hard
  iteration cap for slice 1, tune later (already the RFC's own stated
  fallback).
- §5.2 unique family, §5.3 sorted family, §5.4 index2 family — all remain
  deferred slices, unchanged.
- §6.6-equivalent `DdlOpState` `BuildPhase` progress extension — its own
  later follow-up, unchanged.

## Definition of done

- All 4 corrections applied in place, each citing the exact file:line
  evidence given above (or your own re-verification of it — re-check each
  citation is still accurate against current HEAD before using it, don't
  copy blind).
- Dirty-set threaded consistently through §2.3/§2.4/§3/§4.2 — no leftover
  CDC-log/seq language contradicting the dirty-set design.
- A short revision-log near the top of the document.
- Document remains a DRAFT pending operator review — do not change its
  status to approved, and do not start any implementation.

## Gate before you report done

```
git status --short
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Confirm `git status --short` shows ONLY the RFC file changed — no
production code.

Report: a summary of exactly what changed in each of the 4 correction areas
plus the dirty-set threading, and confirmation the gate commands were run
(even though they should be no-ops for a docs-only change).
