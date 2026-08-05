# Brief — #1004 (part of #1001): restore lost vector_dim boundary test coverage from #998's deleted file

Task: #1004 in the session TaskList (last decomposed leaf of the former
#1001 umbrella). Small, self-contained addition to the #998 fixture matrix.

## Background

The deleted `create_index_try_build_msgpack.rs` (509 lines, removed by
#998's consolidation) had two `vector_dim` boundary tests —
`try_build_rejects_vector_zero_dim` (explicit `vector_dim(0)`) and
`try_build_accepts_vector_dim_one` — neither has an equivalent in the new
`crates/shamir-query-builder/tests/fixtures/create_index_matrix.json` (the
matrix's existing `vector_dim_required_rejected` case only covers the
OMITTED-dim case, not an explicit `vector_dim: 0`). This leaves the
`self.vector_dim == Some(0)` branch of
`crates/shamir-query-builder/src/ddl/create_index.rs`'s validation with
zero test coverage anywhere in `shamir-query-builder`.

## The ask

Add two new rows to `crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`'s
`cases` array (follow the existing JSON shape/style exactly — read the
file first, several similar vector cases already exist as a template):

1. **`vector_dim_zero_rejected`** (or similar name, be consistent with the
   existing naming style): `{ "name": "idx_bad", "table": "docs", "fields":
   [["embedding"]], "index_type": "vector", "vector_dim": 0 }`, `"expect":
   "reject"`, `"reason_contains": "vector_dim"` (matching the existing
   `vector_dim_required_rejected` case's convention — confirm the exact
   substring against `CreateIndexBuildError::VectorDimRequired`'s real
   `Display` text before finalizing).
2. **`vector_dim_one_accepted`** (or similar): a valid minimal vector index
   with `vector_dim: 1`, e.g. `{ "name": "idx_vec_min", "table": "docs",
   "fields": [["embedding"]], "index_type": "vector", "vector_dim": 1 }`,
   `"expect": "accept"`, with a real `wire_hex` — generate it using the
   EXISTING `generate_accept_case_hex` test in
   `crates/shamir-query-builder/tests/create_index_matrix.rs` (per that
   test's own doc comment: run it with `--nocapture` to print the hex for
   every accept case, including your new one once it's in the fixture with
   a placeholder/empty `wire_hex`, then paste the generated value in).

Both the Rust test (`create_index_matrix.rs`) and the TS test
(`crates/shamir-client-ts/src/core/builders/__tests__/create_index_matrix.test.ts`)
read the SAME fixture file, so adding these two rows should automatically
extend both test surfaces — you should NOT need to touch either `.rs` or
`.test.ts` file. If you find you DO need to touch one of them, stop and
reconsider — that's a signal the fixture schema doesn't already support
what you're adding, which would be surprising given how many similar vector
cases already exist.

## Also: one documentation note

Add a one-line note to the fixture's `_comment` (or a new small
`_key_order_note`-style field, your call) for a future reader: the Rust and
TS validators currently run their rejection-rule checks in a DIFFERENT
order (confirm this yourself by comparing
`crates/shamir-query-builder/src/ddl/create_index.rs`'s `TryFrom<&CreateIndex>
for IndexSpec` check order against `crates/shamir-client-ts/src/core/builders/ddl.ts`'s
numbered checks). No current matrix case triggers two simultaneous
violations with different winners across the two languages, but a future
multi-violation case could diverge silently. This is informational only —
do not attempt to unify the check order, that's out of scope here.

## Scope discipline

- Fixture JSON changes only, plus the one doc note. Do not touch
  `create_index_matrix.rs`, `create_index_matrix.test.ts`,
  `create_index.rs`, or `ddl.ts` unless you find a genuine reason (see
  above) — and if so, stop and explain rather than proceeding silently.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder --full
```

Then, from `crates/shamir-client-ts/`:
```
npx vitest run src/core/builders/__tests__/create_index_matrix.test.ts
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only / test /
gate commands.

## What to report back

- The two new fixture rows (paste them).
- Confirmation neither `.rs` nor `.test.ts` needed changes (or, if one did,
  exactly why and what you changed).
- Confirmation of the check-order note you added.
- Exact gate command output (Rust + the one TS test file), including the
  updated total accept/reject case counts (should now be 10 accept + 13
  reject = 23 total, up from 9+12=21).
