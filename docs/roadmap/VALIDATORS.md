# Table validators (WASM CHECK / BEFORE-write hooks)

**Status:** design / proposed (session task).

## Idea

A **validator** is a WASM function bound to a table that fires **before** a
record is inserted or updated. It receives the candidate record (and, for
updates, the previous record) plus the caller identity, and returns
**accept** or **reject(reason)**. A rejecting validator aborts the write with
its reason; in a transaction the tx rolls back.

This is the WASM-native equivalent of SQL `CHECK` constraints + `BEFORE INSERT
/ UPDATE` triggers — **data integrity enforced at the database layer**, not
just in application code. It is the natural complement to the access fabric:

| Concern | Mechanism |
|---|---|
| *Who* may write *which* rows | RLS / Shomer DAC (per-row, per-table) |
| Is a field-tuple *unique* | unique index |
| Is an indexed *computed value* | functional index (`IndexExpr`) |
| **Is the row itself *valid*** | **validator (this doc)** |

It also reuses what we already have: the function system (`ShamirFunction` /
WASM modules), the per-invocation context (`FnCtx` with the effective
`Actor`, globals, params — wired in Stage B-2), and the existing **validation
stage** on the write path (`validate_unique_for_create` in `insert_tx` /
`insert` / `execute_set`). A validator slots in right there.

## Model

A validator is just a **function** (from the function library) bound to a
table lifecycle event:

```
ALTER TABLE users ADD VALIDATOR /myapp/validate_user ON INSERT, UPDATE
ALTER TABLE users DROP VALIDATOR /myapp/validate_user
```

- **Signature (conceptual):** `(record, old_record?, ctx) -> [ValidationError]`
  - `record` — the candidate row (the NEW value).
  - `old_record` — the previous row, present only for UPDATE (lets a validator
    enforce transitions, e.g. "status may not go active → deleted directly").
  - `ctx` — effective `Actor` (`ctx.caller`), function params, globals.
  - **Return: a list of validation errors. An empty list means accept.** A
    validator does NOT return a single scalar reason — it returns *all* the
    problems it found, each bound to a field.
- **Binding** is stored in the table catalogue (info_store), exactly like
  index descriptors: a small list of `(function_ref, events)` per table.
- **Composition (collect-all):** a table may have several validators; the
  engine runs them and **aggregates every error from every validator** before
  failing the write — so the client sees *all* problems in one round-trip
  (form-style UX), not just the first. (A cheap fast-path may skip remaining
  validators once some errored, but the default is collect-all.)

## Error model — structured, field-bound

A validator returns zero or more **structured** errors, each tied to a field
path so a client can highlight the offending field and show every problem at
once:

```
ValidationError {
    field:   Option<FieldPath>,   // ["address","zip"] / nested / array-index segment; None = record-level
    code:    String,              // stable, machine-readable: "too_short", "invalid_format", "out_of_range"
}
```

- `field` is a **path into the record** (the same `FieldPath` model used by
  filters / functional indexes): top-level, nested (`["address","city"]`), or
  an array element (`["items","2","sku"]`). `None` for whole-record rules
  ("at least one contact method required").
- `code` is the machine-readable key (for i18n / programmatic handling).
  Human-readable messages live on the frontend (i18n by code); the DB returns
  only codes — no locale dependency, no message bloat on the wire.

### WASM ABI convention (no new ABI)

A validator is an ordinary function returning a `QueryValue`; the result is
**interpreted** as the error list — so we add a *convention*, not a new ABI:

- `null` or an empty array `[]` → **valid**.
- an array of objects
  `[{ "field": ["address","zip"], "code": "invalid_zip" }, …]`
  → those errors (a bare string element may be sugar for a record-level error
  with that string as the code).

The engine decodes that `QueryValue` into `Vec<ValidationError>`.

### Wire response

On any aggregated errors the write fails with a **dedicated structured
response** — e.g. `DbResponse::ValidationFailed { errors: [{field, code}, …] }` — distinct from a generic error, so SDKs/clients can render
per-field. Inside a tx the tx aborts (no partial write).

## Execution

- **Where:** in the write stage, **after** the RLS/permission check and
  **before** persistence — the same point as `validate_unique_for_create`.
  Per-record for batch inserts; old+new for updates.
- **How:** invoke the bound function via `invoke_function_in_db_as(actor)`
  with the record as a param and `ctx.caller` set. CPU-bound WASM runs under
  `spawn_blocking` (already the case for function calls).
- **Reject → error:** maps to a `BatchError`/`DbResponse` error carrying the
  validator's reason; inside a tx the tx aborts and rolls back (no partial
  write).

## MVP scope — **pure** validators

For the first cut, validators are **pure / read-only**:

- They see `record`, `old_record?`, params, globals, and `ctx.caller`.
- They **must not** write to the DB (no DB gateway, or a read-only gateway).
- They **CAN call other functions** — both WASM and built-in library functions
  (via the function registry in `FnCtx`). A validator that needs to check a
  regex, compute a hash, validate an email format, or call a custom WASM
  helper simply invokes it. The full function catalogue is available; only
  DB-write operations are blocked.
- They should be **deterministic** (same input → same verdict).

This keeps three things simple and safe:

1. **No re-entrancy / recursion** — a validator on table `T` can't trigger
   writes (and thus other validators) on `T`, avoiding loops and deadlocks on
   the batch/unique locks.
2. **WAL-replay safety (critical).** Validators run on the **original** write
   only — the WAL entry is written *post-validation*. On crash recovery, WAL
   replay **does not** re-run validators (the record already passed when first
   accepted; re-running could change the verdict if the validator code or its
   inputs changed, or if it were non-deterministic). The durable log is the
   source of truth; validation is an admission gate, not a replay step.
3. **Predictable performance** — a pure function per record is bounded; no
   surprise fan-out of DB reads/writes.

## Errors & semantics

- A rejection is the **structured, field-bound error list** above — never a
  panic. The op (or tx) fails cleanly with `ValidationFailed { errors }`.
- **Validator-found errors** (the returned list) are bound to fields and
  aggregated across all validators (collect-all).
- **Validator *invocation* failure** (WASM trap, missing function, undecodable
  return) is a *separate* failure mode with a distinct code — **fail-closed**:
  a broken validator blocks the write rather than silently letting it through.
  It is NOT folded into the field-error list (it's an operator/deploy fault,
  not a data-validity fault).

## Performance

- Opt-in per table — tables without validators pay nothing.
- One WASM call per record per write; mitigated by `spawn_blocking` + the
  shared wasmtime engine. High-throughput bulk loads on validated tables should
  expect the per-row cost; document it.

## Future extensions (explicitly out of MVP)

- **Mutating / transform validators** (BEFORE-trigger style): return a
  *modified* record — set defaults, normalise (lowercase email, trim), compute
  derived fields, stamp `created_at`/`updated_at`. Powerful but needs care with
  determinism, indexing, and replay (the *stored* value must be the
  transformed one, and replay must not re-transform).
- **Cross-row / cross-table validators** with a **read-only** DB gateway
  (e.g. referential checks). Requires careful re-entrancy + isolation rules.
- **AFTER hooks** (post-commit notifications / audit) — a different lifecycle
  point with different (eventual) semantics.

## Use-case: sequenced writes (CAS by previous-hash)

A motivating, fully-expressible example of what validators unlock: **optimistic
compare-and-swap on a record's content hash** — a write to a key is accepted
only if the caller presents the correct hash of the *current* version. It
protects the **sequence** of a record's history — no lost updates, and a
verifiable linear chain `prev_hash → hash` — "blockchain without the consensus",
just a tamper-evident order. It needs **no new engine**; it composes from what
exists plus the validator hook.

### The layers

1. **Atomicity (already present — MVCC + SSI).** A CAS must be atomic:
   "read old → compare hash → write" under two concurrent writers would
   otherwise lose an update. The engine already provides it:
   - **Serializable (SSI)** transactions — a concurrent write to the read-set
     aborts with `transaction.status = "aborted"`, `reason = "tx_conflict"`.
   - The per-key write serialisation already used for unique-index validation
     (the same stage a validator runs in).
   Either makes the check-then-write atomic; no new "chain engine" is needed.

2. **Hashing (already present — `shamir-funclib` `/crypto`).** `blake3` /
   `sha256` are built in. The one new detail to pin down is a **canonical,
   deterministic record encoding** for hashing (stable field order, excluding
   the `hash` field itself). The storage form (MessagePack over interned ids)
   is nearly canonical; the rule just needs fixing.

3. **The gate (this doc — a validator).** A validator is the only hook that
   sees both `old_record` and `new_record` and can call any function (incl. the
   hash). The rule is exactly CAS:
   ```
   hash(old_record) == new_record.expected_prev_hash  ?  accept : reject("stale")
   ```
   Fail-closed, bound to the field that carries the expected hash.

### MVP boundary (pure) vs future (transform)

- A **pure / read-only validator (MVP)** can **enforce** the gate — reject when
  `expected_prev_hash != hash(old_record)`. That alone delivers
  sequence-protection.
- It cannot **stamp** the new `hash`/`seq` into the stored record (it's
  read-only). Two paths:
  - **now:** the client computes the new version's hash (via `$fn` /
    `crypto/blake3` computed-write-values) and stores it; the next writer must
    present exactly that hash, so the chain still holds (a wrong client hash
    simply fails the next write).
  - **later:** **transform / mutating validators** (a documented future
    extension above) let the *server* canonically hash and stamp
    `hash`/`prev_hash`/`seq` — the "correct" variant, but beyond MVP-pure
    validators.
- Note: inline `$fn` computed-write-values **cannot** implement the gate — a
  `$ref` only sees the new record's literal fields, never the stored old
  record. Only a validator reads `old_record`. (Computed values can still
  *produce* the new hash; the validator *checks* the link.)

### Verdict

Achievable on **existing** machinery (MVCC/SSI for atomicity + funclib
`blake3`/`sha256` for the hash) **plus** the planned validators (the gate;
ideally transform-validators for server-side stamping). It is a thin layer over
the validator hook, not new infrastructure — the only genuinely new piece is a
**canonical record-hash** definition.

## Why it's a good fit (beauty)

The implementation is a **thin layer**: a binding table (`table → [validator
refs]`) in the catalogue + a hook in the existing write-validation stage that
invokes the bound functions with the candidate record and maps reject → error.
No new execution engine — validators are functions (the same library, the same
`FnCtx`/`Actor` from Stage B-2) bound to a table event. Maximal reuse, minimal
new machinery, integrity at the right layer.

---

# Implementation design (v1) — concrete

This is the agreed operational design to build against. A validator is a
**superset of a function**: same compile pipeline (Rust source → WASM), same
`FnCtx`, same catalogue storage shape — plus a stable id, a special return
convention, and per-table bindings.

## Identity: `RecordId`, resolved from a unique name

- A validator's id is its catalogue record's `_id` (`shamir_types … RecordId`,
  `[u8;16]`). **No new id type.**
- Clients refer to a validator by **name** (unique, like function names —
  uniqueness enforced at create / rename). At bind time the engine resolves
  `name → RecordId` and stores the **id** in the binding. Renaming the validator
  therefore never breaks a binding (bindings reference the id, not the name).
- Catalogue storage mirrors functions: a row in a system table keyed by `name`,
  carrying its `_id`. (Functions already store this way — `TABLE_FUNCTIONS`.)

## Registries & storage

- **`ValidatorRegistry`** (separate from `FunctionRegistry`; lock-free `scc`):
  holds `id (RecordId) → compiled validator`, plus a `name → id` index and a
  `bound_in: Set<TableRef>` per validator. Validators are invoked by the engine
  on the write path, NOT via `invoke_function`, so they are a distinct namespace.
- **Global catalogue** (`SystemStore`, like functions): the validator definition
  (id, name, compiled artifact / source, `FunctionMeta`, `bound_in`). Loaded at
  init → registered into `ValidatorRegistry`.
- **Per-table bindings = the info-twin.** New `MetaKey::Validators` →
  `__meta__/validators` in each table's `info_store` (mirrors
  `index2/persistence.rs` `MetaKey::Indexes`):
  ```rust
  struct PersistedValidators { bindings: Vec<ValidatorBinding> }
  ```

## Types

```rust
pub type ValidatorId = RecordId;  // the catalogue record's _id

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteOp { Insert, Update, Upsert, Delete }

/// Stored in the table info-twin.
pub struct ValidatorBinding {
    pub validator_id: ValidatorId,          // resolved from name at bind time
    pub ops: SmallVec<[WriteOp; 4]>,        // which ops it fires on
    pub priority: u16,                       // 1000..=9999, lower = earlier
}

/// One field-bound error (codes only — no human text).
pub struct ValidationError {
    pub field: Option<FieldPath>,            // None = record-level
    pub code: String,
}
```

## Validator ABI (return convention — no new ABI, an interpretation)

A validator is an ordinary WASM function whose returned `QueryValue` is decoded
as (MessagePack map with these fields):
```
{ "errors": [ { "field": ["address","zip"], "code": "invalid_zip" }, … ],
  "stop": false }
```
- `null` / `[]` / `{ "errors": [] }` → valid, continue.
- a bare array → those errors with `stop = false` (sugar).
- `stop = true` → **halt the remaining validators** for this op (the stopping
  validator's own errors are still reported).

Input params: `record` (the NEW value), `old_record` (present for
Update/Upsert/Delete; for Delete the row being removed), and `ctx` (effective
`Actor`, params, globals, and access to **all** functions — funclib + WASM).
MVP validators are **pure / read-only** (no DB writes) — see "MVP scope".

## The pass (per record, per op) — runs at the `validate_unique_for_create` stage

1. Read the table's bindings (cached in memory from the info-twin).
2. Keep those whose `ops` contains the current `WriteOp`.
3. Sort by `priority` ascending; stable tie-break by id.
4. Resolve each `validator_id` in `ValidatorRegistry`. **Missing → abort the
   whole request** with a distinct code (fail-closed; a binding must always
   resolve).
5. Invoke (CPU-bound WASM under `spawn_blocking`) with `(record, old?, ctx)`;
   decode `errors` + `stop`.
6. Accumulate errors (collect-all). On `stop = true`, break the loop.
7. After the loop: any errors → fail with `DbResponse::ValidationFailed
   { errors }`; inside a tx the tx aborts (no partial write).
8. A validator **invocation** failure (trap / undecodable return) is a separate
   fail-closed code (operator fault), NOT folded into the field-error list.

WAL-replay does NOT re-run validators (admission gate, not a replay step).

## DDL — new admin ops (mirror `create_index` / `drop_index`)

- `create_validator { name, source|wasm, <function opts> }` → compile + assign
  `_id` + persist to catalogue + register. Name must be unique.
- `drop_validator { name }` → resolve name→id; **refuse if `bound_in` is
  non-empty** (report the bound tables); else remove from registry + catalogue.
- `bind_validator { table, name, ops, priority }` → resolve name→id, validate
  `priority ∈ [1000,9999]`, append `ValidatorBinding` to the table info-twin,
  `bound_in += table`.
- `unbind_validator { table, name }` → remove the binding, `bound_in -= table`.
- `list_validators { table? }` → introspection (global catalogue or one table's
  bindings).
- `rename_validator { from, to }` → re-key by name (uniqueness on `to`); id and
  bindings unchanged.

## Stages

- **S0 — Types & schema** (no behaviour): `WriteOp`, `ValidatorBinding`,
  `ValidatorDef`/meta-superset, `ValidationError`, the ABI decoder,
  `MetaKey::Validators`, `PersistedValidators`. (Types first.)
- **S1 — Global catalogue + `ValidatorRegistry`**: SystemStore load/save/remove;
  facade `create_validator` / `drop_validator` / `rename_validator` (compile +
  persist + register + `bound_in` refusal); create/drop wire-ops.
- **S2 — Per-table bindings (info-twin)**: persist/load `PersistedValidators`;
  in-memory binding list per table; `bind_validator` / `unbind_validator` ops
  (existence + range checks + `bound_in` upkeep); load with the table.
- **S3 — Validator pass on the write path**: hook beside
  `validate_unique_for_create` for insert/update/upsert/delete; gather → sort →
  resolve (fail-closed) → invoke (`spawn_blocking`) → decode → collect-all →
  stop; `DbResponse::ValidationFailed`; tx-abort.
- **S4 — SDK signature**: the validator signature in `shamir-sdk`(+macros)
  compiling to the ABI; examples.
- **S5 — Tests + e2e**: unit (priority order, stop-the-loop, fail-closed missing,
  collect-all, `bound_in` drop-refusal, binding round-trip) + e2e via
  `ShamirDb::execute` (create → bind → violating write → `ValidationFailed`).
