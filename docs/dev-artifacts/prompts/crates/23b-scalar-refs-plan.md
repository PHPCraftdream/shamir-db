# Plan: scalar read references (PROC-2c stage 3), phases A and B

Companion of `23-scalar-refs.md` (read it first: surface, constraints, soundness gate, report format). This plan
fixes the open points of the brief with evidence from the code and orders the work in two bounded phases, each
ending in a compiling, type-checked, coherent tree. Facts below were verified in the code at the time of writing; if
something no longer matches, trust the code and say so in the report.

## Decisions

1. **A missing or wrongly typed reference never reaches dispatch, provided every referencing operation carries a
   guard.** In `batch_execute.rs:575-606` the engine, for each alias of a stage, calls `build_resolved_refs` and then
   `resolve_skip` (`runner/query_runner/helpers.rs:99-138`) before `execute_single_impl` (`:608`). Marker resolution
   in `param_subst.rs:239-271` only runs inside `execute_single_impl`. A guard's `$query` ref adds a DataFlow edge
   (`planner.rs:204-219`), so the producer runs in an earlier wave and is present in `resolved_refs`. A missing row
   or field resolves to `None` (`resolve.rs:755-779`); `FnCall` stops on an unresolved argument
   (`resolve.rs:394-401`); `ValueCompare` with `None` makes Eq false (`filter_node.rs:336-353`); a null field gives
   `type_of` = "null". If the producer returns zero rows, `[0]` misses and the dependent gets
   `QueryResult::skipped()`; operations depending on it by data flow are skipped too
   (`has_skipped_data_flow_dep`). The guard is mandatory: without it an unresolved insert aborts the whole batch
   (`param_subst:262-268`) and a wrong-typed value is written as is. Batches are non-transactional, so independent
   operations still commit. A producer error aborts the batch as today.
2. **No compiler-side operation-kind map is needed (F8 stays true).** `skipped` is serialized only when true
   (`query_result.rs:150`). The compiler emits `when` only on reference operations and never emits `after`, so any
   `results[*].skipped == true` comes from a guard, directly or by cascade. After `db_execute` the guest walks all
   result entries: `skipped: true` -> kind 2, message `batch reference did not resolve to the expected scalar`;
   missing `results`, an entry that is not a map, or a non-bool `skipped` -> kind 3. The check is emitted only when
   the module contains a reference (a module-wide flag, because plan values can cross functions inside a module).
   Modules without references stay byte-identical.
3. **All six non-null scalar kinds can be guarded** with the `type_of` names of `value_nav/core.rs:79-92`: `string`
   -> "string", `bool` -> "bool", `bytes` -> "bytes", `f64` -> "f64", `i64` -> "int"; `i32` -> "int" plus a gte/lte
   range check; `Big`/`Dec` values are not "int" and are rejected at runtime. Rejected at compile time:
   optional/null/record/list sinks, whole row, `[]`, nested or dotted fields, non-literal index/alias/field,
   negative index, insert/update/delete producers, every sink other than a read-equality value or an insert value.
   Rejected at runtime with kind 2: null, missing row or field, an int read into an f64 sink, an out-of-range i32.

## Surface

`const row: {f: T} = { f: query_ref_field(b0, "src", 0, "f"), ... };` then `batch_read(b, "a", tbl, row)` or
`batch_insert(b, "a", tbl, row)`. `const` is required (a `let` can be reassigned). A record literal passed directly
to the batch call carries no expected field type: rejected with T010.

## Wire form

Marker: `{"$query":"@src","path":"[0].f"}`. Guard atom:
`{"op":"value_compare","left":{"$fn":{"name":"value_nav/type_of","args":[R]}},"cmp":"eq","right":"<kind>"}`;
`i32` adds `{"op":"value_compare","left":R,"cmp":"gte","right":-2147483648}` and the same with `"lte"` and
`2147483647`. `when` is the single atom, or `{"op":"and","filters":[...]}` with atoms in sorted field order. Entry
fields: the existing fields, then `return_result`, then `when`. Native oracle: `qref`,
`value_eq(func("value_nav/type_of",[qref(..)]),"string")`, `value_gte`/`value_lte`, `and`, `batch.when(&h, ..)`.

## Layout hazards (check before adding files)

Folders at 7 entries: `sem`, `sem/check`, `sem/tests`, `api/tests`, `back`, `back/emit`, `back/emit/support`,
`back/lower`. Files at the limit: `back/lower/expressions/expr.rs` (1000 lines), `emit/module.rs` (986),
`emit/abi.rs` (991, do not touch), `support/projection.rs` (961, do not touch). `shamir-db` `tests/functions/gateway/`
is at 7 entries.

## PHASE A: compiler; references stay gated in `compile` at the end of the phase

A0. Pure move: `lower_record` (`expr.rs:186-296`) to a new `back/lower/expressions/record.rs`, declared in
    `lower/mod.rs` (use a normal `mod`, not `#[path]`, if the folder allows it; otherwise create a subfolder). It
    uses `stmt::{opens_blocks, spill, reload}`, already `pub(super)`.
A1. Add `root: ExprId` (the `batch()` call id) to `PlanFact::Batch` (`entry.rs:26`); update all match sites;
    `record_plan_fact` takes `call_id` (passed from `core.rs`).
A2. In `metadata.rs` add `reference_provenance(c, frame, args) -> Result<(ExprId, Vec<(String, PlanOperation)>),
    &'static str>`, reusing the private `plan_argument`/`literal_alias` (a new item widens nothing).
A3. New file `sem/check/builtins/batch/reference.rs`: `RefLocal { markers, consumed }`,
    `QueryRefFact { root, snapshot, alias, index, field, kind }`, `register_reference_sites` (direct fields of a
    `const`-annotated record-literal initializer), `check_query_ref_field` (arity, T007 ceiling, literals,
    provenance: alias appears exactly once in the snapshot and is a Read; expected type -> kind),
    `finish_reference_local`, `check_reference_use` (called from `check_identifier`; allowed only when
    `frame.ref_sink == Some(id)` and not yet consumed), `check_reference_sink` (base fact is Batch with a known alias
    set, same root, snapshot is a prefix; the sink alias is a literal not already in the base).
A4. Hooks: `statement.rs` Let branch and new `FunctionFrame` fields `ref_locals`, `ref_sites`, `ref_sink`;
    `expression.rs` `check_identifier` (about +6 lines); `core.rs`: replace the gate branch; `check_batch_family`
    takes `call_id` and sets `frame.ref_sink` only for argument 3 of `BatchRead`/`BatchInsert`.
A5. `sem/tables.rs`: `ReferenceMarker { alias: "@a", path: "[n].f", kind }` and two `TypedModule` vectors
    (`reference_markers` keyed by call id, `reference_sinks` keyed by sink call id), filled from `entry.rs`.
A6. New `back/batch/reference.rs` with `marker_cell` (a static record with descriptor `["$query","path"]`); in
    `record.rs` a field with a marker uses that static cell instead of `lower_expr`/`to_slot`.
A7. Gate: split `api/compile.rs` into `compile_with(.., references: bool)`; `compile` reports the existing message at
    the first marker: T010 `query_ref_field is gated: scalar reference validation is not implemented`. A hidden
    `testsupport::compile_with_references` (in the existing `wasm.rs`) is used for the byte oracle.

Diagnostics (reuse codes, add none): T004 arity and wrong argument types; T007 scalar or validator entry; T009 bad
path (negative or non-literal index, field not matching `[A-Za-z_][A-Za-z0-9_]{0,63}`) and optional/record/list sink
type; T010 reference outside an allowed site, mutable or unannotated local, no contextual type, unknown alias,
non-read or duplicate producer, self reference, forward reference, another root or a non-prefix plan, unknown plan
fact (if/loop), a reference local used outside the sink or used twice.

Phase A tests: new `sem/check/builtins/batch/tests/{mod.rs,reference_tests.rs}` (uses `sem::tests::helper`):
positive read->read, read->insert, all six kinds, mixed literal and reference fields, two producers; one negative per
diagnostic above with its expected code; a lowering test for the static marker cell; rewrite the current gate tests
(`sem/tests/diagnostic_tests.rs`: `query_ref_field()` now T004; host `procedures/scalar_refs_tests.rs`: valid source
-> gate message via `compile`; `shamir-db gateway/scalar_refs_e2e.rs`: `create_function` still fails on the T010 gate
message); host byte oracle through `compile_with_references` against `Batch::query` / `Batch::insert` with `qref`
(no `when` yet). End of phase: all-targets check and clippy clean, layout clean, report.

## PHASE B: runtime soundness and enabling

B0. Pure move: `ordered_helper_functions` (`module.rs:137-242`) to `emit/helpers.rs`.
B1. `back/batch/reference.rs`: `guard_cell(ctx, &[ReferenceMarker])`, fully static. `template.rs`: the
    `..., "return_result", "when"` descriptors for read-with-where and insert. `lower.rs`: `BatchRead`/`BatchInsert`
    with a `reference_sinks[id]` entry append `when` and call `builder.mark_reference_guard()`.
B2. IR: a flag on `ir/builder.rs`, `IrModule.reference_guards`, `BatchUses.guarded`.
B3. New `support/db/guard.rs` (3 -> 4 entries): `append_guard_keys` for "skipped" and the kind-2 message;
    `emit_response_skip_check_helper(plan, find)` with signature `(i32) -> i32` (decode, walk `results`, apply the
    rules of decision 2). Re-export from `db/mod.rs`. `module.rs`: `skip_check_index`/`key_skipped`/`msg_skipped`;
    `dynmap_find` is also needed when `guarded`; take/push after `response_affected` in the same order; function
    count +1. `db/core.rs` `emit_db_execute_helper`: call the check on the response cell (about +4 lines).
B4. Remove `compile_with`, the testsupport function and the gate. Docs: design-doc intrinsic table and call chapter;
    addendum status (stage 3 implemented, with these runtime semantics); one sentence each in `CLAUDE.md` and
    `AGENTS.md`.

Phase B tests: host tests with the mock gateway (`procedures/scalar_refs_tests.rs` plus a new
`scalar_refs_runtime_tests.rs`): byte oracle including `when` (string, i32 with the 3-atom `and`, multiple fields);
canned `skipped:true` -> kind 2 with the stable message; non-bool `skipped`, non-map entry, missing `results` -> kind
3; a module without references has an unchanged artifact and the same import count. `shamir-db`: turn the e2e file
into a folder `gateway/scalar_refs/{mod,flow,failures,order_acl}.rs` (`gateway/` is at 7 entries): value flows
(read->insert string, read->read i64) and is stored/matched; missing row, missing field, null, int into f64, i32 at
2^31 -> kind 2 with no dependent row, i32 at 2^31-1 succeeds; order: A read, B an insert referencing A, C a read of
the table of B appended after B (waves give A, C then B, so C returns empty while B inserts one row); an independent
insert in a failing batch remains committed; a fresh invocation after a failure succeeds (terminal behaviour only,
F8); an actor without read access on the source -> the existing authorization error, no insert; the Definer case
behaves as before.

## Risks

The guard depends on `value_nav/type_of` being registered in the engine's scalar resolver; if missing, every
reference skips (fail-closed) and the positive e2e test catches it. rmp shortest-int and f64 encodings must match the
guest (the oracle checks this). rust-guard may treat the two pure moves as widened items: list them as moves.
`[n]` with n > 0 depends on engine row order. Every guarded `db_execute` adds one response decode. The keep-order trap
caveat (F8) is unchanged.
