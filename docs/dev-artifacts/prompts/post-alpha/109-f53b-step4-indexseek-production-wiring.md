# Brief for F-53b Step 4 (#880, P1, IMPLEMENT) — production-wire
`PaginationMode::IndexSeek` into `cursor_handlers.rs`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. F-53b Step 3 (`c0d54884`, #879)
settled a design (memo:
`docs/dev-artifacts/research/f53b-step3-cursor-pagination-after-spike.md`)
for wiring `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
`FetchNext` to reach the AsOf sorted-index keyset-seek arm
(`read_as_of_keyset_seek`, `crates/shamir-engine/src/table/
read_asof_seek.rs`, landed F-53b Step 2) for a NARROW but real cursor
shape: no caller `WHERE` clause, single simple-field ORDER BY, a sorted
index covering it. This task lands that design as production code (the
Step 3 spike deliberately only prototyped it test-locally in
`crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs` —
read that file first, it is the reference implementation for the
mechanism you are porting).

**Read the settled design memo in full before writing any code** —
`docs/dev-artifacts/research/f53b-step3-cursor-pagination-after-spike.md`,
section 2 ("Settled design for Step 4"). This brief restates the memo's
decisions with exact file/line anchors; the memo has the full reasoning
for WHY each decision was made.

### The permanent restriction (do not attempt to relax it)

`TableManager::try_plan_keyset_seek` (`crates/shamir-engine/src/table/
read_planner.rs:472-525`) hard-requires `query.r#where.is_none()` — shared
by both the `Temporal::Latest` fast path and the new AsOf arm. This means
`IndexSeek` mode can ONLY ever apply to a cursor whose ORIGINAL caller
query has NO `where` clause at all. Every cursor with a `WHERE` clause
keeps today's exact `Keyset`/`Offset` scheme, completely unchanged. Do
NOT modify `try_plan_keyset_seek`'s guard or attempt to add residual-WHERE
support to the seek machinery — explicitly out of scope (see the memo
§1.1 for why this is a materially bigger change).

## What to implement

### 1. `PaginationMode::IndexSeek` — new third variant

In `crates/shamir-server/src/cursor_registry.rs` (`PaginationMode` enum,
currently `Keyset`/`Offset`, lines ~116-125): add `IndexSeek` as a third
variant, documented per the memo §2.1 (decided once at `create_cursor`,
falls back permanently to `Offset` on the first `FetchNext` where the
fast arm doesn't fire — mirrors CR-D1's existing `StuckAtCeiling ->
Offset` transition, documented on the same enum a few lines above).

### 2. `CursorState` gains `after_id: Option<RecordId>`

`CursorState` (same file, ~lines 131-182) gets one new field:
`after_id: Option<RecordId>` (per memo §2.2). Reuses the EXISTING
`seek_key: Option<QueryValue>` field for the ORDER BY value half of the
bookmark — only the id half is new, because `IndexSeek` is the only mode
whose read path attaches a real `RecordId` (unlike `Keyset` mode's
`tie_skip` counting substitute, which stays `Keyset`-specific, unused for
`IndexSeek`). `Cursor::new`'s `CursorState` literal needs the new field
initialized to `None`.

### 3. Eligibility probe in `create_cursor`

`crates/shamir-server/src/db_handler/cursor_handlers.rs`'s `create_cursor`
(around line 1217, right where `let mut mode = pagination_mode_for_query(&query);`
currently starts the `Keyset`/`Offset` decision chain): add a NEW probe
function (a free function beside `pagination_mode_for_query`, mirroring
its shape) that runs FIRST, before that existing chain:

```rust
fn probe_index_seek_eligible(
    table: &TableManager,
    query: &ReadQuery,
    interner: &Interner,
) -> Option<u64> {
    if query.r#where.is_some() {
        return None;
    }
    let order_by = query.order_by.as_ref()?;
    if order_by.items.len() != 1 {
        return None;
    }
    let item = &order_by.items[0];
    if item.field.len() != 1 {
        return None;
    }
    let field_path = intern_field_path(&item.field, interner)?;
    let def = table.sorted_indexes().find_by_field(&field_path)?;
    Some(def.name_interned)
}
```

This is an EXACT port of the prototype in
`f53b_step3_cursor_after_spike.rs` (`probe_index_seek_eligible`,
lines 171-190 of that file) — built entirely from already-`pub` API
(`TableManager::sorted_indexes()`, `SortedIndexManager::find_by_field`,
`shamir_engine::query::filter::resolve::intern_field_path`, all
confirmed reachable from `shamir-server` via `shamir_db`'s blanket
re-export — see the memo §1.2 for the exact grep proof). No new `pub`
method needs to be added anywhere.

Wire it as the FIRST branch: if `Some(index_name)`, pin
`mode = PaginationMode::IndexSeek` and SKIP the existing
`order_by_column_is_schema_typed_scalar` + `order_by_column_contains_null`
probes entirely (memo §2.1 — the seek arm's own per-candidate MVCC
classifier in `read_as_of_keyset_seek` already handles the data-shape
edge cases those two `Keyset`-specific safety probes exist for; running
them for `IndexSeek` would be unnecessary work, not a correctness gap).
Only when `probe_index_seek_eligible` returns `None` does the existing
`pagination_mode_for_query` -> schema-typed-gate -> null-probe chain run,
completely unchanged.

### 4. `create_cursor`'s first page for `IndexSeek` mode

The first-page read currently always uses `Pagination::LimitOffset`
(lines ~1252-1260). For `IndexSeek` mode, build the first page via
`Pagination::after_with_id(vec![], Some(internal_limit), None)` instead
(an empty/sentinel initial key — the seek arm's `lookup_range_first_k_page`
needs SOME starting key; check `read_as_of_keyset_seek`'s and
`try_plan_keyset_seek`'s handling of the initial no-bookmark-yet case
and mirror whatever convention the EXISTING `Temporal::Latest` keyset-seek
callers use for their own first page — grep for how `read_index_scan.rs`'s
production callers seed the first `Pagination::After.key` before any
`FetchNext` has run, and reuse that exact convention rather than
inventing a new one). After the read, extract `(seek_key, after_id)` from
the last row for the `CursorState` bookmark, mirroring how the existing
code extracts `(seek_key, tie_skip)` for `Keyset` mode at lines
1283-1314 — but simpler, since `after_id` is just the last row's real
`RecordId` (`row.id`), no tie-counting needed.

**Verify `stats.index_used` ends in `_asof_keyset` on this first page**
before committing to `IndexSeek` mode for the cursor's lifetime — if it
doesn't (the gate declined even at creation time, e.g. a concurrent write
raced `create_cursor` itself), fall back to building the first page via
`PaginationMode::Offset`'s existing `Pagination::LimitOffset` path instead
(same fallback logic as step 6 below, just at creation time instead of
`FetchNext` time).

### 5. `fetch_next`'s new `IndexSeek` dispatch arm + the mandatory fallback

In `fetch_next`'s dispatch `match (state.mode, state.seek_key.clone())`
(currently 2 arms, lines 1655-1739): add a NEW arm for
`(PaginationMode::IndexSeek, seek_key_opt)`, per memo §2.3:

```rust
(PaginationMode::IndexSeek, seek_key_opt) => {
    let mut q = base_query.clone();
    q.pagination = Pagination::after_with_id(
        seek_key_opt.map(|v| vec![v]).unwrap_or_default(),
        Some(effective_page_size as u64),
        state.after_id,
    );
    q.temporal = Temporal::AsOf { at: At::Version(cursor.pinned_version()) };
    let result = match table.read_with_encoding(&q, &ctx, Default::default()).await {
        Ok(r) => r,
        Err(e) => { drop(state); return error_response(&wrap_engine_err(e)); }
    };

    let took_seek_arm = result.stats.as_ref()
        .and_then(|s| s.index_used.as_deref())
        .is_some_and(|l| l.ends_with("_asof_keyset"));

    if took_seek_arm {
        // normal path: peek-row convention for has_more (mirror the
        // existing CR-B4 peek trick — the seek arm's own `limit` already
        // asks for exactly effective_page_size, check whether it ALSO
        // needs a +1 peek by reading read_as_of_keyset_seek's contract
        // for has_more detection, or whether index_used + records.len()
        // < requested is sufficient; settle this by testing against the
        // real engine, do not assume), extract new (seek_key, after_id)
        // from the last row, new_offset = state.offset + returned rows
        // (MUST be tracked in parallel here too, exactly like the Keyset
        // arm does — this is the CR-D1 precondition the fallback below
        // depends on).
    } else {
        // MANDATORY one-time fallback, mirrors CR-D1's StuckAtCeiling
        // handling exactly (lines ~1684-1712): re-run THIS SAME call via
        // fetch_offset_page(state.offset, ...) so the caller gets a
        // correct page back, set force_offset_mode = true so the commit
        // block below (state.mode = PaginationMode::Offset) fires only
        // after the page clears the budget gate (same ordering
        // requirement CR-D1 already documents at lines 1753-1764 — a
        // rejected page must NOT have already committed the mode flip).
    }
}
```

Reuse the SAME `force_offset_mode` flag/commit-ordering the existing
`Keyset` arm already has (lines 1654, 1788-1790) — do not invent a
second flag or a different commit-ordering rule.

### 6. Tests — port the spike's 3 tests to the production path, add one more

Per memo §2.4, in `crates/shamir-server`'s existing cursor handler test
file (find it — likely `tests/cursor_handler_tests.rs` or similar under
`crates/shamir-server/src/db_handler/tests/` or `crates/shamir-server/
tests/`; check both and use whichever already holds `CreateCursor`/
`FetchNext` integration tests):

- Port `probe_1_eligibility_probe_uses_only_public_api` and
  `probe_2_seek_arm_attaches_real_record_id`'s intent into a test that
  drives the REAL `create_cursor` (not the test-local probe function) —
  assert `IndexSeek` mode gets pinned for an eligible query and NOT for
  one with a `WHERE`/unindexed/multi-column ORDER BY.
- Port `negative_gate_failure_mid_lifetime_must_not_page_one_forever`'s
  scenario against the REAL `fetch_next`: `CreateCursor` (no WHERE,
  indexed ORDER BY) -> `FetchNext` page 1 (assert `IndexSeek`, seek arm
  fired) -> concurrent write -> `FetchNext` page 2 (assert transparent
  fallback to `Offset`, CORRECT data — not a duplicate of page 1, mode is
  now permanently `Offset`) -> `FetchNext` page 3 (assert it stays on
  `Offset`, no attempt to re-probe `IndexSeek`).
- Add DESC-direction parity (the seek arm already supports it per Step
  2's `direction: OrderDirection` param; the cursor wiring should carry
  it through without a separate code path).

## What NOT to do

- Do NOT modify `try_plan_keyset_seek`'s `where.is_some()` guard or any
  shared planner logic.
- Do NOT change `Keyset`/`Offset` mode behavior in any way — `IndexSeek`
  is purely additive; both existing modes must stay byte-for-byte
  unchanged (verify with the existing cursor test suite passing
  unmodified).
- Do NOT attempt multi-column ORDER BY support for `IndexSeek` — no
  composite seek primitive exists (out of scope, per the memo).
- Do NOT touch `read_asof_seek.rs`, `read_planner.rs`, `read_temporal.rs`,
  or any other already-landed F-46 through F-54 engine-side code — this
  task is cursor-side wiring only, consuming the existing engine API.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-server -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Zero-trust: after landing, personally re-verify the negative test by
  temporarily reverting the fallback logic and confirming it genuinely
  fails (reproduces page 1 instead of page 2) before restoring the fix —
  the orchestrator will additionally do this themselves, but demonstrate
  it in your own summary too.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-server -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-engine --full
```

When done, give your final summary as plain text: what was implemented,
the exact `has_more`/peek-row convention you settled on for the seek arm
(and why), test results (counts, pass/fail), and confirmation
fmt/clippy/tests are clean.
