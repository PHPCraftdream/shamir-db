# Brief — #986: typed `IndexSpec` enum for the `CreateIndex` builder (internal only)

Task: #986 in the session TaskList. Follow-up from #970 (P1-6). Split from
the original #986 by #998 (already done, merged — its fixture matrix is
your regression safety net; do not touch it beyond reading it).

## What already exists — read these first, do not re-derive from memory

1. **`crates/shamir-query-types/src/admin/types/index_ops.rs:23-82`** — the
   wire type `CreateIndexOp`. 16 fields, in this EXACT declaration order
   (this order is the msgpack wire key order — `rmp_serde::to_vec_named`
   emits struct fields in declaration order, not alphabetically):
   `create_index: String`, `table: String`, `fields: Vec<Vec<String>>`,
   `unique: bool` (`#[serde(default)]`), `sorted: bool`
   (`#[serde(default)]`), `repo: String`
   (`#[serde(default = "default_repo")]`, always present), `index_type:
   Option<String>`, `fts_tokenizer: Option<String>`, `fts_language:
   Option<String>`, `functional_op: Option<String>`, `functional_args:
   Option<Vec<QueryValue>>`, `vector_dim: Option<u32>`, `vector_metric:
   Option<String>`, `vector_quantization: Option<String>`, `include:
   Vec<Vec<String>>` (`#[serde(default, skip_serializing_if =
   "Vec::is_empty")]`), `if_not_exists: bool` (`#[serde(default,
   skip_serializing_if = "is_false")]`).

   **THIS STRUCT'S WIRE SHAPE MUST NOT CHANGE.** Field names, types, order,
   serde attributes — all frozen. This is the hard constraint the whole
   task exists to respect.

2. **`crates/shamir-query-builder/src/ddl/create_index.rs`** — the `CreateIndex`
   fluent builder. `build()` (line ~275, infallible, permissive — stays
   exactly as-is) constructs `BatchOp::CreateIndex(CreateIndexOp { ... })`
   directly from the builder's own flat fields (which mirror `CreateIndexOp`'s
   shape). `try_build()` (lines 196-271) runs 12 validation checks in this
   exact order before calling `build()`:
   1. `EmptyFields` — `self.fields.is_empty()`
   2. `UniqueUnsupportedForType` — `self.unique && non_btree`
   3. `SortedUnsupportedForType` — `self.sorted && non_btree`
   4. `VectorDimRequired` — vector type + (`None` or `Some(0)`) dim
   5. `UnknownVectorMetric` — vector metric not in `["l2","dot","cosine"]`
   6. `VectorOptionsOnNonVectorIndex`
   7. `FtsOptionsOnNonFtsIndex`
   8. `FunctionalOptionsOnNonFunctionalIndex`
   9. `IncludeUnsupportedForType`
   10. `UniqueAndSorted` — `self.sorted && self.unique`
   11. `IncludeWithoutSorted` — `!self.include.is_empty() && !self.sorted`
   12. `SortedMultiField` — `self.sorted && self.fields.len() != 1`

3. **`crates/shamir-query-builder/src/ddl/create_index_build_error.rs:21-108`**
   — `CreateIndexBuildError`, the 12 variants above, one per check. Every
   variant's `Display` text is what #998's fixture matrix's
   `reason_contains` substrings match against — DO NOT change wording,
   only where the check is computed.

4. **`crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`**
   (21 cases: 9 accept, 12 reject — one per `CreateIndexBuildError` variant)
   **+ `crates/shamir-query-builder/tests/create_index_matrix.rs`
   + `crates/shamir-client-ts/…/create_index_matrix.test.ts`** — the shared
   fixture matrix from #998. This is your regression oracle: every accept
   case's `wire_hex` must still match byte-for-byte, every reject case must
   still fail with the same `reason_contains` substring, before AND after
   your refactor. Do not edit this matrix (task #1004, separately queued,
   adds two more rows to it later — not your concern here).

5. **The server-side handler**
   (`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:311-516`,
   `handle_create_index`) independently re-implements the SAME 12 checks
   (lines 403-493) before dispatching to
   `create_index`/`create_unique_index`/`create_sorted_index_with_include`/
   `create_index_v2`. **This is explicitly OUT OF SCOPE for this task** — do
   not touch this file. If you notice the duplication is worth eliminating
   in a follow-up (sharing `IndexSpec` server-side too), say so in your
   report, but do not act on it here.

## The actual ask

Introduce a new internal enum, `IndexSpec`, that makes the mutually-exclusive
combinations `try_build()` currently checks by hand (bool flags + string
`index_type` + a pile of `Option<_>` side-fields) **unrepresentable at the
type level** — i.e. the compiler, not a runtime `if`, rules out e.g. a
`Sorted` variant carrying `vector_dim`.

**Do not copy the enum sketch from the original task description
verbatim** — it names a `ranking` field on `Fts` and an `expression` field
on `Functional` that DO NOT EXIST anywhere in the current code (grep
confirms: only `fts_tokenizer`/`fts_language`, and `functional_op` +
`functional_args`). That sketch was illustrative shorthand, not a spec.
Ground every field in `IndexSpec` in what `CreateIndexOp`/the builder
ACTUALLY carry today — do not invent new wire concepts.

A shape consistent with the real fields (adjust as you see fit, but justify
any deviation in your report):

```rust
enum IndexSpec {
    Hash { fields: Vec<Vec<String>>, unique: bool },
    Sorted { field: Vec<String>, include: Vec<Vec<String>> },
    Fts { field: Vec<String>, tokenizer: Option<String>, language: Option<String> },
    Functional { field: Vec<String>, op: String, args: Option<Vec<QueryValue>> },
    Vector { field: Vec<String>, dim: NonZeroU32, metric: VectorMetric, quantization: Option<String> },
}
```

Notes on the sketch above (your judgement call, not a mandate):
- `Sorted`'s `field: Vec<String>` (singular) rather than `fields:
  Vec<Vec<String>>` naturally encodes `SortedMultiField` — a sorted index
  is single-field by construction, so the variant simply cannot hold more
  than one. Converting FROM the builder's `Vec<Vec<String>>` must still
  produce `Err(SortedMultiField { field_count })` when there's more than
  one, with the correct count in the error.
- Putting `include` only on `Sorted` naturally encodes
  `IncludeUnsupportedForType`/`IncludeWithoutSorted` for every OTHER
  variant (they have no `include` field at all) — but the FROM-conversion
  must still detect "caller asked for `.unique()` and passed `.include(...)`"
  and reject with the correct existing error, not silently drop the data.
- `VectorMetric` as a small enum (`L2`, `Dot`, `Cosine`) instead of a raw
  `String` would make `UnknownVectorMetric` fall out of a
  `TryFrom<&str>`/`FromStr` impl naturally — optional, use your judgement;
  a plain validated `String` is also acceptable if you'd rather not add a
  new public type.
- `dim: NonZeroU32` naturally encodes `VectorDimRequired` (can't construct
  the variant with a zero or absent dim) — this one is the clearest win in
  the whole enum and should definitely be `NonZeroU32`, not `Option<u32>`.
- `name`/`table`/`repo`/`if_not_exists` deliberately do NOT appear in
  `IndexSpec` — they're orthogonal metadata that apply the same regardless
  of index kind, not part of "what kind of index is this". Keep them as
  separate parameters alongside the spec when reassembling the final
  `CreateIndexOp`.

**Conversion layer (both directions required):**
- `TryFrom<&CreateIndex> for IndexSpec` (or an equivalent method on the
  builder) — this is where `try_build()`'s validation logic MOVES TO. It
  must produce the exact same `Ok`/`Err` decision, with the exact same
  `CreateIndexBuildError` variant (and same embedded data, e.g.
  `SortedMultiField { field_count }`'s count), for every one of the 21
  cases in `create_index_matrix.json` — this is what the existing
  `matrix_all_accept_cases_build_and_match_wire_hex` /
  `matrix_all_reject_cases_fail_with_reason` tests already assert; they
  must stay green, unmodified, as your regression proof.
- `From<IndexSpec> for CreateIndexOp` (needs `name`/`table`/`repo`/
  `if_not_exists` passed in some way — a plain function taking all four
  plus the spec is fine, doesn't need to literally be the `From` trait if
  the extra params make that awkward) — infallible, just flattens the
  validated spec back into the wire shape. This is what makes the byte-
  identical `wire_hex` assertions in the fixture matrix keep passing: if
  this flattening reproduces the exact same field values in the exact same
  struct, the serialized bytes are unchanged by construction.
- `try_build()` becomes: build an `IndexSpec` (bubbling `?` on failure),
  then flatten it back into a `CreateIndexOp`, wrapped in
  `BatchOp::CreateIndex(...)`. `build()` (infallible) is UNCHANGED — it does
  not need to round-trip through `IndexSpec` at all, since it doesn't
  validate anything today and shouldn't start.

## Where `IndexSpec` lives

Scope this to `shamir-query-builder` only (e.g. a new file
`crates/shamir-query-builder/src/ddl/index_spec.rs` — this repo's
"one file = one primary export" convention means `IndexSpec` gets its own
file, separate from `create_index.rs`/`create_index_build_error.rs`). Do
NOT move it to `shamir-query-types` or wire it into the server — that
widens scope beyond what's asked and risks touching
`admin_table_index.rs`'s already-independently-tested duplicate logic.

## Hard requirements (all three are release-blocking for THIS task)

1. **`CreateIndexOp`'s wire shape is byte-identical before and after.**
   Proven by #998's fixture matrix's `wire_hex` assertions staying green,
   unmodified.
2. **The ~18 test files that construct `CreateIndexOp { ... }` as flat
   struct literals (50 literal constructions across 20 files, per this
   session's grep — see list below) must NOT need to change.** If any of
   them breaks, that is a signal you've changed the wire DTO's shape, which
   violates requirement 1 — stop and reconsider, don't patch the tests to
   compensate.
   ```
   crates/shamir-query-types/src/admin/types/tests/create_index_op_tests.rs (6)
   crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs (1)
   crates/shamir-engine/src/table/tests/f50_index_lifecycle_spike_tests.rs (2)
   crates/shamir-engine/src/table/tests/f50_step2_index_lifecycle_tests.rs (4)
   crates/shamir-engine/src/table/tests/f76_drop_visibility_tests.rs (2)
   crates/shamir-engine/src/table/tests/filtered_ann_tests.rs (2)
   crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs (2)
   crates/shamir-engine/src/table/tests/index2_empty_result_tests.rs (3)
   crates/shamir-engine/src/table/tests/index2_lifecycle_state_tests.rs (2)
   crates/shamir-engine/src/table/tests/index2_migration_tests.rs (3)
   crates/shamir-engine/src/table/tests/index2_persistence_tests.rs (2)
   crates/shamir-engine/src/table/tests/multi_vector_index_guard_tests.rs (4)
   crates/shamir-engine/src/table/tests/p03b_index2_drop_durability_tests.rs (2)
   crates/shamir-engine/src/tx/tests/commit_phase5_tests.rs (4)
   crates/shamir-engine/src/tx/tests/f73_rederive_fail_closed_tests.rs (2)
   crates/shamir-engine/src/tx/tests/index_rollback_tests.rs (1)
   crates/shamir-engine/src/tx/tests/tx_vector_delete_tests.rs (2)
   crates/shamir-engine/tests/crash_recovery.rs (4)
   ```
3. **#998's fixture matrix (`create_index_matrix.json` +
   `create_index_matrix.rs` + the TS mirror) stays green, UNCHANGED.** Do
   not edit the fixture or its consumer tests as part of this task (a
   separate queued task, #1004, extends it later).

## New tests to add (this task's own regression coverage)

Add tests in a new `crates/shamir-query-builder/src/ddl/tests/index_spec_tests.rs`
(wired in via the existing `tests/mod.rs` manifest pattern this repo uses —
check `crates/shamir-query-builder/src/ddl/` for whether a `tests/` dir
already exists there and follow its conventions, or create one per this
repo's test-organization rules in CLAUDE.md):
- One test per `IndexSpec` variant proving a valid construction succeeds
  and round-trips to the expected `CreateIndexOp`.
- One test per invalid combination proving `TryFrom` rejects with the
  correct `CreateIndexBuildError` variant (this can substantially overlap
  with what the matrix already covers — that's fine, this is testing the
  NEW internal conversion function directly, not just its outward effect
  through `try_build()`).
- A test that `NonZeroU32` genuinely makes `vector_dim: 0` unrepresentable
  at construction (i.e. you cannot even ATTEMPT to build
  `IndexSpec::Vector { dim: 0, .. }` — it won't compile with a literal `0`,
  or `NonZeroU32::new(0)` returns `None` and the conversion must map that to
  `VectorDimRequired`).

## Scope discipline

- Do NOT touch `admin_table_index.rs` (the server-side duplicate
  validation) — report the duplication as a follow-up observation only.
- Do NOT touch `CreateIndexOp`'s definition, field names, types, order, or
  serde attributes.
- Do NOT touch #998's fixture matrix files.
- Do NOT touch the 20 test files with flat `CreateIndexOp { .. }` literals
  — if your change forces you to touch even one, STOP, you've broken
  requirement 1, reconsider your approach before continuing.
- `build()` (infallible) stays infallible and unchanged in behavior.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder --full
./scripts/test.sh -p shamir-query-types -p shamir-engine --full
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

Then, from `crates/shamir-client-ts/`:
```
npx vitest run src/core/builders/__tests__/create_index_matrix.test.ts
```
(confirms the TS-side wire-hex fixtures — which read the SAME matrix file —
are still byte-identical, i.e. nothing on the Rust side silently drifted
the wire shape.)

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only / test /
gate commands.

## What to report back

- The final `IndexSpec` shape you landed on, and why (including any
  deviation from the sketch above).
- Confirmation, explicitly, that none of the 20 files with flat
  `CreateIndexOp { .. }` literals needed to change.
- Confirmation that #998's fixture matrix tests pass UNMODIFIED (paste the
  test names and pass/fail).
- Whether you found the server-side (`admin_table_index.rs`) validation
  duplication worth a follow-up task — don't fix it, just report.
- Exact gate command output (Rust + the one TS test file).
