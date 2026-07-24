# Post-Wave-D Release-Readiness Review — 2026-07-24 (@fh, independent)

Independent, skeptical review of the five Wave D fixes (`3e85e976..HEAD`,
tasks #782–#786: CR-D1..CR-D5) against their briefs
(`docs/dev-artifacts/prompts/post-alpha/28-*.md` … `32-*.md`), plus a
broader release-readiness pass over the cursor / byte-budget / restore
code that Wave D touched. Nothing was assumed correct because a prior wave
reviewed it; every claim below was re-derived from the current working
tree (the CR-D3 counterexample was additionally confirmed by compiling and
running the shipped algorithm standalone).

Severity legend: **HIGH** = fix before public release; **MED** = fix
before release or document loudly; **LOW** = polish / follow-up.

---

## Summary table

| ID | Sev | Area | Finding |
|----|-----|------|---------|
| W-1 | HIGH | numbers / correctness | **CR-D3's `cmp_i64_f64` is WRONG for every negative non-integer `f64`**: `fract()` in Rust is trunc-based (negative for negative inputs), so the Equal-tie-break `f.fract() > 0.0` never fires for negative `f` — `Int(-1)` compares **Equal** to `F64(-0.5)` (and to every f64 in `(-1, 0)`); `Int(-5)` Equal to every f64 in `(-5, -4)`. A regression vs. the pre-D3 code, which handled these values correctly |
| W-2 | MED | cursors / data loss | Keyset cursor over a uniformly-typed `Bin` (or `List`) ORDER BY column silently loses every row after page 1 — same worst-shape failure as N-2, NOT caught by CR-D2's null probe and NOT named in the CR-D2 disclosure (which lists only mixed-type and `NaN` as residual) |
| W-3 | MED | cursors / hard error | Keyset cursor over a `Dec`- or `Big`-valued ORDER BY column: page 1 succeeds, then **every** `FetchNext` hard-errors ("keyset seek key has no comparable filter form") forever — `create_cursor` builds the seek bookmark without checking `query_value_to_filter_value` convertibility, falsifying `fetch_keyset_page`'s "infallible in practice" comment. Realistic trigger: `u64 > i64::MAX` ids promote to `Big` |
| W-4 | LOW | RI-15 / cursors | CR-D2's null probe runs BEFORE `reserve_page_budget_upfront`, so its full-scan read (which materializes the whole null-matching set before `LIMIT 1` applies) escapes the RI-15 admission gate that the first-page read is subject to |
| W-5 | LOW | restore | Step-5 FIRST rename failure (`data_dir → backup_sibling`) orphans the staged `temp_dir` with no pointer in the error — the exact N-6 class, unaddressed for this one path; plus a contrived same-second concurrent-restore TOCTOU on the shared `temp_dir` name |
| W-6 | LOW | cursors / wire codes | CR-D5's authorize-before-resolve reorder in `fetch_next` is NOT perfectly behavior-neutral: a db dropped mid-scroll now yields `access_denied` (fail-closed `resource_meta`) instead of `unknown_db` for a non-admin actor — arguably better, but it contradicts the brief's "pure reordering with no behavior change" claim and no test pins either code |
| W-7 | LOW | docs / contract | CR-D1's offset fallback over a mixed-type/`NaN` ORDER BY column (the documented-open CR-D2 residual) can additionally **duplicate** rows, not just lose them — `state.offset` no longer matches the global-sorted prefix once keyset pages skipped incomparable rows; the disclosure mentions loss only |

---

## 1. W-1 (HIGH) — CR-D3's exact `Int`↔`F64` comparator is wrong for all negative fractional floats

`crates/shamir-engine/src/query/filter/resolve.rs` (`cmp_i64_f64`, the
`Ordering::Equal` arm — the `if f.fract() > 0.0` check, ~line 156) and its
mirror in `crates/shamir-engine/src/query/read/order.rs` (~line 230).

The algorithm compares `i` against `f.floor() as i64` and, on Equal, uses
`f.fract() > 0.0` to decide "f has a fractional part, so `f > floor(f) == i`,
therefore `Less`". That reasoning is floor-based, but **Rust's
`f64::fract()` is trunc-based** (`self - self.trunc()`, "has the same sign
as self" per the std docs). For any negative non-integer `f`,
`f.fract()` is **negative**, the `> 0.0` check is false, and the function
returns `Equal` where the true answer is `Less`. The shipped doc comment
even asserts the false premise in writing: "the sign of `f.fract()` (0 vs
positive — f is finite and `f >= f.floor()` always)".

Confirmed by compiling and running the exact shipped function:

```
cmp(-1, -0.5)  = Some(Equal)   (true: Less)
cmp(-5, -4.5)  = Some(Equal)   (true: Less)
cmp(-5, -4.01) = Some(Equal)   (true: Less)
fract(-4.5)    = -0.5
```

**Failure domain**: every pair `(i, f)` with `f` negative, non-integer,
and `i == floor(f)` — i.e. **any negative float compared against the
integer immediately below it**. Not an exotic boundary: temperatures,
deltas, account balances, coordinates. Consequences per operator:
`Eq` wrongly true (`WHERE int_col = -0.5` matches `-1`), `Ne` wrongly
false, `Gte` wrongly true, `Lte` wrongly true, `Lt`/`Gt` wrongly false —
five of six comparison ops give wrong answers on these pairs. In
`ORDER BY`, `I64(-1)` ties with every `F64` in `(-1, 0)`, producing a
non-transitive equivalence (`-1 ≈ -0.5`, `-1 ≈ -0.9`, but `-0.9 < -0.5`)
— a strict-weak-ordering violation the std sort tolerates without panic
but resolves to an arbitrary interleave.

**This is a REGRESSION**: the pre-D3 code (`(*a as f64).partial_cmp(b)`)
handled every one of these values correctly (`|i| < 2^53` casts are
exact). CR-D3 fixed the rare large-magnitude collapse and broke the
common negative-fraction case. The brief itself contained the flawed
`fract() > 0.0` line and explicitly warned "double-check this derivation
yourself … if you find a flaw, fix the algorithm, don't just paste this";
the check was pasted as-is, and none of the new tests
(`dec_cross_type_tests.rs`, CR-D3 block) covers a negative fractional
operand — `compare_values_int_f64_fractional_tie_break` tests `5` vs
`5.5`/`4.5` only.

**Fix (one line, both sites)**: replace `f.fract() > 0.0` with
`f > f_floor` (or `f.fract() != 0.0`) — `f > floor(f)` is the actual
floor-based condition the surrounding derivation already argues for.
Correct the doc comment's `fract` claim. Add tests: `(-1, -0.5)`,
`(-5, -4.5)`, `(-5, -4.99)`, `(i64::MIN, i64::MIN as f64 + 0.5)`-class
values, and the `QvSortKey` mirror. *(Release blocker — this ships wrong
answers for everyday values, worse than the bug it fixed.)*

---

## 2. W-2 (MED) — Keyset cursor over a `Bin`/`List` ORDER BY column: silent row loss CR-D2 does not catch or disclose

`crates/shamir-engine/src/query/filter/resolve.rs::compare_values` has
**no `Bin↔Bin` (or `List↔List`) arm** — both fall to the `_ => None`
catch-all (~line 273). `query_value_to_filter_value`
(`crates/shamir-query-types/src/filter/filter_value.rs:213-229`) DOES
convert `Bin → FilterValue::Binary` and `List → Array`, so the keyset
machinery happily builds a `Gte`/`Lte` boundary filter from a `Bin` seek
key — which then evaluates to false for **every** row (unresolvable
comparison). Walk it: `ORDER BY bin_col` is keyset-eligible by shape
(`pagination_mode_for_query`), the CR-D2 null probe finds no nulls
(column is uniformly non-null `Bin`), page 1 returns fine (sort maps
`Bin → QvSortKey::Other`, `order.rs:271` — all ties, insertion order),
the bookmark is built (`create_cursor`,
`cursor_handlers.rs:1098-1123`), and the first `FetchNext`'s boundary
fetch returns **zero rows** → `data_exhausted` → empty page,
`has_more: false`. Every remaining row silently vanishes — the exact N-2
failure shape (no error, clean `has_more: false`), on a **uniformly-typed,
non-null** column, which the N-2/CR-D2 analysis treated as safe.

Neither `KNOWN_LIMITATIONS.md` §6 nor `CURSORS.md` §1.1 covers this: both
enumerate the residual gaps as exactly "mixed-type" and "`NaN`" — the
disclosure pass Wave D shipped is incomplete for this class.

**Fix**: cheapest correct move is at bookmark-build time — treat a seek
value whose type has no total order in `compare_values` (Bin, List) as
"can't build a keyset bookmark": set `seek_key = None` (routing every
`FetchNext` through the existing offset arm, which is exactly correct for
an all-ties sort), or better, make `pagination_mode_for_query`-adjacent
logic consult the first page's key type. At minimum, add the Bin/List
bullet to both docs and a pinning test (mirror
`nan_order_by_value_is_a_documented_still_open_limitation`). Not
Wave-D-introduced, but Wave D's own disclosure work is what makes the
omission visible-and-wrong now.

---

## 3. W-3 (MED) — Keyset cursor over a `Dec`/`Big` ORDER BY column: permanent hard error after page 1

`create_cursor` (`cursor_handlers.rs:1098-1123`) and `bookmark_from_tail`
(`cursor_handlers.rs:612-635`) set `seek_key = Some(last_value)` from
`order_by_field_value` **without ever checking
`query_value_to_filter_value(&last_value).is_some()`**. For a `Dec`- or
`Big`-valued ORDER BY column (both fully sortable — `QvSortKey::Dec`/
`Big`, `order.rs:268-269` — so page 1 sorts and returns normally), the
conversion returns `None` (`filter_value.rs:226-228`: "Map, Set, Dec,
Big — no direct FilterValue equivalent"). Every subsequent `FetchNext`
then hits `fetch_keyset_page`'s guard (`cursor_handlers.rs:691-697`) and
returns the hard error `"cursor: keyset seek key has no comparable filter
form"` — forever (the error path leaves `state` untouched, so retries are
byte-identical). The guard's own comment — "callers only reach this
function with a `seek_key` that already produced one at bookmark-build
time … infallible in practice" — is **false**: bookmark build never
performs that check. `same_boundary_value`'s doc comment
(`cursor_handlers.rs:534-540`) makes the same wrong claim
("`query_value_to_filter_value` would already have returned `None` in
`boundary_filter`, falling back to the offset bookmark" — no such
fallback exists on that path).

**Reachability is realistic**: any `u64 > i64::MAX` id (snowflake-style)
promotes to `Big` per the FG-6 numeric contract; `Dec` is the recommended
money type. Failure is loud (an error, not silent loss) but the cursor is
permanently unusable and rows past page 1 are unreachable through it.

**Fix**: at both bookmark-build sites, only set `Some(seek)` when
`query_value_to_filter_value(&last_value).is_some()`; otherwise `(None,
0)` — the existing `(Keyset, None)` dispatch arm already serves exactly
this case correctly via the offset bookmark (`state.offset` is maintained
on every branch). One test with a `Dec` ORDER BY column draining
end-to-end. Pre-existing (not Wave-D-introduced); found via the question-7
broadening pass.

---

## 4. Wave D fixes verified CORRECT (checked, not silence)

- **CR-D1 (livelock fix) — correct.** Re-derived the full arithmetic
  against the current code: `StuckAtCeiling` fires iff
  `internal_limit == ceiling ∧ usable_len == 0 ∧ !data_exhausted` —
  exactly the zero-progress fixpoint and nothing else (the
  ceiling-with-progress and data-exhausted branches are unchanged and
  correct; `tie_skip ≤ C` is invariant, so the `ties_seen < tie_skip →
  skip_count = 0` reset cannot mask a stuck state into a duplicate-page
  state). The offset fallback is exactly-once-correct for every column
  type the keyset path itself is correct for: keyset-returned rows form an
  exact prefix of the pinned snapshot's global sorted order (stable sort +
  inclusive boundary + tie_skip accounting), and `state.offset` counts
  precisely that prefix on every branch (`cursor_handlers.rs:1124, 1447,
  1476, 1502`). Concurrent writes between the keyset phase and the
  fallback are invisible by construction — both read
  `Temporal::AsOf(pinned)` under the same held `SnapshotGuard`. The
  "exactly once" transition claim holds: the mode flip commits only AFTER
  the fallback page clears the budget gate (`cursor_handlers.rs:1523-1560`),
  so a `CursorPageTooLarge` rejection of the triggering page leaves the
  cursor fully Keyset and re-triggerable — deliberate and correctly
  reasoned in the comment. Both new tests (bounded-drain livelock
  reproduction, cap-8/12-tie/3-distinct exactly-once transition) are
  well-constructed. One non-bug worth knowing: with `page_size ==
  max_cursor_page_size` and a tie run exactly equal to the ceiling, the
  fallback triggers even though one more doubling would have progressed —
  the cursor just finishes in (correct) offset mode; perf-only.
- **CR-D2 (null probe) — airtight for its declared scope.**
  `create_cursor` is the only place `PaginationMode` is ever assigned; the
  probe runs on every shape-Keyset path before the first page, against
  the same pinned version, after `drain_all` — there is no
  probe-vs-first-page race (MVCC pin; writes after `open_snapshot` land
  at higher versions and are invisible to both reads). AND-combining the
  caller's own WHERE into the probe is correct AND matches filter
  semantics for null rows (an unresolvable WHERE comparison satisfies
  only `Ne`, and the probe evaluates through the identical filter path
  the real pages use). `Filter::IsNull`/`is_null_at` covering
  explicit-Null and absent-field identically was verified. The
  keyset-preserved regression test (`pinned_mode` helper) guards against
  silently degrading every cursor to Offset. Residual gaps (mixed-type,
  NaN) are disclosed as promised — but see W-2 (Bin/List missing from the
  disclosure) and W-4 (probe outside the budget gate).
- **CR-D4 (single serialization) — no stale-byte leak on any path.**
  Every return statement in both functions was traced.
  `create_cursor`: 12 pre-budget error returns (nothing stashed), the
  too-large rejection (nothing stashed; upfront guard dropped →
  released), `!has_more` (stash then return — response IS final),
  `register Ok` (stash then return), `register Err` ×2 (bytes explicitly
  discarded — correct). `fetch_next`: all error returns precede
  `enforce_page_budget`; after the stash there is provably no branch that
  can produce a different response (state mutation, `bump_activity`,
  registry remove, then `return response` of the same value serialized).
  The measured payload is now genuinely the full wire envelope, matching
  `execute()`. One benign transient: on a `register()` failure the RI-15
  *guard* (already stashed inside `enforce_page_budget`, sized to the
  abandoned page) rides out attached to the small error response until
  its write completes — bounded over-accounting, not a leak. The
  wire-bytes-identical and register-failure tests directly pin the two
  risks the brief named.
- **CR-D5 — all three sub-fixes land as specified.** `restore.rs`: the
  two swap-failure variants (`SwapFailedRollbackSucceeded` vs
  `SwapPartialFailure`) carry accurate, distinct operator instructions;
  temp-dir cleanup is applied to exactly the step-3/step-4 paths and
  deliberately NOT the second-rename failure paths; tests cover both.
  The `drain_all` doc rewrite's central claim was independently verified
  against `MvccStore`: `get_at` probes the overlay before history
  (`mvcc_store/mod.rs:660-686`), and enumeration merges
  `overlay.newest_visible`/`snapshot_le` with history — undrained data
  is correctly visible, so demoting the drain to a logged best-effort
  optimization is the right resolution of N-7. `handler.rs` stale-comment
  fix is fine. TS `return()` now awaits `pending` before cleanup with the
  first-next-in-flight edge documented — matches the brief. The
  authorize/resolve reorder is behavior-neutral in every case except the
  dropped-db edge in W-6.
- **CHANGELOG / KNOWN_LIMITATIONS / CURSORS.md / 07-operations.md**: the
  new text was checked against the code line-by-line — accurate,
  including the N-5 concurrency-division paragraph (worked example
  matches `handler.rs`'s upfront reserve), except the W-2/W-7 disclosure
  gaps.

---

## 5. Remaining findings (LOW detail)

### W-4 (LOW) — CR-D2 probe escapes the RI-15 admission gate

`order_by_column_contains_null` runs at `cursor_handlers.rs:1039` —
before `reserve_page_budget_upfront` (`:1075`). Its read is a full
`read_as_of` scan that materializes the ENTIRE null-matching set before
`LIMIT 1` is applied (the probe's own doc comment says so). The
first-page read is admission-gated by the upfront reserve precisely to
bound execution-time memory (CR-B2's stated purpose); the probe is not —
a table with many large null-scored rows pays an unbudgeted
materialization per `CreateCursor`. One-time per cursor, so bounded in
frequency; either move the probe after the reserve (it can share the
same reservation) or note the exemption in the probe's Cost section.

### W-5 (LOW) — restore.rs residual gaps

- `restore.rs:205`: if the FIRST rename (`data_dir → backup_sibling`)
  fails (e.g. `data_dir` held open by an unrelated process on Windows),
  the bare `?` propagates `RestoreError::Io` — no cleanup, and the error
  message contains no reference to the fully-staged `temp_dir` left
  behind. This is precisely the N-6 "orphaned staged copy with no
  discoverable pointer" class; the brief enumerated only steps 3/4.
  Either `cleanup_staged_temp_dir` here too (nothing has been swapped
  yet, the staged copy has no forensic value) or wrap the error to name
  `temp_dir`.
- `restore.rs:159-168`: two `restore()` calls in the same wall-clock
  second compute the same `temp_dir` name; both can pass the
  `temp_dir.exists()` check before either creates it (`create_dir_all`
  succeeds for both), interleaving two copies into one staged dir. An
  operator-CLI-level race requiring two concurrent restores of the same
  target — contrived; `fs::create_dir` (fails-if-exists) instead of
  `exists()` + `create_dir_all` closes it in one line.

### W-6 (LOW) — auth-reorder wire-code drift on dropped-db-mid-scroll

`fetch_next` now authorizes (`cursor_handlers.rs:1354`) before
`resolve_repo` (`:1362`). `authorize_access` is fail-closed on a missing
resource for non-admin actors (`access_control.rs:850-865` denies on any
`resource_meta` failure), so a db dropped between `CreateCursor` and
`FetchNext` now surfaces as `access_denied` where it previously surfaced
as `unknown_db` (admin actors short-circuit and still get `unknown_db`).
Defensible — arguably the fail-closed answer is better — but the brief
sold this as "a pure reordering with no behavior change", and no test
pins either code for the dropped-db case. Add the test and a one-line
comment acknowledging the intentional code change (or accept and
document).

### W-7 (LOW) — offset-fallback × mixed-type column: duplicates, not just loss

For the documented-open mixed-type/`NaN` ORDER BY columns, keyset pages
skip incomparable rows, so `state.offset` (count of returned rows) is
SMALLER than the true global-sorted-prefix position. If such a column
also trips `StuckAtCeiling` (easy: the huge Equal-tie interleave a
mixed-type sort produces is exactly a giant tie run), CR-D1's fallback
resumes the plain offset scan at `state.offset` — re-serving rows already
returned (duplicates) in addition to the already-documented loss.
Subsumed by "keyset over such columns is broken", but the
`KNOWN_LIMITATIONS.md` §6 / `CURSORS.md` §1.1 text promises only *loss*
("may silently drop rows") — add "and, if the tie-run ceiling fallback
triggers, may also return duplicates" so the disclosure stays exact.
Resolves automatically if W-2/W-3's bookmark-build type check grows into
a "column type is totally ordered" gate that keeps such columns off the
keyset path entirely.

---

## 6. Prioritized action list

1. **W-1** — fix `cmp_i64_f64`'s tie-break (`f > f_floor`, both sites) +
   negative-fraction tests + doc-comment correction. *(Release blocker —
   a Wave-D-introduced regression on everyday values.)*
2. **W-2** — Bin/List keyset row loss: keyset-ineligibility (or seek
   `None`) at bookmark build + docs bullet + pinning test.
3. **W-3** — Dec/Big seek-key convertibility check at bookmark build
   (routes to the existing offset arm) + fix the two false comments +
   test. *(Natural single task with W-2 — same code sites.)*
4. **W-7** — one-sentence duplicates disclosure in both docs (folds into
   W-2/W-3's commit).
5. **W-5** — restore first-rename cleanup/message + `fs::create_dir`
   swap.
6. **W-4** — probe-after-reserve (or documented exemption).
7. **W-6** — dropped-db-mid-scroll test pinning the (accepted)
   `access_denied` code.
