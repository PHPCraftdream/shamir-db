# Brief — P1-4 (#1017): typed public CreateIndex API (not stringly) + strict-by-default

## Context

S.H.A.M.I.R. Database, `crates/shamir-query-builder/src/ddl/create_index.rs`
+ `index_spec.rs`, mirrored in `crates/shamir-client-ts/src/core/builders/ddl.ts`.
Source: review 2026-08-05 §P1-4. Already investigated — read this brief's exact
pointers rather than re-deriving them; a prior session (#908/#915/#970/#998)
already built solid infrastructure this task extends, it does NOT need to be
rebuilt.

## What already exists — reuse it, don't duplicate it

- `crates/shamir-query-builder/src/ddl/create_index.rs`: the public
  `create_index(name, table) -> CreateIndex` entry point + fluent builder,
  still **stringly** for `index_type`/`fts_tokenizer`/`vector_metric`/
  `vector_quantization`/`functional_op` (all `impl Into<String>` /
  `Option<String>`). Has both `build()` (lenient/infallible, always
  succeeds even for nonsense combos — the "raw DTO escape hatch") and
  `try_build()` (validates 12 rules, returns `CreateIndexBuildError`).
- `crates/shamir-query-builder/src/ddl/index_spec.rs`: `pub(crate) enum
  IndexSpec` (Hash/Sorted/Fts/Functional/Vector) — an internal IR that
  already makes illegal combinations **structurally unrepresentable**
  once constructed (`Sorted` has one `field` not `fields`, `Vector::dim`
  is `NonZeroU32`, no variant carries a sibling family's fields, etc.).
  `TryFrom<&CreateIndex> for IndexSpec` in `create_index.rs` is where the
  12 checks live; `IndexSpec::into_op(...)` flattens back to the wire DTO.
  This IR's SHAPE is exactly right — it is just not public, and its
  string-typed leaf fields (`tokenizer: Option<String>`, `metric:
  Option<String>`, `quantization: Option<String>`) are the specific thing
  still not type-checked.
- `crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`:
  the single shared fixture (accept/reject cases + `wire_hex` +
  `reason_contains`) consumed BYTE-IDENTICALLY by both a Rust test
  (`create_index_matrix.rs` → `CreateIndex::try_build`) and a TS test
  (`create_index_matrix.test.ts` → `createIndex()`). Read its `_comment`/
  `_key_order_note`/`_check_order_note` header fields — they document
  real, load-bearing constraints (msgpack key ordering, a known
  cross-language check-order divergence) that any extension must respect.
- TS mirror: `crates/shamir-client-ts/src/core/builders/ddl.ts`'s
  `createIndex(...)` — same stringly shape (`fts_tokenizer?: string`
  etc.), same validation-order caveat noted in the fixture's
  `_check_order_note`.

## The actual gap this task closes

The task's own wording specifies the exact target API shape — implement
it as **additive typed constructor methods on the existing `CreateIndex`
builder** (do not replace `create_index(name, table)` as the single
entry point; that would fragment the API into N free functions each
needing `name`/`table` again):

- `.hash(fields)` — btree/hash index, not unique.
- `.unique(fields)` — btree/hash index, unique. (Note: today `.unique()`
  is a bare flag toggled on the generic builder; the typed form takes
  the field(s) directly, mirroring `.hash(fields)`'s shape.)
- `.sorted(field)` — **exactly one field**, expressed by the method's own
  signature (`Vec<String>` or `impl Into<Vec<String>>` for a single path,
  NOT `Vec<Vec<String>>`) so `SortedMultiField` becomes a compile-time
  impossibility for callers using this method, not just a `try_build()`
  runtime check.
- `.fts(field, tokenizer: Tokenizer)` — single field; `Tokenizer` is a
  **new public enum** (`Whitespace`, `Unicode` — the two values
  `fts_tokenizer` already accepts per the fixture matrix cases
  `"whitespace"`/`"unicode"`). Give it a method/impl that renders to the
  exact wire string (`Tokenizer::Whitespace.as_str() == "whitespace"`)
  so the typed constructor's output is byte-identical to the equivalent
  stringly call — verify this against the existing fixture's `wire_hex`
  values for the `fts`/`fts_with_language` cases.
- `.functional(field, func)` — single field. **No existing `FunctionRef`
  type exists in this codebase** (grep-verified before writing this
  brief) — the task's own wording used it as an illustrative name, not a
  reference to something real. Investigate whether this repo's function/
  validator system (`crates/shamir-engine/src/validator/`,
  `crates/shamir-db/.../execute/admin_function.rs`) has an existing typed
  function-reference/name type worth reusing; if not, `func: impl
  Into<String>` (a plain function name, matching what `functional_op`
  already stores) is the correct, honest typing — do not invent a new
  richer type with no backing semantics.
- `.vector(field, dim: NonZeroU32, metric: Metric, quantization:
  Quantization)` — single field; `Metric` (`L2`, `Cosine`, `Dot` — the
  three values already accepted, see `UnknownVectorMetric`'s check) and
  `Quantization` (`None`, `Sq8` — note `Quantization::None` needs a name
  that doesn't collide with `std::option::Option::None`; call the
  no-quantization variant something unambiguous, e.g. `Quantization::Off`
  or `Quantization::Unquantized`, your call) are **new public enums**.
  `dim` is already `NonZeroU32`-typed at the `IndexSpec` level — the
  typed constructor should take it as `NonZeroU32` directly (not `u32`),
  making `VectorDimRequired` unreachable through this path too.

**Each typed method should produce output byte-identical to the
equivalent already-tested stringly `try_build()` call** — the cleanest
implementation is almost certainly: the typed method renders its enum
arguments to the same strings the stringly path already produces, then
constructs the SAME internal `IndexSpec` variant directly (skipping the
`TryFrom` validation entirely, since the typed method's own signature
already makes the illegal states it would have checked unrepresentable)
and flattens via the existing `IndexSpec::into_op(...)`. This keeps
`IndexSpec`/`into_op` as the single source of truth for wire-shape and
avoids duplicating the flattening logic.

**These typed methods should return something usable directly as a
`BatchOp` (via `.build()`/`IntoBatchOp`, or directly), not require a
separate `try_build()` call** — that IS "строгий build по умолчанию"
(strict-by-default): once a caller has gone through `.hash(...)` /
`.vector(...)` / etc., there is nothing left to validate, so there
should be no fallible step. The EXISTING generic `.index_type(&str)` +
`.build()`/`.try_build()` path remains available unchanged as the raw/
escape-hatch route for callers who genuinely need it (e.g. building from
already-stringly config) — do not remove or deprecate it, this task is
purely additive.

## TS parity — same shape, same fixture

Add equivalent typed factory functions/overloads to
`crates/shamir-client-ts/src/core/builders/ddl.ts`. TS has no enums with
Rust's exhaustiveness guarantees, but **string-literal union types**
achieve the equivalent compile-time safety (e.g. `tokenizer: 'whitespace'
| 'unicode'`, `metric: 'l2' | 'cosine' | 'dot'`, `quantization: 'sq8' |
undefined`) — use that idiom, consistent with how the rest of this SDK
already types constrained string fields (check `crates/shamir-client-ts/
src/core/types/` for the existing convention before inventing a new one).

## Fixture matrix extension

Per the task's explicit instruction, extend
`crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`
with new cases exercising the typed constructors — both valid rows
(assert the SAME `wire_hex` as the equivalent existing stringly case,
proving byte-identical output) and invalid rows where the type system
can't fully prevent a runtime-checkable mistake (there may be few or
none, given the design goal — if every invalid case is now a compile
error, say so explicitly in your final report rather than forcing
artificial "invalid" rows). Read the fixture's own `_consumer_notes` for
how both languages load and interpret it before adding rows — keep the
existing shape/conventions (`name`/`input`/`expect`/`wire_hex`/
`reason_contains`) intact.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, no inline test modules,
  imports at top of file, one-file-one-primary-export (new enums
  `Tokenizer`/`Metric`/`Quantization` may warrant their own small files
  under `ddl/`, or may fit reasonably alongside `create_index.rs` if
  small — your call, but don't cram unrelated types together).
- Wire format (`CreateIndexOp`) is FROZEN — do not touch
  `crates/shamir-query-types/src/admin/*.rs`'s DTO shape. Every typed
  constructor must still ultimately produce that exact same wire type.
- Gate: `cargo fmt -p shamir-query-builder -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-query-builder --full`. If TS tooling is
  reachable from your sandbox, also run its test suite for the touched
  files (check `crates/shamir-client-ts/package.json` scripts); if not
  reachable, say so explicitly rather than silently skipping.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root — a prior task this
session left several `.py`/`.txt`/`.log` files at the repo root that had
to be cleaned up after the fact; don't repeat that.

## Definition of done

- [ ] New public enums (`Tokenizer`, `Metric`, `Quantization` — names/
      exact variants your call within the constraints above) added,
      rendering to the exact wire strings the existing stringly path
      already uses.
- [ ] `.hash(fields)` / `.unique(fields)` / `.sorted(field)` /
      `.fts(field, Tokenizer)` / `.functional(field, impl Into<String>)`
      / `.vector(field, NonZeroU32, Metric, Quantization)` added to (or
      alongside) `CreateIndex`, each producing output byte-identical to
      the equivalent stringly `try_build()` call, each strict-by-default
      (no separate fallible step needed after using them).
- [ ] Existing generic stringly `create_index()` + `.build()`/
      `.try_build()` untouched and still working (escape hatch
      preserved).
- [ ] TS parity: equivalent typed factories in `ddl.ts` using
      string-literal union types.
- [ ] `create_index_matrix.json` extended with typed-constructor cases,
      consumed by both Rust and TS tests, `wire_hex` proven identical to
      the corresponding stringly case where applicable.
- [ ] fmt/clippy/test gates actually run, real pass/fail output reported.
