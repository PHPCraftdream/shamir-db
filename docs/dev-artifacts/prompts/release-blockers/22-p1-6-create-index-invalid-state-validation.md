# Brief — P1-6: CREATE INDEX stringly-typed options admit nonsensical states

Task: #970 in the session TaskList. Source:
`docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P1-6.
Depends on #969 (already landed). Read this brief in full; the scope here is
deliberately narrower than the review's headline "introduce a typed
`IndexSpec` enum" framing, for reasons explained below.

## What's already true — verified this session, do not re-derive

`CreateIndexOp` (`crates/shamir-query-types/src/admin/types/index_ops.rs:22-82`)
is one flat struct: `fields: Vec<Vec<String>>`, `unique: bool`, `sorted: bool`,
`index_type: Option<String>`, plus nullable stringly-typed extension fields
(`fts_tokenizer`, `fts_language`, `functional_op`, `functional_args`,
`vector_dim: Option<u32>`, `vector_metric`, `vector_quantization`), `include`,
`if_not_exists`. This IS a wide state space that admits nonsensical
combinations. **However**, this session already found (and you must build on,
not rediscover) that a prior task — F-81/#908, then narrowed by F-87/#915 —
already established the RIGHT pattern for this exact problem: a **three-way
parity checklist**, not a data-model redesign:

1. **Server-side rejection** at DDL-execution time —
   `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs::handle_create_index`
   (~line 338-420). Currently checks exactly 3 things (lines 386-397):
   `sorted && unique` → reject, `!include.is_empty() && !sorted` → reject,
   `sorted && fields.len() != 1` → reject. **Critically**: these 3 checks run
   only in the `index_type` ∈ {`None`, `"btree"`} branch. Line 374
   (`if op.index_type.as_deref().is_some_and(|t| t != "btree")`) dispatches
   straight to `table.create_index_v2(op)` and `return`s — **none of the 3
   checks, nor any equivalent for the non-btree families, ever run** when
   `index_type` is `"vector"`/`"fts"`/`"functional"`. Concretely verified this
   session: `.unique().index_type("vector")` silently creates a **non-unique**
   vector index — `op.unique` is read nowhere on that path. This is a genuine,
   currently-shipping bug, not just an unvalidated construction state.
2. **Client-side pre-validation** — `CreateIndex::try_build()` in
   `crates/shamir-query-builder/src/ddl/create_index.rs:178-191`, error enum
   `CreateIndexBuildError` in `create_index_build_error.rs`. Mirrors exactly
   the SAME 3 server checks, with an explicit, HONEST doc comment (lines
   168-177, "Scope limitation (F-87, #908)") stating it does NOT cover the
   vector/fts/functional families — this brief closes (most of) that
   documented gap, it does not invent a new one.
3. **TS builder** — `createIndex()` in
   `crates/shamir-client-ts/src/core/builders/ddl.ts:169-222`. Currently
   replicates only the FIRST of the 3 server checks (`unique && sorted`,
   lines 190-195) — does not even mirror the other 2 yet.
4. **Cross-language wire fixture** —
   `crates/shamir-query-builder/tests/create_index_try_build_msgpack.rs` is
   the existing test file that pins: (Part A) the 6 canonical valid shapes'
   exact msgpack bytes against `tests/fixtures/create_index_try_build_msgpack.json`,
   (Part B) a builder-output → server-decode round-trip, (Part C) the 3
   existing invalid-combination rejections, one test function per variant,
   each with a comment naming the exact server line it mirrors. **This is
   the template to extend — same file, same pattern**, not a new mechanism.

Additional confirmed-uncaught gaps (verified this session by reading
`table_manager_index_mgmt.rs`'s `create_index_v2` and `kind.rs`):
- `vector_dim: None` while `index_type == "vector"` → silently defaults to
  384 (`table_manager_index_mgmt.rs` ~line 239, `op.vector_dim.unwrap_or(384)`).
- `vector_dim: Some(0)` → passes straight through unvalidated (field is
  `Option<u32>`, not `NonZeroU32`).
- `fields: vec![]` (empty) for ANY index type → no check exists anywhere;
  server does `.first().cloned().unwrap_or_default()` on the derived paths.
- `vector_metric` arbitrary/misspelled string (e.g. `"consine"`, `"L2"`) →
  silently falls into a `_ => VectorMetric::Cosine` catch-all
  (`table_manager_index_mgmt.rs` ~line 240-244) with **no documented
  intentional-fallback rationale** (unlike `vector_quantization`, see below).
- `fts_tokenizer`/`fts_language`/`functional_op`/`functional_args` set while
  `index_type` is NOT `"fts"`/`"functional"` respectively → silently ignored,
  no error.

**Do NOT touch `vector_quantization`'s unrecognized-string handling.**
`VectorQuantization::from_dsl` (`crates/shamir-index/src/kind.rs:150-161`) has
an explicit, deliberate doc comment: "Returns `None` for unrecognised strings
(the caller treats `None` as 'no quantization' — the legacy f32 path)." This
is documented forward-compatible design, not a bug — leave it exactly as is.

## Why the review's full `IndexSpec` enum redesign is OUT OF SCOPE here

The review proposes a first-class Rust enum
(`Hash{..}`/`Sorted{..}`/`Fts{..}`/`Functional{..}`/`Vector{dim: NonZeroU32,..}`)
replacing `CreateIndexOp`'s flat shape, plus a shared Rust/TS declarative
fixture matrix generating both builders. **Do not implement this.** Verified
this session: `CreateIndexOp` has 85 references across 27 files (1 struct
definition, 1 Rust builder, 2 production consumers, ~18 test files
constructing it as flat literals, plus the TS DTO type in a different repo
tree). A full redesign touches wire compatibility, both language builders,
and every one of those 18 test call sites — large, cross-cutting, and the
existing F-81/F-87 parity-checklist pattern already closes the actual bug
risk (silent misconfiguration, not "the type system doesn't statically
prevent it") via pure additive validation. Match the review's own established
minimum-viable-fix precedent (already used for #966/#967/#968/#969 this
session) rather than the large redesign.

## Required work — extend the existing 3-point parity checklist

Add the following NEW checks, in ALL THREE places (server / try_build / TS),
keeping each check's WORDING consistent across all three so a caller sees the
same explanation everywhere (follow the existing pattern: `try_build()`'s doc
comment and `CreateIndexBuildError`'s `Display` impl both explicitly
cross-reference the server message they mirror — do the same for every new
variant):

1. **`fields.is_empty()`** → reject, regardless of `index_type`. Message:
   "CREATE INDEX requires at least one field".
2. **`unique: true` with `index_type` ∈ {`"vector"`,`"fts"`,`"functional"`}**
   → reject. Message: "`unique` is not supported for '{index_type}' indexes;
   only btree/hash indexes can be unique".
3. **`sorted: true` with `index_type` ∈ {`"vector"`,`"fts"`,`"functional"`}**
   → reject. Message: "`sorted` is not supported for '{index_type}' indexes".
   (`include` is already transitively caught by the existing
   `!include.is_empty() && !sorted` check once `sorted` itself is rejected
   for these types — no separate include check needed here.)
4. **`index_type == Some("vector")` && (`vector_dim.is_none()` ||
   `vector_dim == Some(0)`)** → reject. Message: "vector index requires
   `vector_dim` > 0".
5. **`index_type == Some("vector")` && `vector_metric` is `Some(s)` where
   `s` is not one of `"l2"`, `"dot"`, `"cosine"` (case-sensitive, matching
   the existing match arms exactly)** → reject. Message: "unknown
   vector_metric '{s}'; expected 'l2', 'dot', or 'cosine'".
6. **`index_type != Some("vector")` && (`vector_dim.is_some()` ||
   `vector_metric.is_some()` || `vector_quantization.is_some()`)** → reject.
   Message: "vector_dim/vector_metric/vector_quantization are only valid for
   'vector' indexes".
7. **`index_type != Some("fts")` && (`fts_tokenizer.is_some()` ||
   `fts_language.is_some()`)** → reject. Message: "fts_tokenizer/fts_language
   are only valid for 'fts' indexes".
8. **`index_type != Some("functional")` && (`functional_op.is_some()` ||
   `functional_args.is_some()`)** → reject. Message: "functional_op/
   functional_args are only valid for 'functional' indexes".

### 1. Server (`admin_table_index.rs::handle_create_index`)

Add the new checks BEFORE line 374's `index_type != "btree"` dispatch (so
they run for EVERY index_type, not just btree) — this is the actual bug fix
(closing the "unique silently ignored for non-btree types" hole). Move the
existing 3 btree-only checks (386-397) so they still only apply when
`index_type` is `None`/`"btree"` (checks 2/3/6/7/8 above already scope
themselves by `index_type`, so ordering just needs checks 1 and the new
type-scoped ones to run before the line-374 early return).

### 2. `CreateIndex::try_build()` + `CreateIndexBuildError`

Add one new enum variant per check (8 new variants — reuse/don't duplicate
if a check is identical wording to an existing variant), each with a
`Display` impl following the existing style (state the rule, name the
server file that enforces it). Update the "Scope limitation (F-87, #908)"
doc comment on `try_build()` to reflect what is now covered vs. what
genuinely remains server-only (the doc already names: one-vector-index-per-
table constraint, functional-op trustedness, FTS tokenizer DSL
well-formedness — these three stay server-only, this brief does not touch
them).

### 3. TS builder (`createIndex()` in `ddl.ts`)

Add the same 8 checks (`throw new Error(...)`), matching wording style to
the existing `unique && sorted` check already there (lines 190-195).

### 4. Tests

- Extend `crates/shamir-query-builder/tests/create_index_try_build_msgpack.rs`
  Part C with one new `#[test]` per new `CreateIndexBuildError` variant
  (positive: rejected with the exact variant; and where meaningful, a
  companion "valid, NOT rejected" case near the boundary — e.g.
  `vector_dim(1)` accepted, `vector_dim(0)` rejected).
- Add TS unit tests (find/extend the existing `ddl.ts` test file under
  `crates/shamir-client-ts/src/__tests__/` — check what already covers
  `createIndex`'s existing `unique && sorted` throw and follow the same
  file/pattern) — one test per new check.
- Add or extend a server-side e2e/integration test proving AT LEAST the
  `unique` + `index_type("vector")` case is now rejected end-to-end (not
  just at the client `try_build()` layer) — this is the one that was a
  silently-accepted live bug; a live-server assertion is the strongest
  proof it's actually fixed, not just documented as fixed.

## New follow-up task to file (do NOT implement — just create it)

Before starting the code work, use TaskCreate to file a new pending task:
"Introduce typed `IndexSpec` Rust enum + wire-compatible DTO + shared Rust/TS
fixture-matrix codegen for CREATE INDEX (post-alpha, API-freeze follow-up)"
— body: reference this task (#970) and the review's §P1-6 full proposal,
note that the validation-only fix landed here closes the concrete bug risk
but the review's broader "make invalid states unrepresentable at the type
level" goal remains open for a deliberate post-alpha design pass. No
`blockedBy`/`blocks` needed — standalone, same pattern as #984/#985.

## Gate (MANDATORY)

```
cargo fmt -p shamir-query-builder -p shamir-query-types -p shamir-db -p shamir-engine -- --check
cargo clippy -p shamir-query-builder -p shamir-query-types -p shamir-db -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder -p shamir-query-types -p shamir-db -p shamir-engine
```
Plus, since `ddl.ts` is touched:
```
npx tsc --noEmit   # run inside crates/shamir-client-ts
```
Plus the TS test runner for whatever test file you extend (check
`crates/shamir-client-ts/package.json` for the exact vitest invocation used
elsewhere in this session's prior briefs).

## Scope discipline

- Do NOT implement the `IndexSpec` enum / wire DTO redesign — file it as a
  new task instead (see above).
- Do NOT touch `vector_quantization`'s unrecognized-string handling — it is
  deliberate, documented, forward-compatible behavior, not a gap.
- Do NOT touch the 3 EXISTING checks' wording/behavior — only ADD the 8 new
  ones alongside them.
- Do NOT touch the genuinely server-only validations the existing doc
  comment already carves out (one-vector-per-table, functional-op
  trustedness, FTS tokenizer DSL well-formedness) — those stay exactly as
  they are.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

List every new check added, in which of the 3 places (server / try_build /
TS), with the exact error message used in each. Confirm the new
`unique`+non-btree-`index_type` server-side test actually fails BEFORE your
fix and passes AFTER (i.e. show you reproduced the live bug, not just added
an assertion that happened to already pass). Confirm the new follow-up task
was filed via TaskCreate (give its task id). Give exact gate command output
for every crate in the gate list.
