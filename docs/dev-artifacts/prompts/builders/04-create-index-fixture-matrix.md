# Brief — #998: shared declarative CREATE INDEX fixture matrix (Rust + TS)

Task: #998 in the session TaskList. Split out of #986 (the larger, post-alpha
`IndexSpec` enum redesign) because this half is independently valuable, needs
no wire change, and gives #986 its regression safety net. Read this brief in
full — a real Rust/TS behaviour DIVERGENCE has already been found this
session (below); do not silently paper over it without reporting.

## What already exists — read these first, do not duplicate

1. **`crates/shamir-query-builder/src/ddl/create_index.rs`**'s
   `CreateIndex::try_build()` — the Rust validation path, returning
   `CreateIndexBuildError` (defined in
   `crates/shamir-query-builder/src/ddl/create_index_build_error.rs`). 12
   distinct error variants: `UniqueAndSorted`, `IncludeWithoutSorted`,
   `SortedMultiField { field_count }`, `EmptyFields`,
   `UniqueUnsupportedForType`, `SortedUnsupportedForType`,
   `VectorDimRequired`, `UnknownVectorMetric`,
   `VectorOptionsOnNonVectorIndex`, `FtsOptionsOnNonFtsIndex`,
   `FunctionalOptionsOnNonFunctionalIndex`, `IncludeUnsupportedForType`.
2. **`crates/shamir-client-ts/src/core/builders/ddl.ts`**'s `createIndex()` —
   the TS validation path (throws `Error` synchronously), ~9 numbered checks
   in the function body (unique+sorted mutual exclusion, empty fields,
   unique/sorted-unsupported-for-type, vector_dim required, unknown
   vector_metric, vector-options-on-non-vector, fts-options-on-non-fts,
   functional-options-on-non-functional, include-unsupported-for-type,
   include-without-sorted).
3. **`crates/shamir-query-builder/tests/create_index_try_build_msgpack.rs`**
   + **`crates/shamir-query-builder/tests/fixtures/create_index_try_build_msgpack.json`**
   — an EXISTING hex-encoded msgpack wire-contract fixture for the 6 distinct
   VALID `CreateIndex` shapes (regular, unique, sorted_include, fts,
   functional, vector). Rust-only, `try_build().unwrap()` + `to_vec_named`.
   This is the closest existing thing to what #998 asks for — study its
   `_comment`/`_key_order_note`/`_value_notes` fields, they document real,
   nonobvious wire-encoding facts (bool defaults emit `0xc2` not omission,
   `repo` always present, vector_dim uint16 encoding, etc.) that your new
   matrix must preserve or explicitly supersede.

## A real divergence — already found, verify then report/fix

**`SortedMultiField` has NO TS-side check.** Rust's `try_build()` rejects a
`.sorted()` index with more than one field
(`CreateIndexBuildError::SortedMultiField { field_count }`, doc comment:
"Sorted indexes are single-field scalar columns only"). Grep
`crates/shamir-client-ts/src/core/builders/ddl.ts` for any check on
`fields.length` combined with `sorted` — there is none. A caller building
`createIndex('x', 't', [['a'], ['b']], { sorted: true })` in TS gets NO
client-side rejection and only fails at the server round-trip (or possibly
succeeds if the server's own check has the same gap — verify this too by
reading `admin_table_index.rs`'s sorted-multi-field check).

Contrast with `UniqueAndSorted`, whose Rust doc comment explicitly says "the
TS builder rejects it synchronously in `ddl.ts`" — confirming this one IS
mirrored. `SortedMultiField`'s doc comment does NOT make that claim — a
documentation-level hint the gap is real, not just a grep miss.

**Do not silently patch this inline as a side-fix.** Build the fixture
matrix first (it will independently prove the gap: a `sorted` + 2-fields
case will show Rust rejects, TS's `build()` doesn't). Once the matrix
confirms it, add the missing TS check as PART of this task (it's a small,
obviously-scoped fix once the matrix names it precisely) — but do the
discovery-via-matrix step for real, don't just eyeball the code and patch
it without the regression test proving before/after.

## Required deliverable

**ONE declarative fixture matrix** — a data file (JSON, following the shape
of the existing `create_index_try_build_msgpack.json` closely enough that a
future reader recognizes the lineage; TOML/YAML are also acceptable if you
have a strong reason, but JSON avoids adding a new parser dependency to
either toolchain) listing CREATE INDEX cases:

- **Valid cases** (must build successfully on both sides) — at minimum, port
  forward the existing 6 from `create_index_try_build_msgpack.json`, plus
  any additional valid shapes needed to reach full option coverage (e.g. a
  vector case with `vector_quantization: "sq8"`, an fts case with
  `fts_language` set, a plain `if_not_exists: true` case).
- **Invalid cases** (must be REJECTED on both sides, with an identifiable
  reason) — one entry per `CreateIndexBuildError` variant (12) — including
  `SortedMultiField`, the newly-found gap. Also cover every numbered TS
  check from `ddl.ts` to confirm each has a Rust-side counterpart (cross-
  check in BOTH directions, not just Rust→TS).

Each matrix entry needs enough structure for BOTH toolchains to consume
generically:
```
{
  "name": "sorted_multi_field_rejected",
  "input": { "table": "users", "fields": [["a"], ["b"]], "sorted": true },
  "expect": "reject",
  "reason_contains": "sorted"   // substring both sides' error messages must contain, case-insensitive — do not require exact string equality, Rust and TS error text legitimately differ in wording
}
```
For `"expect": "accept"` cases, also carry the expected wire shape somehow —
either inline the hex (mirroring the existing fixture's approach) or a
structured expected-fields object both languages can independently encode
and compare against. Use your judgement on which is less brittle; justify
the choice in your report. If you keep the hex-wire-contract approach,
merge the 6 existing hex strings into your new matrix (single source of
truth) rather than maintaining both files — that duplication is exactly what
this task exists to eliminate.

## Consumers (both required)

1. **Rust test** reading the matrix, driving `CreateIndex::try_build()` for
   each case, asserting accept/reject + wire shape (if applicable) +
   `reason_contains` substring match against the `Display`/`Debug` of the
   returned `CreateIndexBuildError`. Replace or extend
   `create_index_try_build_msgpack.rs` — do not leave two parallel,
   drifting Rust test files covering overlapping ground; decide and justify
   which to keep.
2. **TS (vitest) test** reading the SAME matrix file, driving `createIndex()`
   for each case, asserting the same accept/reject + `reason_contains`
   behaviour. Check `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`
   for the existing per-case hand-written tests this should subsume or
   complement — again, avoid two drifting parallel test suites; decide and
   justify.

Both consumers must load the fixture file from a location BOTH toolchains
can reach without a build-time copy step, if at all possible (e.g. a shared
top-level `docs/dev-artifacts/fixtures/` or similar — check whether either
toolchain already has a convention for cross-language shared test data
before inventing a new location; report what you found either way).

## Scope discipline

- Do NOT introduce `IndexSpec` (the enum), do NOT change `CreateIndexOp`'s
  wire shape, do NOT touch the ~18 flat-literal `CreateIndexOp` construction
  call sites elsewhere in the Rust test suite — those are explicitly #986's
  concern (post-alpha, blocked on this task's matrix existing first as a
  regression safety net).
- The ONE exception: fixing the confirmed `SortedMultiField` TS gap (small,
  additive, proven necessary by the matrix itself) — everything else stays
  additive/consolidating, not a behaviour change.
- If the matrix surfaces ANY other Rust/TS divergence beyond
  `SortedMultiField`, do NOT silently fix it — report it clearly and ask
  before expanding scope, exactly as you're doing here for the one already
  found.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder --full
```

Then, from `crates/shamir-client-ts/`:

```
npm run build && npx vitest run
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the test
run, then commits. Only edit/create files and run read-only / test / gate
commands.

## What to report back

- Where you put the shared fixture file, and why that location.
- The full matrix (or a representative excerpt if very long) — valid +
  invalid cases, one row per `CreateIndexBuildError` variant confirmed.
- Confirmation of the `SortedMultiField` gap (did the matrix independently
  prove it, what did the server-side `admin_table_index.rs` check turn out
  to say), and the fix you applied.
- Whether you found ANY other divergence beyond `SortedMultiField` — list
  them even if you didn't fix them.
- What you did with the two existing overlapping test surfaces
  (`create_index_try_build_msgpack.rs` and `ddl.test.ts`'s hand-written
  per-case tests) — replaced, extended, or kept alongside, and why.
- Exact gate command output (Rust + TS).
