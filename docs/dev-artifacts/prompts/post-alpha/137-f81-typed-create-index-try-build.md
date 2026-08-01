# F-81 (#908) — typed CreateIndex builders + validating try_build + parity fixtures

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`crates/shamir-query-builder/src/ddl/create_index.rs` already has a fluent
`create_index(name, table) -> CreateIndex` builder
(`.field()`/`.fields()`/`.unique()`/`.sorted()`/`.repo()`/`.index_type()`/
`.fts_tokenizer()`/`.fts_language()`/`.functional_op()`/
`.functional_args()`/`.vector_dim()`/`.vector_metric()`/
`.vector_quantization()`/`.include()`/`.if_not_exists()`), finalized by
`build(self) -> BatchOp` (~line 150) — **infallible, no validation**.

The doc comment on `CreateIndexOp`
(`crates/shamir-query-types/src/admin/types/index_ops.rs:21`) claims
"`unique=true` + `sorted=true` is rejected" — but that check ONLY exists
in the server execution path,
`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:386-390`:

```rust
if op.sorted && op.unique {
    return Err(err("Index cannot be both sorted and unique".to_string()));
}
if !op.include.is_empty() && !op.sorted {
    return Err(err("include is only valid for sorted indexes".to_string()));
}
if op.sorted && op.fields.len() != 1 {
    return Err(err("Sorted index requires exactly one field (composite TBD)".to_string()));
}
```

So a caller using `CreateIndex::build()` can construct an invalid op and
only find out at DDL-execution time — a full round trip through the
server — instead of at construction time. CLAUDE.md's "Database queries
are always built through a query builder" rule (see its "🏗️ Query
construction — builder only" section) exists precisely so the builder is
the authoritative, validating gate; right now it is not, for this one op.

**The TS client already validates this at construction time** —
`crates/shamir-client-ts/src/core/builders/ddl.ts:178-183` rejects
`unique && sorted` synchronously. Rust's builder is behind the TS
builder's validation for the exact same op — this is the gap F-81 closes.

## Style template — mirror this exactly, don't invent a new shape

Two existing `try_build()` splits in this crate are the precedent to
follow, do not deviate stylistically:

1. `crates/shamir-query-builder/src/query/query.rs` — `Query::build()`
   (~line 337, stays infallible/permissive for back-compat) vs
   `Query::try_build()` (~line 355, runs checks then delegates to a
   shared private `build_inner()`), with a dedicated error enum
   `crates/shamir-query-builder/src/query/query_build_error.rs`
   (`QueryBuildError`, `Display` + `std::error::Error` impls, `thiserror`
   per CLAUDE.md's error-handling section).
2. `crates/shamir-query-builder/src/batch/batch.rs` —
   `Batch::try_build() -> Result<BatchRequest, BuildError>` (~line 793),
   error type in `batch/build_error.rs`.

## What to build

### 1. `CreateIndexBuildError` (new, `thiserror`-derived enum)

One variant per invalid-combination class the server already enforces,
plus room for anything the vector family needs (see step 2). Variants at
minimum: `UniqueAndSorted`, `IncludeWithoutSorted`,
`SortedMultiField { field_count: usize }`. Follow `QueryBuildError`'s
`Display` message style closely enough that a caller reading the error
text recognizes it as sibling API, not a one-off.

### 2. `CreateIndex::try_build(self) -> Result<BatchOp, CreateIndexBuildError>`

Runs the exact three checks from `admin_table_index.rs:386-390`
client-side, in the same order, with equivalent (not necessarily
byte-identical, but semantically equivalent) messages, then delegates to
the existing `build()` (or a shared private inner fn) for the actual
`BatchOp` construction — do not duplicate the `BatchOp`-building logic.

Before finalizing the vector-family variant list, read
`create_index_v2`'s validation in `table_manager_index_mgmt.rs` (the
report flagged this as unconfirmed) and `index_type: "vector"`'s actual
required fields (is `vector_dim` mandatory when `index_type == "vector"`?
what about `vector_metric`?) — if the server enforces a vector-specific
requirement client-side validation is currently missing entirely, add the
corresponding `try_build()` check and error variant too, so `try_build()`
is a genuine superset of `build()`'s blind-spot, not just the three
pre-existing checks copied verbatim.

**Leave `build()` alone** — it stays infallible/permissive, exactly like
`Query::build()`/`Batch::build()` do. Do not break any existing caller of
`CreateIndex::build()`.

### 3. Parity fixtures

Follow the existing pattern in
`crates/shamir-query-builder/tests/repl_ddl_msgpack.rs` +
`crates/shamir-query-builder/tests/fixtures/repl_ddl_msgpack.json` (and
the sibling `vector_filter_msgpack.rs`/`.json`): build each of the
distinct valid `CreateIndex` shapes (regular, unique, sorted+include,
fts, functional, vector) via the builder, assert the msgpack bytes match
a checked-in fixture, and assert decode round-trips through the same
`QueryValue`-mediated path the server uses. Add a NEW fixture file
(don't overwrite `repl_ddl_msgpack.json`) scoped to `try_build()`'s valid
outputs, named to make its scope obvious (e.g.
`tests/fixtures/create_index_try_build_msgpack.json`).

Additionally — this is the "parity" the task name specifically calls
out — add test cases proving `try_build()` REJECTS every invalid
combination `admin_table_index.rs` rejects (one test per check: unique+
sorted, include-without-sorted, sorted-multi-field, and whatever new
vector-family check you found in step 2), asserting the specific error
variant. This is the client/server parity: same invalid input, same
rejection, now enforced twice (defense in depth) instead of once.

## Definition of done

- `CreateIndexBuildError` (new file or extending `ddl/` module structure
  per this repo's "one file = one primary export" rule — check whether it
  needs its own file alongside `create_index.rs`).
- `CreateIndex::try_build()` added; `build()` unchanged and still used by
  any existing caller.
- New parity-fixture test file + checked-in fixture JSON, covering both
  valid-shape wire-format pinning AND invalid-combination rejection.
- `cargo fmt -p shamir-query-builder -p shamir-query-types -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-query-builder -p shamir-query-types --full`
  green.
- Report in the commit message: the exact list of `try_build()` checks
  implemented, whether you found and added a vector-family check beyond
  the three pre-existing server checks (or confirmed there isn't one),
  and the fixture file(s) added.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
