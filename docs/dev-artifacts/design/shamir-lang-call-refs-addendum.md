# PROC-2c addendum: checked stored calls, batch references, update and delete

**Status:** Stage 1 (count-only `batch_update`, `batch_delete` and `response_affected`, section 4 and the stage 1 row of section 6) and stage 2 (checked stored-procedure call and safeguards, section 2) are implemented in source; tests are pending CI. Stage 3 Phase A (A1–A7) semantic checking and marker lowering are implemented in source for the narrow `const` annotated-record/direct-reference subset below. Marker lowering is available only through hidden `testsupport::compile_with_references`; production `compile` still emits the existing `T010` gate after valid semantics, so no production reference is accepted. Phase B runtime guards and post-`db_execute` skipped-operation checks are not implemented. Pure move A0 (`lower_record`) is complete; B0 (`ordered_helper_functions`) is not done and is out of scope. This addendum supplements §§3–8 of `shamir-lang-design.md`; it does not change that document. §10 distinguishes observed source behavior from proposals. It replaces the earlier proposals and questions in this file with the decisions below. Procedure entries may declare bare `-> null`, which the compiler emits as `procedure(...)->null`; this intentionally widens only the procedure-entry result boundary, and scalar and validator rules are unchanged.

## 1. Scope and decisions

This addendum defines three work areas: checked calls between stored procedures, Phase A semantic checking and testsupport marker lowering for a gated scalar read-reference subset in typed batches, and typed batch update/delete. It adds no arbitrary raw DTO construction or general query language.

Delivery is staged: stage 1 adds update/delete and affected-count projection; stage 2 adds checked stored calls and their security/runtime safeguards as one unit; stage 3 Phase A implements the narrow static semantic checks and testsupport marker lowering while production `compile` retains the T010 gate. Phase B runtime guards and skipped-operation checks are not implemented. Stage 1 and 2 expose no references in production; stage 3 currently accepts none in production.

Accepted decisions:

1. At the compile-result boundary, carry the compiler-emitted signature descriptor metadata to artifact creation and persist it atomically with the Wasm artifact; the existing `CompiledModule` shape must be extended or accompanied by compiler-owned metadata for this purpose. The callable catalog consists of these emitted signatures. Legacy artifacts have no such catalog entry, are not callable through `call_checked`, and receive no migration path.
2. The host compares deterministic compiler-owned structural signature descriptor bytes directly. There is no contract version, hash, or SHA policy. These bytes are call metadata, not a result-contract DTO or a general reference-descriptor grammar.
3. A checked call captures the artifact, signature and security/resource metadata together once, then validates, authorizes and invokes that captured artifact. No second name lookup is permitted. Missing catalog metadata fails closed.
4. Recursion is forbidden on every stored-call route, including legacy guest `call` and gateway-originated batch `Call`. A shared immutable active-call identity path detects repeats and allows at most 32 entries including the root. Identities remain stable across rename/replacement. Check before entering; unwind with a scope/cancel-safe guard. This path is not a child-list global visited set.
5. The current effective actor of the parent hop authorizes target Execute and is the target Invoker actor. There is no retained root `A0` identity. Preserve existing Invoker setuid behavior: when enabled it resolves the Invoker actor to the target owner. A new fail-closed rule rejects entry to an Invoker target owned by someone other than the parent Definer owner when the parent frame is Definer. Definer executes as its own owner.
6. Version 1 adds only `batch_update`, `batch_delete`, and `response_affected`, with the exact narrow rules in §4. Stage 3 Phase A static checking and testsupport lowering are implemented, but stage 3 remains gated in production and accepts no references through production `compile`; no transactional reference mode or new host import/engine DTO is specified.

## 2. Checked stored calls

### 2.1 Surface, signature and binding

`call` is a procedure-only intrinsic. Its target is a compile-time string literal naming a stored `shamir-lang` procedure; its argument record names exactly the target's declared parameters. Positional arguments are unsupported. The existing compile result contains only Wasm bytes and a report (`crates/wasm/shamir-lang/src/api/compile.rs:16-21`); stage 2 adds compiler-owned signature metadata at that boundary and persists it atomically with artifact creation. The compiler checks calls against its catalog of these emitted signatures; there is no external catalog publisher or legacy-signature inference.

Procedure entries may declare bare `-> null`, which the compiler emits as `procedure(...)->null`; this intentionally widens only the procedure-entry result boundary, and scalar and validator rules are unchanged.

Example source:

```text
@entry procedure place_order(user_id: i64, sku: string) -> i64 {
  let order: { id: i64 } = call("create_order", {
    user_id: user_id,
    sku: sku
  });
  return order.id;
}
```

The expected result type is the exact declared target result, including nullability. Argument names, count, order-independent name matching, and types must match exactly; no implicit conversions are introduced. Parameters are encoded as a map with exactly the declared parameter names. Empty parameter records encode as an empty MessagePack map, not nil. Returned bytes are decoded using the shared guest codec and projected to the declared result; malformed bytes or incompatible result shape are codec/projection kind 3. A missing/zero host result is a host-contract failure, not a nullable result.

Proposed checked ABI:

```text
(shamir_host, call_checked):
  (i32 name_ptr, i32 name_len,
   i32 signature_ptr, i32 signature_len,
   i32 args_ptr, i32 args_len) -> i64 packed_result
```

The signature descriptor uses this canonical UTF-8 ASCII grammar: `procedure(p1:T1,p2:T2)->R`, with no whitespace. Parameters retain declaration order; argument-map key order is irrelevant, but keys/count must exactly match declared parameters. Record types use `{field:T,...}` with unique field names sorted by UTF-8 bytes; list types use `[T]`; optional types append `?`; scalar tokens are exactly `i32`, `i64`, `f64`, `bool`, `string`, `bytes`, and `null`. Opaque types are not callable signature types. The entry/function name is excluded, so rename does not change signature bytes. This encoding is unambiguous for the main design's type grammar and bounded by its existing source/static limits. It is call metadata, not a result-contract DTO or public general descriptor grammar. Host bytes are compared directly to the signature stored with the artifact; there are no versions, hashes or SHA policy.

The checked resolver captures, compares descriptor bytes, authorizes and invokes the captured artifact. The argument map must contain exactly the declared parameter names; nullable parameters are required to be present and may carry nil. The actual encoded null result is MessagePack nil in a nonzero packed result; a missing/zero host result remains a host-contract failure, not a nullable result. Legacy targets without trusted compiler-emitted catalog metadata cannot be invoked through `call_checked`; an importer cannot forge that missing metadata.

The compiler's emitted ceiling is six at stage 2 if all imports are reachable; the main design's ceiling is five (docs/dev-artifacts/design/shamir-lang-design.md §7; crates/wasm/shamir-wasm-admission/src/types/limits.rs:65). Neither is an observed compiler constant; admission's observed cap is 64. The new import is currently absent from both the admission allowlist (`crates/wasm/shamir-wasm-admission/src/sanitizer.rs:87-99`) and host linker (`crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs:232-250`); stage 2 must register it in both together (F1).

### 2.2 Capability, effects and local cycles

`call` has procedure/database-call effect. Only procedure entries may use it, and only to another stored procedure. Scalar and validator entries reject the effect. Helpers inherit effects transitively; reject an entry whose reachable helper graph exceeds its capability ceiling. Statically visible helper cycles remain compile errors. Cross-stored cycles are rejected at runtime by the shared active-call path, including paths crossing legacy and gateway routes.

Use existing compiler diagnostics where suitable: `T004` for target/signature mismatch, `T007` for capability, `T008` for known local cycle/depth and `T009` for unsupported boundaries. Runtime checked-call plan/authorization failures map to proposed kind 2; do not claim that current gateway error wrapping already gives this mapping.

### 2.3 Identity and authorization

Each hop uses the current parent effective actor for target Execute and as target Invoker actor (F3). The target independently derives its effective actor from its own security metadata:

| Target security | Effective actor |
|---|---|
| Definer | target owner |
| Invoker, setuid disabled | current parent effective actor |
| Invoker, legacy setuid enabled | target owner, preserving current behavior |

For a Definer parent, retain its owner in frame metadata; a target whose effective actor is escalated to its owner by the legacy setuid bit sets the same frame owner, and the owner is carried unchanged through Invoker hops below it. Before entering an Invoker target, reject if that target's owner differs from the carried owner. This is a new fail-closed v1 guard; “someone else” means specifically a different owner than that parent Definer owner. Execute authorization still uses the parent effective actor. Rebuild the target's gateway, default repository, secret/network grants and resource context from the captured binding and target effective actor; do not inherit the caller's grants wholesale.

The checked resolver must close that gap with one coherent capture. The existing legacy nested route clones the parent actor/context and invokes without the per-target Execute authorization described here; the proposal hardens it. Missing target, signature, security metadata, owner, or required capability fails closed.

Compile-time catalog visibility uses the principal performing create, create-with-replace, or check, based on that principal's Execute right; it does not use a later alterer's identity, because `alter_function_optimize_as` rebuilds stored bytes and does not take a callable-catalog snapshot. If Execute is later revoked, the existing runtime Execute check refuses already-stored callers.

### 2.4 Recursion, resources, cancellation and gateway calls

The active-call identity path is immutable per invocation lineage, shared by every stored-call mechanism, and capped at 32 entries **including the root**. Before entry, reject if the target identity is already in the path or adding it would exceed 32 entries. Use an identity stable across rename and artifact replacement, not a mutable function name, so neither can evade repeat detection. Pop by scoped guard on return, error, cancellation or task teardown. Keep the existing lower depth policy and pool/resource limits; the 32-entry cap does not raise any lower limit. Any gateway-originated nested call enabled after safe context plumbing must inherit depth, shared fuel pool, active-call identity path, current actor and root deadline; until then it is refused before dispatch (F1).

The legacy guest `call` path must be hardened to use the common target resolution, Execute authorization, target security/grant derivation, active-call path, and captured-artifact invocation. Legacy artifacts imported through that path may lack language signatures, so the legacy route performs no signature comparison; this is not an authorization or recursion bypass. `call_checked` itself remains unavailable for legacy **targets** because they have no trusted signature metadata.

The common context gap is observable in the gateway execute → function invoker → management-created `FnCtx` chain: the current builder initializes depth zero and no pool (F1). Thus the guest `call` depth check does not bound a mixed chain through gateway batch Call. Before enabling gateway Call, common context plumbing must carry the identity path and effective actor. The standard nested guest-call path shares fuel leases across nested Wasm work, suspending/reacquiring the parent lease (F1); any future gateway call must use this same nested accounting rather than a fresh independent pool. Until then, refuse gateway-originated Call before dispatch, including nested batch trees. Preserve and regression-test the current transaction guard: transactional Call is rejected before invocation; do not characterize it as a current deadlock.

Existing Wasm execution has a depth-0 wall timeout, per-Store epoch deadlines and AccountedStore fuel settlement on future cancellation (`crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs:708-721,780-793,850-892`). The gateway context reset is not evidence of a single inherited deadline over mixed routes; stage 2 must preserve root deadline/context without spawning detached work. Prompt cancellation timing is not runtime-verified.

Future nested calls must preserve existing pool/fuel limits and shared lease behavior; do not claim fuel meters native DB/planner CPU. Cancellation-safe identity unwind and no detached child are stage-2 acceptance criteria, not verified behavior. Already committed gateway work is not rolled back.

## 3. Gated scalar read-reference proposal

Production `compile` does not accept scalar-reference source: after successful semantic checking it rejects any reference marker with `T010`, `query_ref_field is gated: scalar reference validation is not implemented`. Phase A semantic checking and marker lowering are implemented in source for the narrow subset below; hidden `testsupport::compile_with_references` enables that lowering for testsupport byte oracles only. This is not a production reference path. Phase B reference-specific runtime guards and post-`db_execute` guest skipped-operation checks remain unimplemented.

Semantic support is limited to direct `query_ref_field` fields in a `const` local initialized with an explicitly annotated record literal, consumed once as the whole record value for an eligible `batch_read` equality filter or `batch_insert`. Each reference uses literal alias/index/field arguments for a `[n].field` path, resolves to an eligible read operation in the same known plan root/history, and has a statically known non-null scalar sink (`i32`, `i64`, `f64`, `bool`, `string`, or `bytes`). Other paths, sources, sinks, or uses are rejected during semantic checking with the existing diagnostics: `T004` for arity/argument types, `T007` for entry capability, `T009` for invalid path/non-scalar sink, and `T010` for unsupported context/provenance/use. Semantic checking records private compiler metadata for these reference sites and sinks; this does not add a public `QueryRef` type or API. Production `compile` remains gated and no production reference is accepted.

The former proposal's additional exclusions remain outside this surface: whole rows, `[].field`, list/column references, nested paths, insert-result producers, update/delete references, other contexts, and `batch_after` are unavailable.

Phase A source shape (testsupport lowering only; production compile returns T010):

```text
const b0 = batch_read(batch(), "users", "users", { id: 7 });
const row: { name: string } = { name: query_ref_field(b0, "users", 0, "name") };
const b1 = batch_insert(b0, "audit", "audit", row);
const response = db_execute(request(b1));
```

The Phase A checker records a marker descriptor for each statically validated reference; testsupport lowering emits the marker record `{"$query":"@alias","path":"[n].field"}`. That is a compiler-produced test artifact, not production compiler output. Phase B's runtime `when` type/presence guard and guest check for skipped reference operations after `db_execute` remain incomplete; the native runtime mechanisms below do not establish end-to-end soundness.

Native builder illustration of a possible Phase B guard (not a Shamir compiler acceptance or behavioral oracle):

```rust
let mut batch = Batch::new();
batch.query("users", Query::from("users").where_(eq("id", 7)));
let audit = batch.insert(
    "audit",
    insert("audit").row(doc().set("name", qref("users", "[0].name"))?),
);
batch.when(
    &audit,
    filter::value_eq(
        val::func("value_nav/type_of", [val::qref("users", "[0].name")]),
        "string",
    ),
);
let bytes = batch.to_msgpack()?;
```

This native-builder illustration shows a possible Phase B guard, not a Shamir compiler acceptance or behavioral oracle. `keep_order` preserves request serialization, not global execution order. The compiler's current keep-order flag is set around request encoding and reset on normal return; an encoder trap bypasses that reset. This caveat does not imply an implemented Shamir reference guard or runtime path.

Existing planner execution uses dependency waves, with insertion order only as the tie-break within a wave (F6). This is an engine behavior observation, not a claim that reference-bearing Shamir operations currently produce or follow planner dependencies.


## 4. Version-1 update/delete intrinsics

### 4.1 Exact surface and restrictions

Add only these signatures:

```text
batch_update(Batch, string alias, string table, Predicate filter, WireRecord set) -> Batch
batch_delete(Batch, string alias, string table, Predicate filter) -> Batch
response_affected(Response, string alias) -> i64?
```

`Predicate` is a nonempty record of non-null scalar literals or parameters, lowered as an exact conjunction of field equality comparisons. No bare scalar predicate, whole null, empty record, nullable/null field, reference, or arbitrary expression is accepted. A nonempty predicate may nevertheless match every row; do not claim the compiler proves it selective. `WireRecord` is compiler notation for a nonempty typed wire-safe record; `set` contains no references. Aliases and table names are compile-time strings. No repository qualifier is exposed: operations use `TableRef::new(table)`, whose repository is `main`.

No returning mode or projection is exposed. There is no direct write import, compare-and-swap/expected-version behavior, or update/delete response-row helper in v1. Do not add a host import, engine DTO, or host execution mode for this stage. The main compiler design's emitted-module import ceiling is five; this stage does not increase it. Stage 2's compiler design ceiling is six if all imports are reachable, as specified in §2.1.

Source example:

```text
let b0 = batch_update(batch(), "u", "users", { id: user_id }, { name: "Ada" });
let b1 = batch_delete(b0, "d", "sessions", { owner: user_id });
let response = db_execute(request(b1));
let changed: i64? = response_affected(response, "u");
```

The native `Update::set` builder returns the update builder; only the `doc().set(...)` operation is fallible here:

```rust
let mut batch = Batch::new();
batch.try_update(
    "u",
    update("users").where_(eq("id", user_id)).set(doc().set("name", "Ada")?),
)?;
batch.try_delete("d", delete("sessions").where_(eq("owner", user_id)))?;
let bytes = batch.to_msgpack()?;
```

### 4.2 Wire lowering, result and authorization

Lower the predicate to native `Filter` equality nodes and the set to a typed record; do not serialize a language-built raw filter map. Update maps to `UpdateOp` with `where_clause: Some(filter)`, nonempty `set`, no return `select`, and no expected version. Delete maps to `DeleteOp` with the predicate and no return `select`. The DTO's flattened operation fields and `return_result` remain as currently defined. Keep alias insertion order and existing named MessagePack serialization.

`response_affected` reads `results[alias].stats.records_scanned`, whose engine accounting copies `WriteResult.affected` as `u64` (F2). For update this is changed rows; for delete it is removed rows. Missing alias is null only if the valid root `results` map exists. Missing root, missing stats/count, wrong type, negative value, or value above `i64::MAX` is kind 3. This projection reports counts only; it does not assert return rows.

Required access is Read for read, Create for insert, Write for update and Delete for delete (F2). The gateway performs authorization using the effective actor. Do not duplicate or replace engine checks. A Definer procedure's batch is authorized as its effective owner; a called procedure gets its own target-derived effective actor (F2,F3).

Write/Delete authorization does not itself require Read for returned rows (F2). Update selects records for return modes, while native required-access classification is Write, not Read (F2). This is a design risk, not an observed exploit; it motivates excluding returning rows from this first surface.

### 4.3 Static and runtime validation

Static compiler checks: literal alias/table, unique alias in the compiler's current plan metadata, supported operation shape, nonempty predicate and set, exact scalar non-null predicate values, and typed wire-safe set fields. Existing static diagnostic codes are used where appropriate: `T001`/`T004` for invalid shapes and `T010` when compiler metadata cannot prove a required type/operation fact. T-codes describe compile-time diagnostics only.

Runtime checks remain authoritative: DTO decoding, alias/count limits, dependency and execution limits, gateway authorization, table existence, engine filters and validators, foreign-key/write constraints, and response decoding. Plan/DTO failures use proposed kind 2; malformed response/projection uses kind 3. These are proposed mappings, not claims about today's host error wrapping. The compiler must carry operation-kind metadata from Batch through Request to Response for compile-time response checking; current helpers do not track it (F8). An absent alias is still valid nullable projection only under a valid response root.

## 5. Bounds and failure boundaries

No extra cross-call work or result-byte budget is designed. Preserve the existing per-call pool/fuel policy, per-request `BatchLimits`, admission limits and DB buffer checks. Current design/runtime reference points include a 1 MiB compiler DB argument buffer, an 8 MiB emitted output capacity, at most 50 batch operations, 64-byte aliases, and a default dependency-depth limit of 10 (F6). These limits are distinct; an 8 MiB guest output capacity is not a claim that every host response is bounded by that value before allocation.

The 32-entry stored-call identity path includes the root and applies across legacy, checked and gateway call routes. Existing lower depth/pool limits continue to apply. Request planner/authorization checks occur before dispatch. Nothing in this design says Wasm fuel meters native query planning or row CPU. Nontransactional writes may already be committed when a later projection fails; no rollback or retry is promised.

## 6. Implementation plan

Stages are indivisible at their stated safety boundaries. Tests below are acceptance criteria for implementation, not run as part of this document rewrite.

| Stage | Scope / likely owners | Acceptance focus | Main risk |
|---|---|---|---|
| 1 — update/delete | `crates/wasm/shamir-lang/src/sem/check/builtins.rs`; `crates/wasm/shamir-lang/src/back/batch/{lower,template}.rs`; `crates/wasm/shamir-lang/src/back/emit/support/db.rs`; native `try_update`/`try_delete` + `to_msgpack` byte oracle; no engine/DTO/ABI change; import ceiling stays five | exact builder byte oracle; reject unsupported predicates/empty sets; pin `stats.records_scanned == WriteResult.affected`; changed-only update and removed delete counts; no-return empty records; malformed projection and ACL cases | DTO/accounting mismatch |
| 2 — checked call and common admission | compiler checking/emission; admission sanitizer; host linker/bindings; function artifact create/replace and signature persistence; management authorization/binding; runtime context; legacy call and batch-call route | emitted signature atomically follows artifact; missing record fails closed; exact ABI/descriptor; captured binding is the invoked artifact; all target security cases; repeat and 32-entry rejection across every route; stable identity on rename/replacement; cancel-safe unwind; pool/depth inheritance; transaction guard remains; gateway-originated Call refused until context plumbing is safe | authorization confused-deputy, reentrant alias/replacement, cancellation/context split |
| 3 — scalar read refs (gated in production) | Phase A A1–A7 semantic checking and marker lowering are implemented in source; `const` annotated-record/direct-reference read-equality or insert subset only; hidden testsupport lowering, production `compile` retains `T010` | Phase A semantic and marker-lowering cases are present in source; no production reference acceptance. Phase B reference-specific `when` guards and post-`db_execute` guest skipped-operation checks remain unimplemented | Runtime soundness and production enablement remain gated |

Stage 2 must ship the checked path, catalog/artifact persistence, linker/admission registration, actor/security checks, shared active identity, legacy hardening, gateway context policy and transaction guard together. Do not enable an unchecked intermediate path. Stage 3 production acceptance remains gated: no reference is accepted by production `compile` until Phase B guards and post-execution checks are implemented and end-to-end behavior is established.

## 7. Acceptance matrix

- **Stage 1 compiler:** positive update/delete with non-null scalar parameter equality and typed nonempty set; reject empty/bare/null/nullable/ref/arbitrary-expression predicates, empty sets, unsupported return/projection forms and nonliteral table/alias. Compare native builder output with the emitted DTO bytes.
- Stage 1 acceptance: exact native builder byte oracle and predicate rejection; pin `stats.records_scanned == WriteResult.affected`, changed-only update count and removed delete count; assert no-return operations expose no records. Also cover malformed/missing projections and gateway Write/Delete denial with Definer actor.
- **Stage 2 compiler/admission:** exact named signature and result nullability; non-procedure target, unknown catalog, mismatched signature, denied effect and unsupported arguments fail closed; emitted import, sanitizer and linker agree. Legacy targets cannot be invoked through `call_checked`; untrusted callers cannot supply or forge trusted catalog metadata.
- **Stage 2 identity/path:** actor chain for Invoker, Definer, Definer-to-Invoker owner match/mismatch and legacy setuid; Execute denial; absent metadata/owner; per-target grants; same captured artifact invoked; self/repeated identity and 32-entry boundary across checked, legacy and gateway paths; identity survives rename/replacement; path restores on success, error and cancellation. Confirm transactional batch `Call` is rejected before dispatch and nontransactional gateway `Call` is refused until context is carried safely.
- **Stage 3 Phase A:** semantic tests cover the narrow supported reference forms and static rejection cases; hidden testsupport byte-oracle lowering is available. Production compile must retain the stable T010 gate. Phase B is not implemented: reference-specific guards and post-`db_execute` skipped-operation checks; no production runtime/value-flow, planner-wave, or ACL acceptance is claimed. These are source/documentation facts, not tests run for this update.

## 8. Non-goals

- Dynamic/function-value calls, arbitrary name-based registry access, scalar/validator targets, cross-module linking, recursion, legacy migration or bypassing Execute/security checks.
- References in stage 1 or 2; whole-row/list/column references; references produced by inserts or writes; update/delete references; general result typing or a new reference descriptor grammar.
- Direct update/delete imports, row-returning update/delete, projection, CAS/expected-version, upsert, repository-qualified operations, DDL, raw DTO maps/bytes, custom filter ASTs, dynamic values or general transactions.
- Global source-order execution, transactional reference batches, rollback after committed effects, automatic retry, or claims that native CPU is Wasm-fuel-metered.
- Contract-version/hash/SHA policy and general result-shape descriptor DTOs are excluded. No new transactional reference mode, cumulative cross-call work/byte budgets, or duplicate-alias host scan is proposed.

## 9. Decisions

1. **Callable population:** only compiler-emitted `shamir-lang` procedure signatures stored atomically with their artifacts are callable through `call_checked`; legacy artifacts are not migrated or callable through it.
2. **Signature verification:** compare deterministic canonical structural signature bytes directly; checked binding captures artifact and metadata once and invokes that artifact; missing catalog data fails closed.
3. **Call identity/security:** every stored-call route shares a stable active identity path, repeat rejection and a 32-entry including-root limit; current parent effective actor authorizes Execute and supplies Invoker identity; retain legacy Invoker setuid semantics and add the Definer-owner mismatch guard.
4. **Legacy and gateway routes:** harden legacy guest calls using the common resolver without requiring legacy signature comparison. Gateway batch `Call` shares invocation context; refuse it before dispatch until safe plumbing exists. Preserve transaction rejection.
5. **Immediate batch surface:** stage 1 adds only `batch_update`, `batch_delete`, and `response_affected`; use nonempty non-null scalar equality records, nonempty typed sets, main repository, and no returning/projection/CAS.
6. **Reference/compiler surface:** stage 1/2 have no production references. Stage 3 Phase A semantics and testsupport marker lowering are implemented for the narrow `const` annotated-record/direct-reference read-equality or insert subset; production `compile` still gates with T010 after successful semantics. Phase B's reference-specific guard and post-`db_execute` guest skipped-operation checks are not implemented, so no production reference/runtime acceptance is claimed. No `batch_after`, public `QueryRef` type, engine/DTO/ABI/import change, transactional reference mode, version change, or update/delete references is accepted. The language emits only `call_checked`, never gateway batch `Call`; gateway-originated Call stays refused until common invocation-context plumbing is safe.

## 10. Verified against the code

Evidence IDs map source observations to paths and line ranges. They describe inspected source, not runtime-test results.

**Historical note:** The source observations below record the state before stage 2 was implemented; statements about missing checked-call registration, catalog/signature persistence, common resolution, and hardening describe that pre-stage-2 state. Stage-2 tests are pending CI.

- **F1 — existing nested calls, linker, gateways, contexts and transaction guard.** `crates/wasm/shamir-wasm-host/src/wasm/bindings/call.rs:83-96,107-119,139,156-179,186-209` increments depth and clones actor/grants/pool into the child, including nested fuel lease suspension/reacquisition. `crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs:232-250` registers legacy `call` and DB imports only; there is no `call_checked` registration. `crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:262-276` decodes then executes as the facade actor. `crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs:45-48` invokes with the actor clone. `crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/management.rs:761-781` authorizes, resolves function metadata, builds `FnCtx`, then invokes registry; this is not one atomic binding capture. `crates/wasm/shamir-wasm-host/src/runtime/context.rs:555-574,599-613` has no A0 field and initializes depth zero/pool none in the observed builder. `crates/engine/shamir-engine/src/query/batch/runner/query_runner/runner.rs:143` rejects transactional batch Call before invocation; this guard is retained as a regression criterion. No transactional deadlock is observed.
- **F2 — batch actions, update count and stats.** `crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:530-536` maps Read/Insert/Update/Delete to Read/Create/Write/Delete. `crates/engine/shamir-engine/src/table/write/update/execute.rs:228-233,385-394` selects rows and increments affected only for changed rows. `crates/engine/shamir-engine/src/query/batch/control/receipts/accounting.rs:37-45` copies `WriteResult.affected` into both `stats.records_scanned` and `stats.records_returned`; returned record lists are separate. This pins the proposed `response_affected` source.
- **F3 — authorization and effective actor.** `crates/engine/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:209-274,357-380` checks Execute as the passed actor, resolves Definer to owner, Invoker to caller unless legacy setuid selects owner, and returns caller when no record is supplied. Top-level management's separate authorization/metadata/registry steps are in F1.
- **F4 — reference resolver and consumers.** `crates/engine/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:755-779` parses `[n]` and optional field; `[n]` alone returns the whole record and empty-index `[]` is not accepted by that scalar parser. `crates/engine/shamir-engine/src/query/batch/runner/param_subst.rs:239-271` resolves write markers and fails on unresolved references. `crates/engine/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:787-807` contains column resolution; both `crates/engine/shamir-engine-query/src/query/filter/evaluation/filter_node.rs:419-429` (IN) and `crates/engine/shamir-engine/src/query/batch/runner/query_runner/arms_read.rs:243-261` (ForEach) use column refs. This addendum does not infer that arbitrary column refs work in the proposed language equality subset.
- **F5 — null equality semantics.** `crates/engine/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:175-180` treats Null/Null as equal; unresolved `None` is distinct. This motivates excluding null/nullable predicate values rather than treating them as unresolved.
- **F6 — planner wave order.** `crates/query/shamir-query-types/src/ops/batch/plan/planner.rs:846-897` builds dependency waves and sorts within a wave by insertion order (`877-878`). `crates/engine/shamir-engine/src/query/batch/exec/batch_execute.rs:575-628` executes nested stages and aliases sequentially per the current plan. `crates/query/shamir-query-types/src/ops/batch/req/batch_limits.rs:164-177` sets default dependency depth 10. A backward dependency does not imply global source order.
- **F7 — first-stage compiler/artifact boundary.** Compiler metadata at compile result and atomic artifact persistence are new work; stage 1 touches only the compiler typed-builder helpers and no host import/engine DTO. The five-import main-design ceiling and six-import stage-2 design ceiling are design bounds, not observed compiler constants.
- **F8 — absence of response operation-kind tracking.** `crates/wasm/shamir-lang/src/back/emit/support/db.rs:34,246,248,542-545,641-714` defines the 1 MiB argument buffer, 50-operation/64-byte alias bounds, normal-return keep-order handling and ResponseGet walking, but no tracked Batch→Request→Response operation-kind metadata. The design's 8 MiB output capacity is in `docs/dev-artifacts/design/shamir-lang-design.md` §7. This document proposes adding compiler metadata; it does not claim the current helper already tracks kind.
- **F9 — legacy call gap.** `crates/wasm/shamir-wasm-host/src/wasm/bindings/call.rs:83-96,156-179` clones current DB/network-secret context and invokes nested functions without the proposed common per-target resolver. Stage 2 hardening is new work; legacy signature comparison is not required.
- **F10 — native builder markers and DTO.** `crates/query/shamir-query-builder/src/batch/batch.rs:194,220` provides `Batch::query`/`Batch::insert`; `crates/query/shamir-query-builder/src/read/val/filter_value.rs:157-186,171` normalizes builder query aliases/paths (`qref`); `crates/query/shamir-query-types/src/read/filter/filter_value.rs:87-99` constructs the native filter value without that normalization. `crates/query/shamir-query-types/src/ops/batch/ops/query_entry.rs:38-63` defines flattened operation/return/after/when serialization. `crates/query/shamir-query-types/src/ops/table_ref.rs:21-25,42-48` sets `TableRef::new` to main and emits a bare table for main.
- **F11 — diagnostics and error categories.** `T` diagnostic codes are compiler-static by the main design. Runtime kind 2 plan and kind 3 projection mappings in this addendum are proposed; current host gateway failures are not asserted to map that way.
- **F12 — call binding and catalog policy.** Atomic artifact/signature/resource binding and fail-closed missing-catalog behavior are accepted design decisions, not verified existing mechanisms. The separately looked-up management path is documented in F1.
- **F13 — mutation predicate scope.** Null equality currently compares equal per F5; a nonempty scalar equality record can still match all records. The compiler's ban on bare-scalar predicates is therefore an explicit safety rule, not evidence that the engine treats a scalar as a record filter.
- **F14 — scalar-reference Phase A and production gate.** `crates/wasm/shamir-lang/src/sem/check/builtins/batch/reference.rs` contains semantic checking for the narrow annotated-const record-local/direct-field subset, including plan provenance, scalar kind, and one-time eligible sink checks; `sem/tables.rs` carries private marker/sink metadata. `back/lower/expressions/record.rs` and `back/batch/reference.rs` lower marker cells, and `testsupport/wasm.rs` exposes hidden `compile_with_references` for test-only lowering. `api/compile.rs` retains the production T010 gate after semantics. Source tests include positive semantic/lowering and negative diagnostic cases; this does not claim those tests were run. Phase B guards and post-`db_execute` skipped-operation checks are not implemented.

### Not verified / design-only

- `call_checked`, signature persistence, coherent captured binding, security/grant rebuilding, shared stable active-call path, cancellation-safe unwind, legacy call hardening, and gateway context propagation/refusal are proposals, not implemented behavior.
- PROC-2c stage 3 Phase A semantic checking and testsupport marker lowering are present for the narrow subset described in §3. Production `compile` still rejects references with T010 after semantic checking; Phase B reference-specific guards and post-`db_execute` guest skipped-operation checks remain unimplemented, so no production runtime/value-flow, planner-wave, or ACL acceptance is claimed.
- The transactional batch `Call` deadlock claim is false for current source: the batch runner rejects the call before invocation (F1).
- Proposed kind-2 runtime plan and kind-3 projection mappings are not asserted as today's host error mapping.
- No build or test was run for this documentation-only update.
