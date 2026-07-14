Task: MEDIUM performance — three independent findings from
`docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md`:
1. **1.6**: `shamir-funclib`'s `distinct()` array function does O(N²)
   full `PartialEq` comparisons instead of hash-based dedup.
2. **2.1**: `SegmentSet::open` fully replays (reads + decodes EVERY
   entry of) every sealed WAL segment at startup, JUST to compute
   `max_version` — a number that's likely derivable much more cheaply.
3. **2.3**: `Interner`'s CAS-loop clones the ENTIRE reverse-lookup
   `Vec` on every FIRST-touch of a new field name, making cold-start/
   bulk-load with many unique field names O(N²) in slot copies.

These are independent — fix each on its own merits; do not couple them.

## Finding 1.6 — funclib `distinct()` O(N²) (LOW complexity, do this fully)

- `crates/shamir-funclib/src/arrays.rs:145-161` (confirm current lines):
  ```rust
  reg.register("distinct", FnEntry::pure(|a| {
      let arr = arg_list(a, 0)?;
      let mut out: Vec<QueryValue> = Vec::with_capacity(arr.len());
      for e in arr {
          if !out.iter().any(|kept| kept == e) {
              out.push(e.clone());
          }
      }
      Ok(v_list(out))
  }, 1, Some(1)));
  ```
  `out.iter().any(|kept| kept == e)` is a LINEAR scan of everything
  kept so far, for EVERY element — O(N²) total comparisons, and each
  `QueryValue` comparison is itself O(len) for strings/nested
  structures. A 10k-element array = ~50M value comparisons in one
  scalar function call.

### Fix — 1.6

1. Replace the O(N²) linear-scan dedup with a hash-based approach: use
   a `HashSet`/`HashMap` keyed by a canonical hash of `QueryValue` (per
   the audit's fix sketch) so membership-testing is O(1) amortized
   instead of O(kept-so-far).
2. Handle the audit's called-out edge case: NOT every `QueryValue`
   variant may cleanly implement `Hash` (e.g. `F64`/floating-point
   values have the classic NaN/hash-consistency problem). Investigate
   `QueryValue`'s actual variants (check its definition) and:
   - If it already derives/implements `Hash` cleanly for all variants
     (some codebases special-case float bit-patterns for hashing),
     use it directly.
   - If NOT hashable for some variant (e.g. `F64`), the audit suggests
     a fallback via sorting/total-order — investigate whether
     `QueryValue` already has (or could cheaply get) a total-order
     `Ord`/`PartialOrd` impl usable for a sort-then-dedup approach for
     JUST the non-hashable variants, or whether a simpler pragmatic
     fix (e.g., a wrapper newtype implementing `Hash` via the value's
     canonical byte/string representation) is more appropriate given
     this codebase's existing patterns. Use judgment; report your
     choice and why.
3. This function is registered as a "pure" `FnEntry` — confirm the
   fix preserves EXACT output semantics (element order preservation:
   `distinct` should still return elements in FIRST-occurrence order,
   matching the current O(N²) implementation's behavior — a naive
   `HashSet`-based rewrite that doesn't preserve insertion order would
   be a silent behavior regression; use an order-preserving dedup
   pattern, e.g. track a `HashSet` for membership AND push to the
   output `Vec` only on first-sight, which naturally preserves order).

## Finding 2.1 — WAL `SegmentSet::open` full-replay-for-one-number (MED-HIGH complexity — investigate feasibility, may need to scope down)

- `crates/shamir-wal/src/segment_set.rs:131-141` (confirm current
  lines): for every SEALED segment found at startup, `seg.replay()` is
  called — a FULL read+decode of every entry in that segment — purely
  to compute `entries.iter().map(|e| e.commit_version).max()`. Startup
  cost is O(total WAL bytes on disk); after a long downtime with many
  un-truncated sealed segments, opening the DB reads and decodes
  gigabytes just to throw away everything except one number per
  segment.

### Fix — 2.1 (investigate; scope down if genuinely infeasible cleanly)

1. Investigate `WalSegment`'s actual on-disk format (read
   `wal_segment.rs` in full, especially how entries are laid out and
   whether there's any trailer/footer/index structure). The IDEAL fix
   is: read only the LAST entry (or a small trailer recording
   `max_version` written at seal-time) instead of decoding every entry.
   Concretely, investigate:
   - Does sealing a segment (transitioning it from active to sealed)
     have a hook where a small footer/summary (e.g. just the
     `max_version` as a fixed-size trailer) could be written ONCE,
     cheaply, at seal time — so that `open` for an ALREADY-SEALED
     segment can just read that trailer instead of replaying? This
     would be the cleanest fix but requires a FORMAT change (a new
     on-disk structure) — assess whether this is safe to introduce
     given existing sealed segments on disk won't have this trailer
     (need a fallback: if no trailer present, fall back to the
     current full-replay path for backward compatibility with
     segments written before this fix).
   - Alternatively, if the entry format is length-prefixed and
     sequential, can the LAST entry be located by SEEKING (not
     decoding every entry) — e.g. if there's a way to find the start
     of the last record without a forward scan (a segment footer with
     an offset, or entries being fixed-size, or some other structural
     hint)? If the format is a pure forward-append log with no
     index/footer and no way to seek backward without a full scan
     (common for append-only logs), a targeted seek-to-last fix may
     be GENUINELY INFEASIBLE without a format change.
2. **Scope-down escape valve**: if after investigating the actual
   on-disk format, NEITHER a backward-compatible trailer NOR a seek-
   based approach is cleanly achievable within a surgical PERF fix
   (i.e., it would require a WAL FORMAT VERSION BUMP with migration
   logic — a much larger, riskier undertaking touching durability
   guarantees), **STOP this specific fix, do NOT half-implement a
   format change**, and instead: (a) confirm/document in your report
   exactly why a cheap fix isn't feasible given the current format,
   (b) write up what a proper fix would require (format version bump,
   trailer write-on-seal, fallback-replay for legacy segments without
   a trailer) as a follow-up task description, (c) leave 2.1 as pure
   documentation/analysis in this task, and move on to 2.3/1.6 (this
   mirrors the successful pattern already used for #488's 3.2
   deferral in this campaign).
3. If, after investigation, you find a SAFE, low-risk win is actually
   achievable (e.g. a segment's LAST record's `commit_version` can be
   read without decoding the FULL record, or the existing replay
   function has an early-exit/streaming variant that doesn't require
   materializing all entries into a `Vec` before finding the max), that
   partial improvement is also acceptable — report exactly what you
   found and implemented.

## Finding 2.3 — Interner reverse-vec full clone on first-touch (MEDIUM complexity — investigate, likely scope down)

- `crates/shamir-types/src/core/interner/interner.rs:129-141` (and
  `:308-332` in `touch_with_id`, confirm current lines): the CAS-loop
  for registering a brand-new field name does `let mut new_rev =
  (*cur).clone()` — a FULL O(N-slots) copy of the reverse-lookup
  `Vec` on EVERY first-touch of a new name. For schema-rich bulk-load
  (10k+ unique field names, e.g. nested JSON documents with many
  distinct keys), this is O(N²) total slot-copies across the whole
  load, and concurrent inserts multiply this via CAS retries.

### Fix — 2.3 (attempt if tractable; scope down per audit's own "средняя" complexity rating if not)

1. Per the audit's fix sketch: replace the single monolithic
   `ArcSwap<Vec<...>>` with a SEGMENTED spine —
   `ArcSwap<Vec<Arc<[OnceLock<Arc<str>>; 1024]>>>` (or an equivalent
   chunked structure): growing the interner adds a NEW CHUNK (an O(#chunks)
   clone of just the outer pointer-vector, NOT the actual string data),
   and filling a slot within an existing chunk is a write into a
   `OnceLock` — no vector copy at all. Readers need two levels of
   indexing (chunk index + slot-within-chunk index) instead of one
   flat index.
2. This is a genuine data-structure change to a widely-used,
   performance-sensitive shared component (the interner is used by
   EVERY table in a repo). Investigate the FULL blast radius: how many
   call sites read via `reverse_snapshot`/`get_str`/`with_str` (grep
   the workspace) and whether they can be adapted to a two-level index
   without semantic changes.
3. **Scope-down escape valve**: per the audit's own complexity rating
   ("средняя" = medium, higher than 1.6's "низкая" but lower than
   3.2/RecordKey's "высокая"), THIS finding is explicitly a judgment
   call. If, after investigating the actual `Interner`/`InternerKey`
   API surface and its callers, the segmented-spine rework proves too
   large or risky for a single surgical PERF task (e.g., it touches
   the on-disk chunk-persistence format used by `InternerManager` for
   crash recovery — cross-reference this campaign's earlier A8/A11
   fixes to `interner_manager.rs` if relevant, since those fixes
   depend on the CURRENT reverse-vec/chunk-persistence shape), **STOP
   and scope down**: document the finding, the proposed segmented-
   spine design, and the blast radius as a follow-up task description
   in your report, rather than attempting a risky half-migration. Do
   NOT touch anything related to A8/A11's interner-persist-before-WAL-
   truncate invariants while investigating this — if there's ANY risk
   of interacting with those crash-safety guarantees, that's an
   automatic signal to scope down rather than proceed.
4. If a genuinely safe, scoped improvement IS achievable (e.g., simply
   using a `Vec::clone`-avoiding technique that doesn't require the
   full segmented-spine redesign — investigate if there's a simpler
   partial win, like doubling the growth strategy to amortize clones,
   or batching multiple first-touches under one CAS retry instead of
   one-clone-per-name), report what you found and implemented instead.

## Performance verification requirement (MANDATORY for whichever findings are actually fixed — this is a PERF task)

Per this repo's `/opti` methodology and this campaign's established
precedent (tasks #486/#487/#488):
1. For 1.6: bench `distinct()` on a 1k/10k-element array with a
   realistic duplicate ratio, before/after. Expect ~O(N) vs O(N²)
   scaling difference to show clearly even at modest N.
2. For 2.1 (if fixed, even partially): bench `SegmentSet::open` against
   a pre-seeded set of sealed segments of varying total size, before/
   after.
3. For 2.3 (if fixed): bench cold-start interner population with 10k+
   unique field names, before/after.
4. Follow this repo's `bench-scale-tool::Harness` convention (check
   `crates/shamir-funclib/benches/`, `crates/shamir-wal/benches/`,
   `crates/shamir-types/benches/` for existing patterns to match, or
   the newly-added benches from tasks #486-488 for the general shape).
5. Report exact baseline vs. after numbers with speedup ratios. For any
   finding you scope down/defer, there is naturally no bench to run —
   just report the deferral clearly.

## TDD/regression requirement

1. For 1.6: confirm `distinct()`'s output order and correctness (exact
   set of unique values, first-occurrence order preserved) is
   unchanged for a range of inputs (empty, no-dupes, all-dupes, mixed
   types if `QueryValue` is polymorphic, including the F64/NaN edge
   case if relevant).
2. For 2.1 (if fixed): confirm `SegmentSet::open` still correctly
   computes `max_version` for sealed segments written BEFORE and (if
   applicable) AFTER the fix (backward compatibility with existing
   on-disk segments is critical — do not break opening a pre-existing
   database).
3. For 2.3 (if fixed): confirm interner correctness (name→id and
   id→name round-trip) is unchanged; add a concurrency test if the
   segmented-spine changes the CAS-retry behavior in an observable way.

## Test scope command

```
./scripts/test.sh -p shamir-funclib
./scripts/test.sh -p shamir-wal
./scripts/test.sh -p shamir-types
./scripts/test.sh -p shamir-engine
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-funclib -p shamir-wal -p shamir-types -- --check
cargo clippy -p shamir-funclib -p shamir-wal -p shamir-types --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly, for EACH of the three findings:
```
[Finding 1.6] Status: fixed / deferred
  > Baseline / After / Δ (if fixed)
  > OR: deferral reason + follow-up description (if deferred)

[Finding 2.1] Status: fixed / partially-fixed / deferred
  > Baseline / After / Δ (if fixed)
  > OR: deferral reason + follow-up description (if deferred)

[Finding 2.3] Status: fixed / partially-fixed / deferred
  > Baseline / After / Δ (if fixed)
  > OR: deferral reason + follow-up description (if deferred)
```
- Full test/gate results (exact commands + pass/fail) for whichever
  crates were actually touched.
- Confirmation of backward-compatibility for 2.1 if a format change
  was made (old segments must still open correctly).
