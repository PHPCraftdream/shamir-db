# F-74 (#901) — bump tx sorted epoch before posting apply + fix inverted safety comment

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Two defects, both in `crates/shamir-engine/src/tx/commit_phases.rs`'s
`apply_index_batch` (currently around lines 585-618)

### 1. Ordering — apply-before-bump leaves a TOCTOU residual

The tx-commit path's sequence is:

```
apply_index_ops_at_commit(...).await   // mutates postings
invalidate_posting_cache_for_ops(...)  // cache-only
tbl.sorted_indexes().bump_touched_indexes(ops, commit_version)  // raises epoch
```

Postings are mutated BEFORE the epoch (`SortedIndexManager::
last_mutation_version`) is raised. Between the apply and the bump, another
OS thread can pass F-58's entry gate (`last_mutation_version(idx) <=
pinned`), scan the index (whose posting has ALREADY been removed/moved by
the apply that just ran), execute F-58's POST-check before the bump lands
(the post-check reads the still-not-yet-bumped epoch and sees no problem),
and return an incomplete AsOf page.

`crates/shamir-engine/src/table/read_asof_seek.rs`'s module doc (see its
"Bump-vs-apply ordering" section) already documents this precisely and
explicitly accepted it as an out-of-scope residual for F-58/F-67, reasoning
that "there is no `.await` between the two calls." That reasoning is
insufficient: absence of an `.await` only prevents the SAME tokio task
from yielding mid-window — it creates no atomicity across OS threads
(a genuinely concurrent task on another worker thread can run its own
gate-check → scan → post-check sequence entirely within that window,
tokio being a multi-threaded work-stealing runtime by default in this
workspace). **This task closes that residual — update
`read_asof_seek.rs`'s module doc to reflect that it is no longer accepted
as out-of-scope; state precisely what changed.**

The non-tx direct path is the reference for the correct order:
`SortedIndexManager::on_record_created`/`on_record_updated`/
`on_record_deleted` (`crates/shamir-index/src/legacy/sorted_index_manager.rs`)
call `bump_touched_indexes` BEFORE `apply_ops`. That order is
conservative-safe: if the bump fires before the apply lands, a concurrent
scan might unnecessarily fall back to a full scan (always correct, just
slower) — never the reverse (a scan proceeding on the fast path against an
index whose postings just changed).

**Fix (minimum viable slice, per both readonly reviews):** bump the
touched sorted indexes BEFORE `apply_index_ops_at_commit` runs, mirroring
the non-tx path's order. If the apply then fails partway through, the
epoch is already raised — a spuriously-early fallback for anyone who reads
concurrently during the failed apply, which is always safe by
construction (full-scan fallback is never wrong, only conservative).
Do NOT reach for the stricter options review #1 lists (a per-index
seqlock, versioned/immutable snapshots) — those are explicitly out of
scope for this task's minimum slice; note them as a follow-up if you want,
but do not implement them here.

**Practical ordering problem to solve:** `bump_touched_indexes` needs
`ops: &[IndexWriteOp]` to know WHICH indexes were touched — today it's
called with the exact same `ops` that `apply_index_ops_at_commit` just
applied. Moving the bump earlier means computing "which sorted indexes did
`ops` touch" from the SAME `ops` slice BEFORE calling apply — the ops are
already fully planned/available at that point in the function (they're a
parameter), so this is a pure reordering of two already-independent calls
against the same pre-existing `ops` slice, not a new computation. Confirm
this by reading the full function signature and body before touching it.

### 2. Inverted safety comment — a documentation defect, present in TWO places

The comment at `apply_index_batch` (~lines 611-615) claims:

> "An `ops` batch with no sorted-index entries... bumps nothing here — a
> false-negative... only costs a fallback to the already-correct full
> scan, never a correctness bug."

This is backwards. The gate is `epoch <= pinned ⟹ USE the fast path`. A
MISSED bump leaves the epoch LOW, which leaves the gate OPEN — i.e. it
KEEPS the unsafe fast path enabled, it does not force a fallback. Before
F-67 (#893) the bump was UNCONDITIONAL, so the only possible error was
OVER-bumping (safe: closes the gate, forces a fallback). F-67 made the
bump conditional on key decoding (`decode_sorted_index_name` returning
`Some`), introducing the possibility of UNDER-bumping (a decode miss on a
key that actually WAS a sorted-index posting) — the comment's wording was
carried over from the pre-F-67 unconditional-bump era without re-deriving
whether it still held, and it does not.

The identical inverted comment/reasoning appears a second time in
`SortedIndexManager::bump_touched_indexes`'s own doc
(`crates/shamir-index/src/legacy/sorted_index_manager.rs`, currently
around lines 804-811 — grep for "false negatives here only cost a
fallback" to find the exact current location, the line number has shifted
since this task was ticketed because F-71 (#898) added ~100 lines earlier
in the same file). Fix BOTH occurrences — correct the reasoning to state
plainly that a missed bump is a real correctness risk (keeps the gate
open on an index whose postings have changed), not a harmless
degrade-to-full-scan. This is a documentation-only correction (the
`decode_sorted_index_name` skip-on-`None` behavior itself is not being
changed by this task — that decode function's correctness is out of
scope here; only the comment describing the CONSEQUENCE of a miss is
wrong).

## Definition of done

- A deterministic pause-seam (this codebase's established `TEST_*`
  `OnceLock`/hook convention — grep `table/read_asof_seek.rs`'s existing
  F-58 test-only pause seam, and `tx/commit_phases.rs`'s own
  `FAIL_PHASE_5C_TX_ID` for style precedent) placed STRICTLY BETWEEN the
  posting apply and the epoch bump (i.e. exercising the OLD, buggy
  ordering's exact window) — used to prove the fix two ways:
  - **Red-then-green**: with the old apply-then-bump order temporarily
    restored, park a committing tx at the seam (postings already applied,
    epoch not yet bumped), issue a concurrent AsOf read pinned to a
    version before the commit for an UPDATE to the indexed field, and
    confirm it returns a SHORT/WRONG page (the bug). Then apply the real
    fix (bump-then-apply) and confirm the same scenario is no longer
    constructible / the read is now provably safe (either the read waits
    for the bump-before-visible-mutation ordering, or the read's gate is
    now unable to observe an unbumped-but-applied state at all — describe
    which).
  - Repeat for a DELETE (removing an indexed row), not just an UPDATE —
    the module doc explicitly calls out both cases as unservable by a
    current-state index.
- Update `crates/shamir-engine/src/table/read_asof_seek.rs`'s
  "Bump-vs-apply ordering" module-doc section: the tx-commit path now
  matches the non-tx path's order (bump-before-apply), so the residual it
  used to accept as out-of-scope is closed. Say so explicitly, do not
  leave the old "accepted as out-of-scope" language standing once it's no
  longer true.
- Fix both inverted-comment locations (`commit_phases.rs`'s
  `apply_index_batch` and `sorted_index_manager.rs`'s
  `bump_touched_indexes`) with corrected reasoning.
- `cargo fmt -p shamir-engine -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine -p shamir-index --full` green.
- Do not touch F-71 (#898)'s `ready_at_version`/`mark_ready_at` mechanism
  or F-73 (#900)'s error-propagation changes in `pre_commit.rs` — this
  task only reorders two calls and corrects two comments inside
  `apply_index_batch` and its sibling doc in `sorted_index_manager.rs`.
- Do not run this task concurrently with any other task touching
  `commit_phases.rs`, `sorted_index_manager.rs`, or `read_asof_seek.rs`.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
