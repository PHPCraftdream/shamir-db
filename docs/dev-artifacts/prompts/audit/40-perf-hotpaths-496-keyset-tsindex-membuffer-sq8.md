Task: HIGH-perf residual cluster — 4 independent findings from
`docs/dev-artifacts/audits/2026-07-06-perf-hot-paths.md`. Task #496.

These are INDEPENDENT findings — fix each on its own merits. Per this
campaign's established pattern, if any single finding is genuinely
high-complexity/structural beyond what's tractable here, STOP on that
ONE finding, document your investigation + a follow-up task
description, and continue with the others. Two of these (SQ8 SIMD,
MemBuffer drain-before-scan) are explicitly rated "Средняя" (medium)
complexity by the audit — expect they may need a scope-down.

## Finding 1.2 (HIGH) — Keyset-seek (`Pagination::After`) fetches the entire half-plane

`crates/shamir-engine/src/table/read_index_scan.rs:443-531` (confirm
current lines): `lookup_range(seek_key, +∞)` returns a `BTreeSet<RecordId>`
of ALL records past the seek key (`crates/shamir-index/src/legacy/sorted_index_manager.rs:546-569`
— the stream is collected into a set, losing value-order), then
`get_many_bytes` fetches+decodes EVERYTHING, projects everything, fully
sorts via `apply_order_by_qv`, and only THEN truncates to `limit`. This
is O(remaining table) fetch+decode+project+sort per PAGE — the entire
point of keyset pagination is to avoid exactly this, so deep pagination
degrades to O(N²) over a full scroll.

### Fix — 1.2

1. Add an ordered `lookup_range_first_k(name, lo, hi, k, direction)` API
   to the index manager — the index is ALREADY ordered by value, so this
   should walk the stream in order, skip entries equal to the seek key
   (already-seen boundary), and STOP once `k` results are collected. This
   removes both the full sort and the full fetch.
2. Investigate the existing `lookup_range`/stream infrastructure in
   `sorted_index_manager.rs` before designing a new API from scratch —
   reuse the existing ordered-stream machinery if it already preserves
   value order internally (the audit notes value-order is LOST when
   collected into a `BTreeSet`; if the underlying stream itself is
   already ordered, the fix may be as simple as NOT collecting into a
   set and consuming the stream directly with an early-stop).
3. Wire this into `read_index_scan.rs`'s keyset-seek path, replacing the
   full-range-then-sort-then-truncate sequence.
4. Add a regression test proving keyset pagination over a large table
   with a small `limit` does NOT fetch/decode the whole remaining table
   — e.g. instrument or count actual `get_many_bytes` calls / decoded
   record count and assert it's bounded by roughly `limit`, not
   `table_size - offset`.
5. Add a bench (the audit notes NO bench exists for keyset-seek at all)
   following this repo's `bench-scale-tool::Harness` convention — measure
   page-fetch time at a fixed `limit` as table size grows; the fix should
   show roughly FLAT per-page cost instead of growing with table size.

## Findings 2.1 + 2.2 (HIGH) — MvccStore's `ts_index` and `cells` grow unbounded

`crates/shamir-tx/src/mvcc_store/mod.rs:163` (`ts_index`, confirm line)
and `:118` (`cells`, confirm line): both are populated on every committed
version / every touched key respectively, and NEITHER has any eviction
path anywhere in the crate (the audit confirms via grep: only
insert/query/rebuild for `ts_index`, no `cells.remove`/`retain`
anywhere). Vacuum/purge already clean `history` but not these two
structures. At sustained write load this is unbounded memory growth
(gigabytes/month at 5k writes/s for `ts_index`; ~100B/key forever for
`cells` under queue-like insert+delete workloads).

### Fix — 2.1 + 2.2

1. For `ts_index` (2.1): add pruning to the existing
   `gc_overlay_to`/vacuum/purge path — a range-remove by `ts <=
   purge_watermark` (the keys are already `Reverse`-ordered, so this is
   a tail-range removal, not a scan). Investigate the EXACT existing
   vacuum/purge call sites in this crate (grep for `gc_overlay_to`,
   `vacuum`, `purge`) to find where history cleanup already happens and
   hook the SAME watermark into `ts_index` pruning.
2. For `cells` (2.2): remove a `RecordCell` when its version is
   tombstoned (version <= durable AND no live snapshot references it) —
   during the SAME vacuum pass. Verify `seek_latest_version`'s cold-start
   path correctly handles an ABSENT cell (the audit notes this should
   already be handled correctly, confirm by reading the code, don't
   assume).
3. These two are independent structures but likely share the SAME vacuum
   trigger point — investigate whether they should be pruned together in
   one pass (for locality) or can genuinely be separate; choose whichever
   is cleaner given the actual code structure.
4. Add regression tests: after a vacuum/purge cycle past some watermark,
   confirm `ts_index`'s size (or an iteration count) reflects only
   live-relevant entries, not the full historical count. Same for
   `cells`. This needs the actual crash-safety/correctness invariants
   preserved — a cell/ts_index entry must NEVER be pruned while it's
   still needed by a live snapshot/reader; investigate concurrency
   safety carefully here (this touches the exact machinery earlier A8/A9
   tasks in this campaign already hardened — read that surrounding code
   before touching it, and if you find any interaction risk with those
   invariants, treat this as the scope-down trigger and defer with a
   documented follow-up rather than risk a correctness regression in
   MVCC/SSI machinery).

## Finding "5" (MED-complexity, Топ-5 table) — MemBuffer drain-before-scan write amplification

`crates/shamir-storage/src/storage_membuffer.rs:521-605` (confirm lines):
every scan operation currently does a full `drain_once` BEFORE scanning,
to ensure the scan sees dirty writes. Under sustained concurrent writes,
this means every scan pays a write-amplification cost proportional to
however much is currently dirty — this is a "read-triggered write
amplification" that degrades scan p99 latency under write load.

### Fix — 5 (investigate; scope down per audit's own "Средняя" complexity rating if not cleanly achievable)

1. Investigate the audit's suggested direction: a merge-overlay approach
   instead of drain-before-scan — i.e., the scan reads BOTH the
   already-flushed inner store AND the still-dirty in-memory buffer,
   merging them on the fly (matching MVCC-style overlay reads elsewhere
   in this codebase — check `versioned_overlay.rs` for the existing
   pattern this campaign has already used, since finding 1.4 in the same
   audit references it), rather than forcing a flush before every scan.
2. This must preserve the EXISTING fsync-batching behavior (per the
   audit's own "сохранение fsync-батчинга" note) — do not change when/how
   background flush batches trigger, only how SCANS observe dirty state.
3. **Scope-down escape valve**: if a merge-overlay redesign of the scan
   path requires touching more of `storage_membuffer.rs`'s internal
   state machine than is safe for a single surgical pass (this campaign's
   task #494 already touched this file for findings 2.2/2.3 — read that
   recent history first via `git log -p` on this file to avoid
   reintroducing a bug those fixes closed), STOP and document your
   investigation + a follow-up task description for the merge-overlay
   redesign, per this campaign's established scope-down pattern.
4. If fixed: add a regression test AND a bench showing stable scan p99
   under concurrent write load (before/after), following the
   `bench-scale-tool::Harness` convention used elsewhere in this repo.

## Finding "4" (MED-complexity, Топ-5 table) — Fused SQ8-rescore + SIMD kernels

`crates/shamir-index/src/vector/{quantized_dist.rs,sq8.rs}`,
`hnsw_adapter.rs:1642-1664` (confirm lines): the audit identifies three
independent wins here: (a) fusing the SQ8 rescore step to avoid
redundant work, (b) SIMD-accelerated weighted-distance kernels (the
audit notes this repo already has SIMD kernel samples in `simd.rs` —
follow that existing style/pattern rather than inventing a new SIMD
approach), (c) precomputing `scales_sq` instead of recomputing it
per-query/per-candidate.

### Fix — 4 (investigate; scope down per audit's own "Средняя" complexity rating if not cleanly achievable)

1. Read the existing `simd.rs` kernel patterns in this codebase FIRST —
   match its style (portable_simd vs. explicit target-feature intrinsics,
   whatever this repo already uses) rather than introducing a new SIMD
   approach.
2. Precompute `scales_sq` once (wherever it's currently recomputed
   redundantly) — this alone (item c) may be a low-risk, high-value
   first sub-fix even if the full SIMD kernel work is scoped down.
3. Investigate the "fused rescore" opportunity — what specifically is
   redundant between the SQ8 approximate distance and the rescore step;
   read `hnsw_adapter.rs:1642-1664`'s actual current code to understand
   the exact redundancy before designing a fix.
4. **Scope-down escape valve**: this is explicitly one of the more
   invasive findings (SIMD kernel work, "ядра по образцам simd.rs" implies
   real numerical/SIMD implementation work) — if genuine SIMD kernel
   authoring is too large for this pass, implement item 2 (precompute
   `scales_sq`) as a safe, narrow win, and defer items 1/3 with a
   documented follow-up task for the full fused-SIMD rescore redesign.
5. Follow this repo's `/opti` methodology: baseline bench BEFORE, the
   change, bench AFTER, honest reporting of the actual speedup (or lack
   thereof) — this campaign has a strong precedent of reporting flat/
   no-improvement results honestly rather than fabricating numbers.

## TDD/regression requirement

For EACH finding you fix: add a regression test that would FAIL without
the fix, per the specifics above. For any finding involving MVCC/vacuum
machinery (2.1/2.2), be EXTRA careful about correctness under concurrent
readers — this campaign has already hardened crash-safety invariants in
this exact area (A8/A9/A10 in the concurrency-isolation cluster,
task #481) and a careless prune could silently break a live snapshot
read. When in doubt, favor the scope-down escape valve over a risky
change to this machinery.

## Test scope

```
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-storage
./scripts/test.sh -p shamir-index
```

## Gate

```
cargo fmt -p shamir-engine -p shamir-tx -p shamir-storage -p shamir-index -- --check
cargo clippy -p shamir-engine -p shamir-tx -p shamir-storage -p shamir-index --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Performance verification requirement (MANDATORY — this is a PERF task)

Per this repo's `/opti` methodology and this campaign's established
precedent: for EACH finding you fix, report exact baseline vs. after
numbers with speedup ratios, honestly — if a fix doesn't show the
expected improvement, report that too with a root-cause explanation
rather than fabricating a clean win. Use
`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p <crate>
--bench <name>` per this repo's bench cache isolation rule.

## Report format

For EACH of the findings (1.2, 2.1, 2.2, MemBuffer-drain, SQ8-SIMD):
```
[Finding X] Status: fixed / partially-fixed / deferred
  > Baseline / After / Δ (if fixed, with bench numbers)
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/gate results (exact commands + pass/fail) for whichever crates
were actually touched.
