# Brief for #796 (F-6) — count_total=true must exclude the top-K LIMIT fast path

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (confirmed by code reading, not just the review's claim)

`crates/shamir-engine/src/table/read_exec.rs`'s `read_collecting` computes
a `use_topk` gate (~line 975-979):

```rust
let use_topk = query.order_by.is_some()
    && take_resolved.is_some()
    && !query.select.distinct
    && !has_group_by
    && !has_agg;
```

This does **NOT** check `query.count_total`. When `use_topk` is `true`, the
result goes through `exec::apply_order_by_topk` (a bounded top-K heap) and
its pagination metadata is built via `exec::fast_path_pagination(&query.pagination)`
(~line 997) — a helper whose OWN doc comment
(`crates/shamir-engine/src/query/read/exec.rs`, ~line 102-112) explicitly
says it "applies a finite LIMIT without computing a total count... total
is `None`" — this is correct, INTENTIONAL behavior for the general
LIMIT-fast-path case, but it means **any query with `ORDER BY` + a finite
`LIMIT` that ALSO sets `count_total: true` silently gets `total: None`
back, ignoring the flag.**

The code comment directly above this call (~line 986-990) is WRONG and
must be corrected as part of this fix:

```rust
// count_total with top-K: we don't know the total from the
// heap alone — but we tracked records_scanned. For true
// count_total, the full-sort path is needed; top-K is memory-opt
// only. Guard: count_total is already excluded above via the
// `read_counting` path dispatch.
```

Traced the actual dispatch (`crates/shamir-engine/src/table/read_exec.rs`,
~line 772-810): the top-level dispatch is
`needs_full_collect = has_group_by || has_agg || has_order || has_distinct`
— when `has_order` (ORDER BY present) is `true`, `needs_full_collect` is
`true` **regardless of `count_total`**, routing to `read_collecting`
(where the buggy `use_topk` gate lives). The `else if query.count_total`
branch that calls `read_counting` is reached ONLY when
`needs_full_collect` is `false` — i.e. **no** ORDER BY at all. So
`read_counting`'s dispatch does NOT exclude `count_total` from the
ORDER BY + LIMIT case; it's simply never reached when ORDER BY is present,
or count_total==true and order_by==true, one to the same. The comment's
claim is false — `count_total=true` + `ORDER BY` + `LIMIT` DOES flow
through the buggy top-K gate inside `read_collecting`, exactly as the
review found.

## The fix

Add `&& !query.count_total` to the `use_topk` gate:

```rust
let use_topk = query.order_by.is_some()
    && take_resolved.is_some()
    && !query.select.distinct
    && !has_group_by
    && !has_agg
    && !query.count_total;
```

This routes `count_total=true` + `ORDER BY` + `LIMIT` queries to the
EXISTING `else` branch (~line 999-1002):

```rust
if let Some(ref order_by) = query.order_by {
    exec::apply_order_by_qv(&mut qv_result, order_by);
}
exec::apply_pagination(qv_result, &query.pagination, query.count_total)
```

`apply_pagination` (`crates/shamir-engine/src/query/read/exec.rs`,
~line 122-173) already correctly honors `count_total` (computes
`Some(records.len() as u64)` before slicing) — this is the EXISTING,
already-correct full-sort behavior for `count_total` without top-K; this
fix just makes sure `count_total=true` queries reach it instead of the
top-K path.

Rewrite the misleading comment above the `use_topk` branch (~line 986-990)
to describe the ACTUAL new behavior: `count_total=true` now excludes the
top-K optimization entirely (falls back to full sort via
`apply_order_by_qv` + `apply_pagination`, which correctly computes the
total) — do not leave the old, now-doubly-wrong claim about
"`read_counting` path dispatch" in place.

## Performance note (do not "fix" this, just be aware)

This means `count_total: true` on an `ORDER BY` + `LIMIT` query loses the
O(K)-memory top-K optimization and pays a full sort — this is the
CORRECT tradeoff (you cannot know a total without touching every matching
row), not a regression to fix further. Do not attempt to invent a
"count matching rows without full materialization" optimization here —
out of scope.

## Tests

Add a regression test (find the existing test file(s) for `read_exec.rs`'s
top-K path / `apply_order_by_topk` / the `#128`-referenced
`limit_queries_all_emit_pagination_contract` test mentioned in
`fast_path_pagination`'s doc comment — follow this repo's test
organization convention) that:

1. Seeds enough rows that `ORDER BY <field> LIMIT <k>` with `k` smaller
   than the row count would engage the top-K path.
2. Runs the SAME query with `count_total: true` added.
3. Asserts the returned `total` (`PaginationInfo::total_count` or
   whatever field name the wire response actually uses — check) equals
   the TRUE total row count (not `None`, not just the page size `k`).
4. As a sanity companion, confirm `count_total: false` (or omitted) on the
   identical query still returns `total: None` (the intentional top-K
   fast-path behavior is unchanged when the flag isn't set) — this
   guards against accidentally always going through the slow path now.

## Constraints

- Do NOT change `apply_pagination`, `fast_path_pagination`, or
  `apply_order_by_topk`'s own internals — this fix is purely the gate
  condition (and its comment) in `read_collecting`.
- Do NOT change behavior for `count_total=true` combined with
  `group_by`/`aggregates`/`distinct` — those already bypass `use_topk` via
  the existing `!has_group_by`/`!has_agg`/`!query.select.distinct` guards
  and are unaffected by this fix.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, `mod.rs` re-exports
  only, one primary export per file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
