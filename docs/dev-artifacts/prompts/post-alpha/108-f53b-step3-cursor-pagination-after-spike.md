# Brief for F-53b Step 3 (#879, P1, SPIKE) — can cursor_handlers.rs reach
the AsOf keyset-seek arm, and how?

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. F-53b Step 2 (`54e8f0ef`, #878)
landed `crates/shamir-engine/src/table/read_asof_seek.rs`'s
`read_as_of_keyset_seek` — an `O(page_size)` sorted-index seek arm for
`Temporal::AsOf` reads, dispatched from `read_temporal.rs:94-114` when
`TableManager::try_plan_keyset_seek` (`read_planner.rs:472-525`) returns
`Some(..)` AND `sorted_indexes().last_mutation_version() <= version`
(the mutation high-water gate).

Step 2's brief assumed `crates/shamir-server/src/db_handler/
cursor_handlers.rs`'s `FetchNext` could be converted to send
`Pagination::After` and reach this new arm. Step 2's own delegated agent
investigated and correctly, honestly found this premise incomplete and
deferred the conversion rather than force it — filed as this task (#879).

**This orchestrator's own follow-up investigation (before writing this
brief) found the deferral was well-founded, and surfaced the EXACT reason,
which this brief now states precisely so the spike does not have to
re-discover it from scratch:**

### The blocking finding: `try_plan_keyset_seek` requires `query.r#where.is_none()`

`read_planner.rs:478-485`:
```rust
if query.r#where.is_some()
    || query.group_by.is_some()
    || query.select.distinct
    || query.count_total
    || exec::has_aggregates(&query.select)
{
    return None;
}
```

This is a documented MVP restriction shared by BOTH the pre-existing
`Temporal::Latest` keyset-seek fast path and the new AsOf arm — **not** a
new bug or gap Step 3 is expected to fix; extending the seek machinery to
support a `WHERE` clause is explicitly out of scope (a materially bigger
change than this task).

`cursor_handlers.rs`'s CURRENT bookmark scheme
(`boundary_filter`/`fetch_keyset_page`, module doc lines 11-95) ALWAYS
AND-combines an inclusive `Gte`/`Lte` boundary into `query.r#where` —
meaning if `cursor_handlers.rs` naively started sending
`Pagination::After` while still injecting that boundary filter into
`where`, `try_plan_keyset_seek` would see `where.is_some()` and return
`None` every time, falling through to the OLD full-scan/inline-top-K
paths below it in `read_as_of`.

**That fallback is unsafe for `Pagination::After` specifically**:
`Pagination::resolve()` (`shamir-query-types/src/read/limit.rs:176-186`)
maps `After { .. }` to `(skip=0, take=limit)` UNCONDITIONALLY — so any
path that consumes `.resolve()` instead of `.keyset()` (i.e. every path
below the seek-arm dispatch) will return PAGE ONE FOREVER for an `After`
pagination, never advancing. This is the EXACT hazard
`cursor_handlers.rs`'s existing module doc (lines 11-27) already documents
as the reason `Pagination::After` isn't used today — Step 3 must not
reintroduce it.

### What this means for scope

The AsOf seek arm can only help a cursor whose:
1. Original caller query has **no `WHERE` clause at all** (not even one
   this task could add — any `where` disqualifies `try_plan_keyset_seek`
   entirely), and
2. ORDER BY is a single, simple top-level field, and
3. A sorted index exists on that field, and
4. `last_mutation_version() <= pinned_version` holds (checked per-call by
   `read_as_of` itself, not something `cursor_handlers.rs` needs to
   duplicate).

Cursors with a caller `WHERE` clause (very common) **must keep today's
exact CR-A4 boundary-filter + `tie_skip` scheme unchanged** — there is no
way to make them reach the new arm without extending
`try_plan_keyset_seek` itself, which is out of scope here.

## What the spike must settle (prove or disprove each, with a real test —
do not assume)

1. **Eligibility probe**: how should `create_cursor` (or a new helper)
   determine, ONCE, whether THIS cursor's query qualifies for the
   index-seek bookmark scheme — i.e. mirror `try_plan_keyset_seek`'s
   guard shape (no `where`, single simple-field ORDER BY, a sorted index
   on that field exists) WITHOUT actually running a seek yet. Is there
   already a way to ask "does a sorted index cover this field" from
   `cursor_handlers.rs`'s vantage point (it already calls
   `pagination_mode_for_query`/`order_by_column_is_schema_typed_scalar`
   against a `&TableManager` — check whether `sorted_indexes()`
   /`find_by_field` is reachable, or whether a new small `pub` method on
   `TableManager` needs to be added, mirroring `try_plan_keyset_seek`'s
   shape but WITHOUT the pagination/where checks since those are exactly
   what create_cursor is deciding).
2. **A new `PaginationMode` variant** (e.g. `IndexSeek`) alongside today's
   `Keyset`/`Offset` (`cursor_registry.rs:117-125`), decided ONCE at
   `create_cursor` time per the existing CR-A4 discipline (never
   re-derived per `FetchNext`) — settle its `CursorState` field
   requirements. Since the new arm's `QueryRecord::Inserted { id:
   Some(id), .. }` DOES carry a real `RecordId` (confirmed:
   `read_asof_seek.rs:187-202`, unlike the generic AsOf full-scan
   projection which discards it — this is the key advantage over
   `Keyset` mode's `tie_skip` counting hack), decide whether
   `CursorState` should store an `Option<RecordId>` bookmark for this
   mode (feeding `Pagination::after_with_id`) instead of `tie_skip`.
3. **The silent-fallback hazard and its fix**: prove (with a real test)
   that when an `IndexSeek`-mode cursor's `FetchNext` sends
   `Pagination::After` and the fast arm does NOT fire for some reason
   (the gate fails due to an intervening write, or the index was dropped
   between `create_cursor` and this `FetchNext`), the response does NOT
   silently return page-one-forever. Investigate whether
   `QueryResult.stats.index_used` (set to
   `Some(format!("sorted_idx_{index_name}_asof_keyset"))` by the fast arm,
   `read_asof_seek.rs:207`) is a reliable, checkable signal
   `cursor_handlers.rs` can inspect after every `FetchNext` call to detect
   "the fast arm did NOT fire this call" and recover safely — e.g. by
   permanently falling back to the SAME kind of one-time, detected-failure
   transition CR-D1 (#782, `cursor_handlers.rs`'s existing
   `KeysetOutcome::StuckAtCeiling` -> permanent `Offset` fallback) already
   established as sound and precedented, rather than inventing a new
   pattern. Write a negative test that forces the gate to fail
   mid-cursor-lifetime (e.g. a concurrent write to the indexed column
   after `create_cursor` but before a `FetchNext`) and confirms the
   cursor either (a) transparently falls back without data loss/duplicate
   rows, or (b) errors clearly — NOT that it silently loops page one.
4. **Value-proposition sanity check**: given the no-`WHERE` restriction is
   permanent, roughly how much of this codebase's/typical cursor usage
   would actually hit the `IndexSeek`-eligible shape (a plain `ORDER BY
   <indexed field>` with no filter) versus needing today's
   `Keyset`/`Offset` schemes regardless? This isn't a blocking gate — just
   surface it honestly in the summary so the follow-up implement task
   (Step 4) can be scoped/prioritized realistically, without over-claiming
   this closes the whole cursor-performance gap.

## What NOT to do

- Do NOT modify `try_plan_keyset_seek`'s `where.is_some()` guard or any
  other part of the shared Latest/AsOf keyset-seek planner — extending it
  to support `WHERE` is explicitly out of scope.
- Do NOT implement the full production wiring in this spike — prototype
  the mechanism (test-local, like F-53b Step 1's
  `f53b_cursor_seek_spike_tests.rs`) and settle the design in a memo; the
  actual `cursor_handlers.rs`/`cursor_registry.rs` production wiring is a
  separate follow-up task once this spike's findings are reviewed.
- Do NOT touch any of F-46 through F-54's already-landed code.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt` / `cargo clippy --workspace --all-targets -- -D warnings`
  clean.
- Write the design memo to
  `docs/dev-artifacts/research/f53b-step3-cursor-pagination-after-spike.md`
  (mirror `f53b-cursor-seek-spike.md`'s structure: what was proven,
  disproven, and the settled design for the follow-up implement task).

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-server -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-server --full
```

When done, give your final summary as plain text: what was proven/
disproven, the settled `PaginationMode`/`CursorState` design, the
silent-fallback-hazard test result, the honest value-proposition read, and
confirmation fmt/clippy/tests are clean.
