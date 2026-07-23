# Brief: CR-D2 — keyset cursor silently loses Null/incomparable ORDER BY rows (#783, release blocker)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

**Read this brief fully before touching code — this bug's failure shape
(silent data loss, no error, clean `has_more: false`) is the worst class
this cursor feature has produced. Be careful and thorough.**

## Problem — verified by an independent review

The keyset bookmark's boundary filter is `field >= seek_key` (ASC) /
`field <= seek_key` (DESC) (`boundary_filter`,
`crates/shamir-server/src/db_handler/cursor_handlers.rs`, ~lines 357-383),
evaluated through `compare_values`
(`crates/shamir-engine/src/query/filter/resolve.rs`). Any row whose ORDER
BY value CANNOT be ordered against the seek key makes that comparison
unresolvable — the filter is FALSE for that row (excludes it), no error,
no signal. Meanwhile `ORDER BY`'s sort (`QvSortKey`,
`crates/shamir-engine/src/query/read/order.rs`) places exactly these rows
where a later page's boundary filter can never reach them again:

- **`Null` / missing field** → `QvSortKey::Null`, sorted LAST under the
  ASC default. Page 1 (no boundary yet) returns only the leading
  real-valued rows; every SUBSEQUENT page's `field >= seek` excludes ALL
  null/missing rows — the scan appears to "run out," reporting
  `has_more: false`. **Every null/missing-value row is silently dropped.**
  (DESC happens to work, since nulls sort FIRST there and the boundary
  scan reaches them naturally.)
- **Mixed-type column** (some rows `Int`, some `Str`): the sort treats
  cross-type keys as `Equal` (insertion-order interleave), but after page
  1 the boundary `field >= Int(x)` returns `None`/false for every `Str`
  row — every row of the OTHER type(s) is silently dropped once the scan
  moves past page 1.
- **`NaN` in an `F64` ORDER BY column**: any comparison involving `NaN` is
  `None`/false — same loss; `NaN` additionally breaks
  `same_boundary_value`'s tie-counting (`f64`'s `PartialEq` on `NaN` is
  always false).

No existing test covers ANY of these (every keyset test uses a uniformly-
typed, non-null `i64` column). Unlike the CR-A4/CR-B1/CR-D1 bugs, the
client gets **no error at all** — just a clean, confident-looking
`has_more: false` that is silently wrong.

`pagination_mode_for_query` (~search for it) CANNOT detect this
statically — whether a column has nulls/mixed types/NaN is a DATA
property, not something derivable from the query's shape alone. This is
why the fix below is necessarily either a data-dependent check or an
honest documented limitation, not a static mode-selection change.

## Fix — tiered scope, in priority order

### 1. MUST fix: Null/missing ORDER BY values, detected at `create_cursor` time

This is very likely the MOST COMMON real-world trigger (an optional field
absent on some rows is routine; `NaN`/mixed-type columns are rarer). It is
also cleanly detectable with a single cheap existence check: `Filter::IsNull`
already exists in this codebase's filter AST
(`crates/shamir-query-types/src/filter/filter_enum.rs`, ~line 65) — at
`create_cursor` time, AFTER deciding `mode = pagination_mode_for_query(&query)`
returns `Keyset` (a single-column simple ORDER BY), run ONE additional
cheap read against the SAME pinned snapshot: `WHERE <order_by_field> IS
NULL`, limited to 1 row (existence check only, not a count — cheapest
possible query shape). If it finds ANY row, the query is NOT safe for
keyset pagination — fall the WHOLE cursor back to `PaginationMode::Offset`
from creation (before running the first page), exactly like the existing
"no simple single-column ORDER BY" static fallback already does, just
decided from a data probe instead of query shape. This closes the
null/missing-field case completely and unconditionally — no cursor with a
null-containing ORDER BY column can ever hit the silent-drop bug again.

Verify this extra existence-check query is genuinely cheap (an indexed or
early-terminating scan, not a full-table materialization) before
committing to this design — check how `Filter::IsNull` is evaluated in
the engine's filter-execution path, and whether a `limit: 1` pagination
on a read query actually short-circuits the underlying scan rather than
materializing everything first. If it turns out this check is NOT
actually cheap (i.e. it forces a full table scan regardless of `limit:
1`), reconsider: the one-time cost at `create_cursor` is still bounded
(same cost class as `read_as_of`'s existing per-page full-scan-then-filter
approach, and the drain_all step already run once per cursor creation),
so even an O(table) existence check here is acceptable — it happens ONCE
per cursor lifetime, not once per page.

### 2. SHOULD investigate, MAY defer to documentation: mixed-type columns and `NaN`

Detecting "does this column contain more than one `QueryValue` variant
type" or "does this `F64` column contain any `NaN`" is a HARDER,
less-obviously-cheap check than a simple `IS NULL` existence probe — there
may be no existing filter primitive for "is this field a different type
than X" or "is this field NaN" in the current filter AST. Investigate
whether either is expressible cheaply with what exists today. If yes,
apply the SAME create-time-detection-and-fallback-to-Offset pattern as
the Null case above. If NOT cheaply expressible without deeper engine
changes, this is EXPLICITLY ACCEPTABLE to defer — document the residual
limitation honestly (see Docs below) rather than attempting a risky,
under-tested engine change under this task's time budget. State clearly
in your final report which of these two sub-cases you fixed vs.
documented, and why.

### 3. Full two-phase-scan design (NOT this task's scope)

The "textbook" complete fix — a keyset phase over comparable values,
followed by an offset-bookmarked TAIL phase specifically for the
null/incomparable rows once the keyset phase exhausts — is explicitly OUT
OF SCOPE for this task (per the review that raised this finding: "this
deserves its own follow-up task if judged too large for this task's
scope"). Do NOT attempt this restructure here. If, while investigating
#2 above, you conclude the WHOLE problem (including mixed-type/NaN)
could be closed with the SAME create-time-fallback-to-Offset trick used
for Null (i.e. a broader "does this column contain ANY value that isn't
comparable to itself" check), that's still in-scope and preferred over
leaving it undocumented — the boundary you must not cross is building a
genuinely new two-phase pagination scheme.

## Docs (do NOT skip, whichever fix tier you land on)

- `docs/guide-docs/client-server-protocol-spec/CURSORS.md`: document that
  a keyset cursor now (per fix #1) automatically falls back to offset-mode
  pagination when its ORDER BY column contains a `Null`/missing value
  (checked once at `CreateCursor` time) — this is user-visible behavior
  (their query's pagination cost model may silently change from
  keyset-style to offset-style based on data, not query shape) and belongs
  in the wire-contract doc. If you deferred #2, ALSO document that a
  mixed-type or NaN-containing ORDER BY column is a STILL-OPEN limitation
  (keyset cursors over such columns may silently drop rows) — do not paper
  over an accepted gap with vague language; state exactly what's still
  broken.
- `docs/guide-docs/KNOWN_LIMITATIONS.md` §6: mirror the same disclosure —
  update or add a bullet describing the current (post-this-task) state
  precisely.

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **Null ORDER BY value, ASC**: a column with SOME rows having a null/
  missing value in the ORDER BY field — confirm this test FAILS against
  the current code (the null rows never appear across the whole drained
  cursor) BEFORE your fix, then passes after (every row, including the
  null ones, appears exactly once — regardless of which mode ultimately
  serves them).
- **Regression**: existing keyset tests over a uniformly-typed, non-null
  column must stay green AND must still actually exercise the Keyset code
  path (not accidentally start routing through the new Offset-fallback
  probe for a column that has no nulls) — assert on `PaginationMode`'s
  test-visible signal if one exists (check `pagination_mode_pinned_at_creation`-
  style existing tests for the convention), so this fix doesn't silently
  degrade EVERY cursor to offset mode by mistake.
- **Mixed-type / NaN**: whichever of "fixed" or "documented" you land on
  for these two, add a test PINNING that exact behavior (either "every row
  appears exactly once" if fixed, or an explicit test asserting the
  CURRENT, documented-limitation behavior — e.g. "some rows are dropped
  under these conditions" — so the gap is at least tracked by a test, not
  silently unverified).

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`cursor_handlers.rs`, its tests, `CURSORS.md`, `KNOWN_LIMITATIONS.md`).
Do NOT touch `fetch_keyset_page`'s tie-run-ceiling logic (CR-D1's
territory, already fixed and committed) — this task is about detecting an
UNSAFE-FOR-KEYSET column at `create_cursor` time and falling back
entirely, not about anything inside the per-page retry loop itself.
