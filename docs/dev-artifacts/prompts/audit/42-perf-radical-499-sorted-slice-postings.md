Task: PERF-RADICAL-3.2 — posting-list representation, `BTreeSet<RecordId>`
→ sorted-slice. Task #499 (deferred from task #488's own scope, from
`docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md` finding 3.2, rated
"средняя" complexity but structural — touches the canonical
representation of every non-vector index's posting lists).

## Context

`crates/shamir-index/src/legacy/index_manager.rs` (~lines 626-671,
confirm current lines) and `sorted_index_manager.rs`'s `lookup_range`
currently return/intersect `BTreeSet<RecordId>` for every non-vector
index lookup. Postings are read from an ALREADY-sorted prefix scan (the
underlying storage iteration is ordered), so building a `BTreeSet` is
pure overhead: one heap allocation per node, cache-unfriendly pointer
chasing on traversal, and O(n log m) intersection with cache misses at
every comparison — when the data is already available in sorted order
and a simple linear/galloping merge over two sorted SLICES would be
both algorithmically better and far more cache-friendly.

Task #488 already fixed the ADJACENT finding (avoiding a full clone on
cache-hit, `Arc<BTreeSet<RecordId>>` instead of a cloned `BTreeSet`) —
this task is about the REPRESENTATION itself: replacing `BTreeSet` with
`Arc<[RecordId]>` (a sorted, immutable slice) as the canonical posting
form.

## Investigation required BEFORE any implementation

This is exactly the kind of finding this campaign has repeatedly found
needs a design/investigation pass before committing to an
implementation (see the KeyBytes precedent, task #491's design doc at
`docs/dev-artifacts/design/record-key-128-migration-plan.md`, and its 5-step
sequenced migration). Do NOT jump straight to implementation. Instead:

1. Map every current CONSUMER of the posting-list `BTreeSet<RecordId>`
   return type — every call site of `lookup_range` /
   `IndexManager::lookup_by_index` / whatever the actual posting-fetch
   entry points are (grep the workspace). For each, determine: does it
   need SET semantics (membership test, arbitrary insert/remove) or
   purely ORDERED-ITERATION semantics (iterate in sorted order, merge
   with another sorted sequence, intersect, union)? A slice-based
   representation is a clean win for the latter but may need adaptation
   for the former.
2. Confirm the posting-building path (index creation / incremental
   backfill from task #490, and the live write-hook that updates
   postings on insert/update/delete) can produce a SORTED slice
   naturally, or whether it currently relies on `BTreeSet`'s automatic
   ordering-on-insert — if so, a slice-based representation needs an
   explicit sort step somewhere (at write time? at read time via a
   sorted merge of a "base" sorted slice + a small "delta" set of
   recent unsorted inserts?). Investigate the actual insert/update/
   delete code path for postings before assuming this is a pure read-
   side change.
3. Design the intersection/union algorithm: a galloping/merge-based
   intersection over two sorted `&[RecordId]` slices (standard
   technique: for two sorted sequences, walk both with a merge-style
   two-pointer scan, or exponential/galloping search for the more
   selective side when sizes differ greatly). Investigate whether this
   codebase already has a similar merge primitive elsewhere (grep for
   existing sorted-merge utilities) before writing a new one from
   scratch.
4. Determine whether `RecordId`'s `Ord` (used for sorting) matches
   whatever ordering the underlying storage scan naturally produces —
   confirm this is consistent (the audit brief for the KeyBytes
   migration, task #491, notes `RecordId`'s BE-timestamp prefix makes
   lexicographic byte order == chronological order; confirm sorted-slice
   posting order is intended to match this, or whatever the actual
   correct posting order is for existing query semantics like ORDER BY
   over an indexed field).

## Scope-down guidance (use liberally — this is exactly the kind of finding built for it)

If the full representation migration (touching every posting consumer,
the write-hook, AND the intersection/union algorithm) is too large for
one surgical pass, sequence it like the KeyBytes migration was
sequenced (task #491 → #503-506):

1. **Step 1 (safe, always do this)**: write a design doc at
   `docs/dev-artifacts/design/posting-list-sorted-slice-migration-plan.md` covering
   the investigation above — current state with file:line citations,
   feasibility verdict, proposed type design (`Arc<[RecordId]>` or a
   custom newtype wrapping it with the needed trait impls), a
   migration sequence of small independently-committable steps, and
   landmines (e.g. correctness risk if intersection logic has an
   off-by-one or doesn't handle duplicate/empty inputs correctly).
2. **Step 2 (if step 1's investigation shows a clean, low-risk path
   exists)**: implement JUST the READ-SIDE representation change for
   the highest-value, lowest-risk consumer (e.g. equality-lookup +
   two-way AND intersection, which the audit's own bench gap note
   flags as needing coverage at |postings| ≥ 10k) — leaving other
   consumers (union, NOT, multi-way AND) on the old `BTreeSet` path if
   that keeps the change surgical, via an explicit adapter/conversion
   at the boundary. Document any performance cost of that adapter
   (e.g. a one-time sort-and-collect at the boundary) honestly.
3. If step 1's investigation reveals the write-hook / incremental
   backfill path fundamentally can't easily produce sorted output (e.g.
   deep coupling with existing crash-safety guarantees from earlier
   tasks in this campaign, similar to the interner's `entries_after`
   coupling that caused task #501 to be deferred), STOP after step 1
   and file the implementation as follow-up work with the design doc as
   its foundation — do not force a risky change into existing
   crash-safety-critical write paths.

## Performance verification requirement (MANDATORY if any implementation lands — this is a PERF task)

Per this repo's `/opti` methodology: the audit explicitly notes NO bench
exists for equality-lookup at |postings| ≥ 10k (a stated bench-coverage
gap). If you implement step 2, first ADD this bench (following the
`bench-scale-tool::Harness` convention used elsewhere in this repo —
`crates/shamir-index/benches/` has several reference examples), measure
baseline (`BTreeSet` intersection) vs. after (sorted-slice merge) at
|postings| ≥ 10k, and report honest before/after numbers with the
speedup ratio — or a flat/no-improvement result with root-cause
explanation, per this campaign's established precedent.

## TDD/regression requirement

If any implementation lands: add a regression test confirming
intersection/union results are IDENTICAL between the old `BTreeSet`
path and the new sorted-slice path for the same inputs (including edge
cases: empty postings, single-element postings, duplicate RecordIds if
that's even possible, fully-disjoint sets, fully-overlapping sets).

## Test scope

```
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

## Gate

```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Investigation] Status: complete
  > Design doc written at docs/dev-artifacts/design/posting-list-sorted-slice-migration-plan.md
  > Feasibility verdict: <safe to implement step 2 / needs deferral, and why>

[Implementation] Status: fixed / partially-fixed / deferred entirely
  > What changed + regression test added (if fixed/partial)
  > Bench: baseline vs after at |postings| >= 10k (if fixed)
  > OR: deferral reason (if deferred) — follow-up task descriptions for
    the remaining migration steps from the design doc
```
Full test/gate/bench results (exact commands + pass/fail) for whichever
crates were actually touched.
