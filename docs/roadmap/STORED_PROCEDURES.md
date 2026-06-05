בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Stored Procedures — callable getter-functions

**Status:** design / proposed (revision 2026-06-05).

A **procedure-getter** is a WASM function invoked as a **top-level batch
operation** — `{ "call": "fn_name", "params": [1, 2, "value"] }` — that
returns a result (object / array / scalar / empty) and may also write the
DB. It runs **server-side**, so it can check rights / time / business
state and shape the answer **without round-trips** to the client.

It is the same machinery as the function calls we already embed in
`select` / `set` / `upsert` / `insert` / `delete` / filters / group-by —
only now the function is also a **first-class batch entry**, callable as a
request and participating in the batch dependency graph (its result can
feed other queries; its params can reference other results).

> Companion: [`FUNCTIONS.md`](./FUNCTIONS.md) (the WASM engine "M"),
> [`ACCESS_FABRIC.md`](./ACCESS_FABRIC.md) (setuid / getter-only),
> [`QUERY_BUILDER.md`](./QUERY_BUILDER.md), [`PLAN.md`](./PLAN.md).

---

## 0. What already exists (the foundation — ~80% done)

The heavy lifting is already shipped — a procedure call is "almost there":

- **`ShamirDb::invoke_function_in_db_as(db, repo, name, params, actor)
  -> QueryValue`** (`shamir-db/src/shamir_db/shamir_db.rs` ~1699) — runs a
  WASM function in a DB context with an explicit actor. This *is* the
  procedure call; the rest is wiring it to the batch surface.
- Functions **read/write the DB** via `DbGateway` (`ctx.db().query/get/
  insert`) → `FacadeDbGateway` → `execute_as`. "May update the DB" — done.
- Functions return **`QueryValue`** — object / array / scalar / null.
  Exactly the four answer shapes required.
- **setuid / `effective_fn_actor`** — a procedure runs with its owner's
  rights (the getter-only data-firewall): rights/time/business checks live
  inside the procedure, server-side. The caller needs only `Execute` on
  the function, not `Read` on the tables it touches.
- **`ctx.call`** — procedures can call other functions (depth-bounded).
- Functions are already invoked **inside** other ops (`FnCall` `$fn` in
  select/filter/set/group). The user's analogy to aggregate functions is
  exact — `call` is the same, lifted to a top-level batch entry.
- Function writes already emit the **changefeed** (Phase 3b / #177) — free.
- Sandbox (fuel + memory limits) + the perf work (InstancePre / AOT cache
  / pooling) already make invocation cheap.

---

## 1. What's missing (the thin wire + batch glue)

1. **`CallOp` DTO + `BatchOp::Call` variant** — wire shape
   `{ "call": "fn_name", "params": [...], "repo": "main" }` (serde
   discriminator key `call`). Functions are not yet a standalone batch
   entry.
2. **`FunctionInvoker` trait in the executor** — mirror of `AdminExecutor`.
   The engine executor does not know the facade's
   `invoke_function_in_db_as` (that lives in `shamir-db`). Inject a channel:
   `QueryRunner::run` sees `BatchOp::Call` → `invoker.invoke(name, params,
   actor) -> QueryResult`. `shamir-db` implements the trait via
   `invoke_function_in_db_as`.
3. **Result mapping `QueryValue → QueryResult`** — see fork §2.A.
4. **Batch dependency-graph participation** — `CallOp.params` may carry
   `$query` refs (another result → a procedure param); and the Call's
   result must be referenceable by later ops (`call`-alias → its
   `QueryResult` in `resolved_refs`). This makes procedures full citizens
   of the batch.
5. **Builder + macros** — `Batch::call(alias, name, params)` and
   `q!(call fn_name(1, 2, "value"))`.
6. **Params format** — see fork §2.B.

---

## 2. Forks to decide before starting

### A. Result format — `QueryResult.value` vs records-mapping
`QueryResult` today is `{ records: Vec<Value>, stats, pagination }`.
- **Option A1 (recommended): add `value: Option<serde_json::Value>`** to
  `QueryResult`, serde-skipped when `None`. A procedure puts a non-tabular
  answer (scalar / object) in `value`; a tabular answer stays in
  `records`. Clean, unambiguous, backward-compatible.
- **Option A2: map into `records`** — object→`[obj]`, array→`arr`,
  scalar→`[scalar]`, empty→`[]`. No type change, but scalar-vs-single-row
  is ambiguous to the client.

→ Recommend **A1** (`value` field): it expresses scalar/object/empty
honestly and leaves `records` for genuinely tabular procedure output.

### B. Params — positional vs named
`{ "call": ..., "params": [1, 2, "value"] }` is **positional**, but the
guest ABI takes `Params` (a named map).
- **Option B1 (recommended): positional** — the wire `params` array is
  passed to the guest as a `QueryValue::Array` (procedure reads
  `params[0]`, `params[1]`, …). Matches the requested `{call, params:[…]}`
  shape directly.
- **Option B2: named** — `params: { "id": 1, "kind": "x" }`. More verbose
  on the wire but self-documenting.

→ Recommend **B1** (positional) as the primary form; optionally also
accept a named object later. The builder can offer both ergonomically.

### C. Side-effect transactionality
A procedure writes via `DbGateway` = `execute_as` (autocommit). Inside a
`transactional` batch, those writes are **not** part of the batch's tx
(same open question as DDL-in-tx).
- **MVP (recommended): autocommit** — a getter mostly reads; its writes
  commit independently. Document the boundary.
- **Later:** thread the batch's `TxContext` into the procedure's
  `DbGateway` so its writes join the tx. Separate effort (couples the
  function gateway to tx state).

→ Recommend **autocommit for MVP**, tx-integration as a follow-up.

---

## 3. Phased implementation plan

### Phase 1 — Wire + core (the callable procedure)
- `CallOp` DTO (new module, e.g. `query-types/src/call/` or beside
  read/write): `{ call: String, #[serde(default)] params: Vec<FilterValue>,
  #[serde(default = "default_repo")] repo: String }`. (`FilterValue` so
  params can be literals **or** `$query` refs — reuses the universal
  expression vocabulary.)
- `BatchOp::Call(CallOp)` + serde (discriminator key `"call"`) +
  `is_admin()`=false, `table_ref()`=None (it's neither DML nor DDL).
- `FunctionInvoker` trait (`shamir-engine` executor):
  `async fn invoke(&self, op: &CallOp, actor: &Actor, resolved_refs)
  -> Result<QueryResult, BatchError>`. Inject like `AdminExecutor`.
- `QueryRunner::run`: dispatch `BatchOp::Call` → resolve params (literals +
  `$query` refs from `resolved_refs`) → `invoker.invoke(...)`.
- `shamir-db` impl of `FunctionInvoker` → `invoke_function_in_db_as(db,
  op.repo, op.call, params, actor)`; map `QueryValue` → `QueryResult`
  (fork §2.A — recommend `value` field).
- **Result format**: implement fork §2.A (add `QueryResult.value`).
- e2e: a batch with a single `call` returning each of object / array /
  scalar / empty; a setuid getter-only procedure returning filtered data
  to a caller without table `Read`.

### Phase 2 — Dependency-graph participation
- `BatchPlanner::extract_dependencies`: add a `Call` arm — scan
  `op.params` for `$query`/`FieldRef`/`QueryRef` (reuse
  `extract_deps_from_filter_value`).
- Make the Call's `QueryResult` available to later ops via
  `resolved_refs` (it already flows through `all_results` keyed by alias —
  verify `$query` resolution reads a Call result the same as a Read).
- e2e: `call check_access(@user.id)` → its result used in the `where` of a
  following query; topo-order correct; cycle/unknown-alias guarded.

### Phase 3 — Builder + macros
- `Batch::call(alias, name, params)` (params: `impl IntoIterator<Item =
  impl Into<FilterValue>>`), returning a `Handle` (so its result is
  referenceable like any other).
- `q!` statement form: `q!(call fn_name(1, 2, "value"))` and with refs
  `q!(call check(@users.first().id))`. Extend the `q!` grammar
  (`query_parse.rs`) with a `call` head.
- Tests + migrate any hand-rolled call-style tests to the builder/macros.

### Phase 4 — (optional) polish
- `describe`/introspection: list callable procedures + their declared
  shape; named-params variant (fork §2.B option) if wanted.
- Tx-integration of side effects (fork §2.C "later").

---

## 4. Cross-cutting concerns
- **Rights:** already solved — `effective_fn_actor`/setuid. The batch
  actor → effective actor → procedure reads as owner. Caller needs only
  `Execute` on the function.
- **Changefeed:** procedure writes (via `execute_as`) already emit events
  (#177) — observability/replication for free.
- **Sandbox/cost:** fuel + memory limits already bound a runaway
  procedure; depth-limit bounds `ctx.call` recursion.
- **Errors:** a procedure error surfaces as `BatchError::QueryError` with
  the alias (consider a structured `code` like the DDL error model #178).
- **Read-only hint (future):** a procedure declared read-only could be
  rejected if it attempts a write — useful for "getter" guarantees.

---

## 5. Files to change
- `crates/shamir-query-types/src/` — new `CallOp` (e.g. `call/` module);
  `batch/types.rs` (`BatchOp::Call` + serde + `is_admin`/`table_ref`);
  `batch/planner.rs` (`extract_dependencies` Call arm);
  `read/query_result.rs` (`value` field, fork §2.A).
- `crates/shamir-engine/src/query/batch/executor.rs` — `FunctionInvoker`
  trait + dispatch in `QueryRunner::run`; thread invoker like `admin`.
- `crates/shamir-db/src/shamir_db/execute.rs` — `FunctionInvoker` impl →
  `invoke_function_in_db_as`; `QueryValue → QueryResult` mapping.
- `crates/shamir-query-builder/src/` — `Batch::call`; `q!` `call` head in
  `shamir-query-builder-macros/src/query_parse.rs`.

---

## 6. Verdict
~80% is already done — execution, DB access, setuid rights, `QueryValue`
results, `ctx.call` all work via `invoke_function_in_db_as`. The remaining
work is a **thin slice**: `BatchOp::Call` + an invoker channel into the
executor + result mapping + dependency-graph participation + builder/macro
sugar. Not a from-scratch feature — procedures become citizens of the
batch, right next to DML and DDL.

Suggested order: **Phase 1 (wire+core) → Phase 2 (deps) → Phase 3
(builder/macros)**, each the project way (research → implement →
zero-trust verify → green gate → scoped commit). Decide forks §2.A
(`value` field) and §2.B (positional params) before Phase 1.
