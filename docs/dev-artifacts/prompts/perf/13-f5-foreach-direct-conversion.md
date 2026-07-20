# F5 — replace ForEach's per-iteration msgpack round-trip with a direct conversion

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fifth item of "Этап 8 — Performance", sourced from report 07
(`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), finding **F5** (severity Med-High) —
read that section (lines 163-193) first.

**The problem (verify against the ACTUAL current code — line numbers below
are current, not the report's, which cites slightly different lines from
before this campaign's other Этап 8 tasks landed):**
`crates/shamir-engine/src/query/batch/query_runner.rs` has the SAME
msgpack-round-trip pattern at TWO sites:
- line ~467-469 (inside the `BatchOp::Batch` handling — runs ONCE per
  sub-batch invocation, not the hot loop, but identical code):
  ```rust
  let value = rmp_serde::to_vec_named(&inner_results)
      .ok()
      .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok());
  ```
- line ~676-679 (inside the `ForEach` loop — **THIS is the hot loop**, runs
  once per iteration, up to `ABSOLUTE_MAX_FOR_EACH_ITERATIONS = 100_000`
  times):
  ```rust
  let value = rmp_serde::to_vec_named(&inner_results)
      .ok()
      .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok())
      .unwrap_or(QueryValue::Null);
  ```

Both convert `inner_results: TMap<String, QueryResult>`
(`BatchResponse::results`'s type, see `crates/shamir-query-types/src/batch/
batch_response.rs:36`) into a `QueryValue` PURELY so the outer `$query`
path-resolution code (`resolve_query_ref_value` in `resolve.rs`) can walk it
— by encoding to msgpack bytes and immediately decoding those same bytes
back into `QueryValue`. This is O(result size) encode + decode + full
re-allocation, TWICE, per call — and per ITERATION at the ForEach site.

## THE REAL RISK IN THIS TASK — READ BEFORE WRITING ANY CODE

**A naive hand-written `QueryResult → QueryValue` conversion is a trap.**
`QueryResult.records: Vec<QueryRecord>` where `QueryRecord`
(`crates/shamir-query-types/src/read/query_record.rs`) has THREE variants
with DIFFERENT, NON-OBVIOUS serialization behavior:

```rust
impl Serialize for QueryRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryRecord::Direct(v) => v.serialize(s),
            QueryRecord::Inserted(rec) => rec.serialize(s),
            QueryRecord::IdBytes(b) => b.serialize(s),  // msgpack `bin`
        }
    }
}
```

- `IdBytes(b)` serializes as msgpack **binary** — round-tripped through
  `QueryValue`'s deserializer this becomes `QueryValue::Bin(...)`.
  **`QueryRecord::as_value()` (lines 183-191 of that file) returns
  `QueryValue::Null` for `IdBytes` instead** — a COMPLETELY DIFFERENT
  result. **Do NOT use `as_value()` as the basis for this conversion** — it
  has different semantics than the real wire format, specifically for this
  variant. Confirm this yourself by reading both the `Serialize` impl and
  `as_value()` side by side before writing anything.
- `Inserted(rec)` delegates to `InsertedRecord::serialize`
  (`crates/shamir-query-types/src/write/inserted_record.rs`), which is
  **non-trivial**: when `fields` is a `Map`, it interleaves an `"_id"` key
  IN SORTED POSITION among the other field keys (walking a
  sorted-by-key-string pairs list and inserting `_id` at its alphabetic
  slot) — NOT simply "fields map + an id field appended". There is also a
  separate branch for a non-`Map` `fields` value (see the test named
  `no_id_non_map_value_direct_serialization` in
  `crates/shamir-query-types/src/write/tests/inserted_record_tests.rs` for
  what that shape looks like). **`as_value()`'s `Inserted` arm
  (`rec.fields.clone()`) drops this `_id`-interleaving entirely** — another
  divergence from the real wire format.

**Do not solve this by copy-pasting `InsertedRecord`'s sorted-key logic
into a second, hand-maintained place** — that creates exactly the
maintenance hazard (two implementations of the same wire shape, one of
which nobody remembers to keep in sync) this campaign's "one file = one
primary export" / DRY discipline exists to prevent.

## Three acceptable implementation strategies — pick based on your own risk assessment; the differential test (Verification, below) is what actually proves correctness, not which strategy you pick

**Strategy A (RECOMMENDED if you can build it correctly): a minimal
`serde::Serializer` whose `Ok` type is `QueryValue`.** This reuses the
EXISTING `Serialize` impls of `QueryResult`/`QueryRecord`/`InsertedRecord`/
`QueryStats`/`PaginationInfo`/`ExplainPlan` VERBATIM (calling
`inner_results.serialize(&mut this_serializer)` instead of
`rmp_serde::Serializer`) — so `Inserted`'s sorted-key-with-`_id`
interleaving, `IdBytes`'s bin-encoding, and any FUTURE field added to any of
these types all flow through CORRECTLY with zero hand-maintained mirror
code, because you are not duplicating logic, you are redirecting the SAME
serialization calls to build a `QueryValue` tree in memory instead of a
byte buffer. This is the same pattern `serde_json::to_value` uses for
`serde_json::Value`, or `serde_yaml`'s `to_value`, or (as a workspace
precedent for style, not for scope) the trivial `ToWire::to_query_value`
blanket impl at `crates/shamir-query-builder/src/wire/mod.rs:31` (which
still round-trips through bytes — read it for the CONCEPT of "any
`T: Serialize` can become a `QueryValue`", not as a solution, since it
doesn't remove the round-trip). You do NOT need to implement the FULL
generic serde data model — only the subset of `Serializer` trait methods
that `QueryResult`/`QueryRecord`/`InsertedRecord`/`QueryStats`/
`PaginationInfo`/`ExplainPlan`/`Decimal`/`BigInt`'s `Serialize` impls
actually call (map/struct/seq/str/bytes/bool/i64/f64/none/some/unit-ish
paths) — inspect what they call and implement exactly that surface;
methods you can prove are never reached may `unimplemented!()` with a
clear message.

**Strategy B (pragmatic middle ground, LOWER RISK, smaller diff): fast-path
the common case, fall back to the byte round-trip for anything else.**
Inspect `inner_results` first: if every `QueryResult` in the map has
`records` containing ONLY `QueryRecord::Direct` entries (the overwhelmingly
common ForEach body shape — a `SELECT`/read-style body), build the
`QueryValue` directly (map of alias → object with `records`/`stats`/
`pagination`/`value`/`explain`/`skipped` keys, each field mapped straight
across since `Direct`, `QueryStats`, `PaginationInfo`, `ExplainPlan` have
NO custom `Serialize` tricks — verify this yourself by reading their derive
attributes, they're plain `#[derive(Serialize)]` with only
`skip_serializing_if`/`default`, no custom impl). If ANY record is
`Inserted`/`IdBytes`, or you're not confident about an edge case, fall back
to the EXACT existing `rmp_serde::to_vec_named` + `from_slice` call for
that one `QueryResult` (or the whole map, your choice) — this NEVER
produces a wrong answer (the fallback is the exact code that runs today),
it just captures less of the perf win on the rarer write-result-in-ForEach
shape. State clearly in your summary which shapes take the fast path and
which fall back.

**Strategy C (do NOT use): hand-mirroring every type's exact wire shape
with bespoke per-field code, including reimplementing `InsertedRecord`'s
sorted-key/`_id`-interleaving logic by hand.** This is the trap described
above — a second, hand-maintained copy of nontrivial serialization logic
that WILL drift from the original over time. If Strategy A feels too large
and Strategy B's fallback coverage feels too narrow, that is a signal to
lean further into B's fallback (cover fewer shapes with the fast path, not
more with hand-mirrored logic), not to reach for C.

**Whichever strategy you choose**, replace BOTH call sites (`BatchOp::Batch`
line ~467-469 AND the `ForEach` loop line ~676-679) with it — they are the
identical pattern, no reason to leave one on the old path once you have a
tested replacement.

## Item (b) — compiled-body cache (OPTIONAL, lower priority — only if capacity remains after item (a) is done and verified)

The report also flags (F5, second half) that each `ForEach` iteration
re-plans and re-compiles the identical loop body (`compile_filter` per
WHERE, `SelectProjection::new` including its prescans, `pre_intern_select_keys`,
planner probes) — all invariant across iterations except the injected
`$param`. A full fix (hoisting a compiled-body cache keyed by op index,
threaded through the recursive `execute_batch_impl`/
`run_nested_body_in_outer_tx` calls) is a substantially larger, more
invasive change than item (a) — touching the recursive execution plumbing
itself, not a leaf-level conversion function. **Do NOT attempt this unless
item (a) is fully done, tested, and you have clear remaining capacity and
confidence.** If you don't attempt it, say so plainly and move on — this
is explicitly acceptable, matching the report's own "P1 if capacity"
framing for this half of F5.

## Verification (MANDATORY before you report done — this is the load-bearing safety net for this whole task)

- **The decisive differential test**: for a representative set of
  `TMap<String, QueryResult>` shapes, run BOTH the OLD path
  (`rmp_serde::to_vec_named` + `from_slice::<QueryValue>`) AND your NEW
  path on the SAME input, and assert the resulting `QueryValue`s are
  `PartialEq`-identical. Cover, at minimum:
  - a `QueryResult` with `records` = a mix of `QueryRecord::Direct` (nested
    Map/List/scalar shapes), `QueryRecord::Inserted` (BOTH with an `id` set
    and without — exercise the sorted-key `_id`-interleaving path with a
    field name that sorts BEFORE `"_id"` alphabetically and one that sorts
    AFTER, so the interleaving logic's midpoint-insert and both-ends cases
    are both exercised), and `QueryRecord::IdBytes` (confirm it becomes
    `QueryValue::Bin(...)`, NOT `Null`);
  - `stats`/`pagination`/`value`/`explain` each present and each absent
    (`None`) — confirms `skip_serializing_if` semantics are preserved (a
    `None` field must NOT appear as a key with a null value if your new
    path builds a `QueryValue::Map` directly — verify this against the OLD
    path's actual output for a `None` case, don't assume);
  - `skipped: true` and `skipped: false` (note `skip_serializing_if =
    "std::ops::Not::not"` — `false` is the default and is OMITTED from the
    old wire shape; your new path must match, not just "represent bool
    correctly");
  - an empty `TMap<String, QueryResult>` (empty ForEach-body-result edge
    case) and a `TMap` with 2+ aliases.
  - If you took Strategy B, this differential test is what proves the
    fallback triggers correctly AND that its output matches the fast path's
    output where they overlap (same records shape through both routes must
    agree).
- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green
  — run it TWICE (this session's flake-triage discipline). Any existing
  `for_each`/`ForEach` test (grep for it) exercising `$query @alias...`
  references into the loop's per-iteration results must still pass
  UNCHANGED.
- `cargo fmt -p shamir-engine -p shamir-query-types -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above, which strategy
  (A/B) you chose and why, and whether you attempted item (b).

## Out of scope

- Do NOT touch tasks 8a-8d's artifacts (already landed, different
  files/crates) or F6/F9/F10/F11 (task 8f).
- Do NOT change `QueryRecord`/`InsertedRecord`/`QueryResult`'s actual
  `Serialize`/`Deserialize` impls — this task only changes HOW the engine
  converts an already-built `TMap<String, QueryResult>` into a `QueryValue`
  for internal `$query` resolution, never the wire format itself.
- Do NOT attempt item (b) (compiled-body cache) unless item (a) is fully
  done and verified first, per the explicit permission above.
