# #641 — `$cond`/`FilterValue` doesn't compose into write SET-values

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The gap (already root-caused, do not re-investigate from scratch)

`InsertOp.values: Vec<QueryValue>`, `UpdateOp.set: QueryValue`,
`SetOp.{key,value}: QueryValue` (`crates/shamir-query-types/src/write/types.rs:77-132`)
are typed as plain `QueryValue` — literal data only. There is NO resolution
step anywhere in the write execution path
(`crates/shamir-engine/src/table/write_exec.rs`) for `$query`/`$fn`/`$cond`/
`$expr` markers embedded inside a write payload. Concretely: if a caller
writes `{"user_id": {"$query": "@orders", "path": "[0].user_id"}}` as an
INSERT value, that literal map (containing the string `"@orders"` and
`"path"` keys) is inserted AS-IS into the table — never resolved to the
real value the `$query` ref points to.

**Important nuance already discovered**: this is NOT a total gap.
`crates/shamir-engine/src/query/batch/param_subst.rs` ALREADY substitutes
`$param` markers inside write values (`contains_param_ref`/
`substitute_params`, called from `query_runner.rs`'s `BatchOp::Insert`/
`Update`/`Set` dispatch arms, e.g. line ~651-680 for Insert) — this is the
EXACT precedent/pattern to extend, not a new mechanism to invent.
Additionally, `BatchPlanner::extract_deps_from_value`
(`crates/shamir-query-types/src/batch/planner.rs:317-339`) ALREADY scans
write values for a `"$query"` key to build DAG dependency edges — but its
detection is LOOSE (`map.get("$query")` anywhere in a map, not an exact
single-key check like `$param`'s) and it does NOT recurse into `$fn`/
`$cond`/`$expr` shapes the way `extract_deps_from_filter_value` (fixed for
WHERE/`when` in bug #642) already does. So today: the planner sets up
correct EXECUTION ORDER for a write containing `$query`, but the VALUE
itself is never resolved — the dependency edge exists but is pointless
because nothing consumes it.

## Chosen fix direction (decided by the orchestrator — implement exactly this)

**No wire format change.** `InsertOp.values`/`UpdateOp.set`/`SetOp.{key,value}`
stay `QueryValue` on the wire — a `$query`/`$fn`/`$cond`/`$expr` marker is
simply a `QueryValue::Map` that happens to carry one of these reserved keys,
exactly the SAME convention `$param` already established and that this
codebase already relies on throughout OQL (WHERE, `when`, `for_each`'s
`over`). This is a zero-wire-break, purely additive fix.

1. **Generalize `param_subst.rs`'s walker.** Rename/extend
   `contains_param_ref`/`substitute_params` (or add sibling functions in the
   same file — your call on the cleanest shape) to recognize ALL FIVE
   reserved markers: `$param`, `$query`, `$fn`, `$cond`, `$expr` — NOT
   `$ref` (`FieldRef`, "another field in the same document"): explicitly
   OUT OF SCOPE for this task, since resolving it would require the
   partially-built document as context, a materially harder problem;
   document this exclusion clearly in the code comment (mirrors `when`'s
   documented exclusion of field-based comparisons for a parallel reason —
   no meaningful record context).
   - Detection: a `QueryValue::Map` node is a marker iff it has EXACTLY ONE
     key matching one of the 4 in-scope reserved names (`$param`/`$query`/
     `$fn`/`$cond`/`$expr`) — mirror `$param`'s existing `map.len() == 1`
     check, but do NOT require the value to be a `String` (only `$param`'s
     payload is a bare string; `$query`/`$fn`/`$cond`/`$expr` carry richer
     structures — an alias+path map, a `FnCall` map, a `Cond` map, a
     `FilterExpr` map respectively).
   - Resolution: when a marker is detected, convert the single-key
     `QueryValue::Map` into a `FilterValue` via a raw msgpack round-trip
     (serialize the `QueryValue`, deserialize as `FilterValue` — this
     works because a `FilterValue`'s wire encoding for `QueryRef`/`FnCall`/
     `Cond`/`Expr`/`Param` IS exactly this single-reserved-key map shape;
     `crates/shamir-query-types/src/filter/filter_value.rs`'s doc comment
     on `query_value_to_filter_value` even hints at this: "Callers should
     use the msgpack round-trip for Map"). Then resolve the `FilterValue`
     via `resolve_filter_query` (`crates/shamir-engine/src/query/filter/resolve.rs`)
     against a `FilterContext` built from the CURRENT `resolved_refs`/
     `actor`/`params` already in scope at the Insert/Update/Set dispatch
     site in `query_runner.rs` (same context these dispatch arms already
     have available for the `$param`-only substitution today). Use a dummy/
     null record for the `RecordRef` argument (consistent with `when`'s and
     `for_each`'s established "no real record" pattern) — this is fine since
     `$ref`/`FieldRef` is explicitly excluded from this task's scope.
   - If the msgpack round-trip / `resolve_filter_query` resolution FAILS
     for a detected single-reserved-key map (malformed payload), return a
     clear `BatchError` (do NOT silently fall back to treating it as a
     literal — these 4 reserved key names should not naturally collide with
     real document field names, matching `$param`'s existing "error on
     unbound" philosophy rather than silent pass-through).
   - Preserve the EXISTING fast-path: a pre-scan (`contains_param_ref`'s
     generalized equivalent) short-circuits to a plain clone when the value
     tree has NO markers at all — the overwhelming common case (plain
     literal document writes) must pay zero extra cost.
2. **Fix `extract_deps_from_value`'s dependency extraction** to match the
   precision AND recursion depth of `extract_deps_from_filter_value` (the
   #642 fix): tighten the `$query` detection to the exact single-key-map
   convention above, and recurse into `$fn`'s args / `$cond`'s branches /
   `$expr`'s operands the same way `extract_deps_from_filter_value` already
   does for WHERE-clause filters — read that function
   (`crates/shamir-query-types/src/batch/planner.rs`, search for
   `extract_deps_from_filter_value`) and mirror its recursion structure for
   the write-value walker.
3. **Update `query_runner.rs`'s Insert/Update/Set dispatch arms** (lines
   ~639+, ~735+, ~979+) to call the generalized resolver instead of (or in
   addition to, if you keep `substitute_params` as a thin wrapper — your
   call on the cleanest refactor) the `$param`-only substitution.
4. **Rust + TS builders**: confirm (or add if missing) a way to construct a
   write payload value that embeds a `$query`/`$fn`/`$cond`/`$expr` marker —
   check `crates/shamir-query-builder/src/` and
   `crates/shamir-client-ts/src/core/builders/` for how `doc()`/write-row
   builders currently accept values, and whether `val::query_ref(...)` /
   similar helpers already produce the right `QueryValue`-shaped marker (if
   `FilterValue`-typed helpers exist but aren't accepted in write-row
   position today, wire them through).
5. **Tests**: unit tests proving an INSERT/UPDATE/SET value containing a
   real `$query` ref to another alias's result resolves correctly at
   execution time (not just that the dependency edge exists — that already
   silently "worked" before this fix in the sense of ordering, but the
   value itself was wrong). Also a test for `$fn`, `$cond` as an embedded
   write value. Also a test confirming a malformed single-reserved-key map
   errors clearly instead of writing garbage. Also confirm the existing
   `$param`-only behavior is unchanged (regression guard).
6. **Docs**: update `docs/guide-docs/guide/01-queries.md` (or wherever
   write/INSERT is documented) with a short section on embedding `$query`/
   `$fn`/`$cond`/`$expr` inside write values, and update
   `docs/dev-artifacts/roadmap/oql/FINAL-SUMMARY.md`'s #641 entry to mark it
   resolved (with the `$ref`/FieldRef exclusion noted as a documented,
   deliberate scope boundary, not a remaining bug).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p
  shamir-query-builder --full` green.
- TS builder unit tests green (whatever suite covers write-row builders).
- `cargo fmt --check` clean for every touched crate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.

## Out of scope

- `$ref`/`FieldRef` resolution inside write values (same-document field
  references) — explicitly deferred, document why.
- Do NOT touch #643, #634, #659 — separate tasks.
- Do NOT change the wire format of `InsertOp`/`UpdateOp`/`SetOp`.
