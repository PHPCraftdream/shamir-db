בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Native ↔ WASM Function-Parity Campaign — Phase 0 Seam Findings

**Status:** READ-ONLY investigation (Phase 0 foundation).
**Scope:** Pin down the five integration seams a later phase will touch, with
exact file:line references and the concrete code shape each consumer needs.
**Verified this session** against the current tree; line numbers may drift on
future edits — re-derive with the symbol names if so.

The DB unifies user artifacts behind **one** trait:

```rust
// crates/shamir-wasm-host/src/contract.rs:21
#[async_trait]
pub trait ShamirFunction: Send + Sync {
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue>;
}
```

Both WASM modules (`WasmFunction`) and native Rust types (`Argon2idFunction`,
and in tests `CasValidator`, `AddProc`, etc.) already implement it. Two planes
consume `ShamirFunction`:

| Plane | Type held | Lives in |
|---|---|---|
| Procedural (functions) | `Arc<dyn ShamirFunction>` in `FunctionRegistry` | `shamir-wasm-host/src/registry.rs:20` |
| Procedural (validators) | `Arc<dyn ShamirFunction>` in `ValidatorRegistry` | `shamir-engine/src/validator/registry.rs:34` |
| Scalar (filters/indexes) | pure `fn(&[QueryValue]) -> ScalarResult` in `ScalarRegistry` | `shamir-funclib/src/registry.rs` |

Phase 0 only touches the **catalogue row plumbing** for the procedural plane.
The scalar plane is out of scope for this campaign phase.

---

## Seam 1 — Catalogue persistence (where `ArtifactKind` lives)

### 1.1 There is NO serde struct — rows are schema-less `QueryValue::Map`

The function and validator catalogue rows are not modelled as typed Rust
structs. They are built inline as a `QueryValue::Map(TMap<String, QueryValue>)`
at the moment of persistence and round-tripped through the generic
`SetOp`/read path of the system store. **This is the dominant fact for Phase
0:** there is no `#[derive(Serialize, Deserialize)] struct FunctionRow` to add
a `#[serde(default)]` field to. Migration safety has to live in a **decode
helper** that treats a missing `kind` key as `Wasm`, not in serde defaults.

### 1.2 Function row build site

`crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:147–175`:

```rust
let mut m = shamir_types::types::common::new_map();
m.insert("name",      QueryValue::Str(name));            // :148
m.insert("wasm_b64",  QueryValue::Str(wasm_b64));        // :152
m.insert("wasm_hash", QueryValue::Str(wasm_hash));       // :156
m.insert("lang",      QueryValue::Str(lang_tag));        // :160   ("wasm" or "rust")
m.insert("source",    Str | Null);                       // :164
m.insert("version",   QueryValue::Int(1));               // :171
let mut record = QueryValue::Map(m);
meta.inject_into(&mut record);                            // adds visibility/security/secret_grants
```

Persisted by `SystemStore::save_function(name, &record, &meta)` →
`crates/shamir-db/src/shamir_db/system_store.rs:473–492`, which upserts into
the table named `TABLE_FUNCTIONS = "functions"` (`system_store.rs:32`), in the
`SYSTEM_REPO = "system"` keyspace (`system_store.rs:18`).

### 1.3 Validator row build site

`crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:116–148`:

```rust
let mut m = new_map();
m.insert("name",      QueryValue::Str(name));             // :117
m.insert("_id",       QueryValue::Str(id.to_string()));   // :121  (RecordId)
m.insert("wasm_b64",  QueryValue::Str(wasm_b64));         // :125
m.insert("wasm_hash", QueryValue::Str(wasm_hash));        // :129
m.insert("lang",      QueryValue::Str(lang_tag));         // :133
m.insert("source",    Str | Null);                        // :137
m.insert("bound_in",  QueryValue::List(vec![]));          // :144  (filled at bind-time)
let record = QueryValue::Map(m);
self.system_store.save_validator(name, &record, &meta).await?;
```

Persisted by `SystemStore::save_validator` → `system_store.rs:855–874`, into
`TABLE_VALIDATORS = "validators"` (`system_store.rs:40`).

### 1.4 WASM bytes — stored as base64 string, not raw bytes

Field `"wasm_b64"` (type `QueryValue::Str`), produced at
`function_management.rs:134` / `validator_management.rs:99`:

```rust
let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm);
```

Plus a hex `fxhash::hash64` digest in `"wasm_hash"` (`:135` / `:100`).

> **Implication for native artifacts:** a native function has no WASM bytes.
> Phase ≥1 will need to make `wasm_b64` optional on `Native` rows (or store a
> sentinel). The `kind` field introduced in Phase 0 is what later code will
> branch on to skip the `base64::decode` + `WasmFunction::from_binary` step
> for `Native` rows.

### 1.5 Boot materialisation (load → register)

`crates/shamir-db/src/shamir_db/shamir_db/core.rs` — `init_with_env_policy`,
inside the closure that builds the `ShamirDb`:

- **Functions** — `core.rs:223–274`:
  `load_functions()` → for each `rec`, decode `wasm_b64`, `WasmFunction::from_binary`,
  `functions.register(&name, Arc::new(wf))`. Failures (`name` missing,
  `wasm_b64` missing, b64 error, compile error) all `log::warn!` and `continue`
  — boot is resilient to per-row corruption.
- **Validators** — `core.rs:278–356`: same shape + reconstructs `RecordId` from
  `_id` (`:306`) and restores `bound_in` (`:340–345`).

> **Integration point for Phase ≥1:** the boot loop is where `kind` will be
> read to dispatch: `Native` rows skip `from_binary` and instead rehydrate a
> native artifact (probably via a `NativeFunctionFactory` registered by the
> embedding application, keyed by name). Phase 0 only adds the field; nothing
> reads it yet.

### 1.6 Keyspace isolation

Functions and validators live in **separate tables** under the same `system`
repo. They do not share a row type — `kind` must be written into BOTH build
sites (1.2 and 1.3) and read by BOTH boot loops.

| Artifact  | Table          | Repo     | Key     | Constant                       |
|-----------|----------------|----------|---------|--------------------------------|
| Function  | `"functions"`  | `"system"` | `name`  | `system_store.rs:32`           |
| Validator | `"validators"` | `"system"` | `name`  | `system_store.rs:40`           |

---

## Seam 2 — Host-side validation result encode

### 2.1 Decoder location + signature

```rust
// crates/shamir-engine/src/validator/decode.rs:29
pub fn decode_validation_result(v: &QueryValue)
    -> Result<ValidationOutcome, ValidatorDecodeError>;
```

Re-exported at `crates/shamir-engine/src/validator/mod.rs:18`. Returns
`ValidationOutcome { errors: Vec<ValidationError>, stop: bool }` (defined at
`crates/shamir-engine/src/validator/validation_outcome.rs:7`).

### 2.2 Exact accepted shapes (three)

The decoder accepts **three** top-level shapes — a host-side encoder for
native validators may produce any of them; the simplest is `Null`:

| Shape | Meaning |
|---|---|
| `QueryValue::Null` | accept, no errors, `stop = false` |
| `QueryValue::List(items)` | reject; each item is a `ValidationError`; `stop = false` |
| `QueryValue::Map` with `"errors"` | reject; map form, structured |

For the Map form the contract is:

```
{
  "errors": [ <err>, ... ],   // REQUIRED, must be a List
  "stop":   bool              // OPTIONAL, defaults to false
}
```

Each `<err>` is either a bare `Str` (record-level, `field = None`) or a `Map`
with `"code": Str` (required) and optionally `"field": List<Str>` / `"field": Null`.

**There is no `valid` boolean and no `Ok`/`Err` variant.** Success is encoded
as the absence of errors (`Null` root, or a Map/List with empty `errors`).

### 2.3 The only existing encoder is guest-side

`crates/shamir-sdk/src/validation.rs:156` — `Validation::into_value(self) -> Value`
(SDK `Value`, msgpack-wire-compatible mirror of `QueryValue`). It always emits
the **Map form**, even on accept:

```rust
Value::Map(vec![
    ("errors".to_owned(), Value::List(error_list)),
    ("stop".to_owned(),   Value::Bool(self.stop)),
])
```

This matches the decoder's map branch exactly.

### 2.4 No host-side production encoder exists today

There is **no** function in `shamir-engine/src/` or `shamir-db/src/` that
builds this `QueryValue` shape. The only host-side construction is in test
code: `crates/shamir-db/tests/validators_e2e.rs:112` (`rejection_single_error`)
hand-builds a `QueryValue::Map` to feed a WAT module's return slot.

> **Integration point for Phase 1:** `register_native_validator(|new, prev,
> ctx| -> Validation)` needs a host-side `Validation -> QueryValue` encoder.
> The cleanest move is to lift the SDK `into_value` body into a host-visible
> `pub fn validation_to_query_value(v: Validation) -> QueryValue` (in
> `shamir-engine::validator`, next to `decode_validation_result`), have the
> SDK call into it via the existing `shamir-sdk` ↔ `shamir-engine` re-export
> seam, and have the native closure adapter call it directly. The shape is
> already pinned by the decoder contract in §2.2.

### 2.5 Call site

`crates/shamir-engine/src/table/table_manager_validators.rs:238–261`:

```rust
let result = validator.call(&ctx, &batch, &params).await;          // :238
match result {
    Ok(value) => {
        let outcome = decode_validation_result(&value).map_err(...)?; // :250
        all_errors.extend(outcome.errors);                            // :258
        if outcome.stop { break; }                                    // :261
    }
    Err(fn_err) => return Err(ValidatorFailure::Invocation { ... }),  // :242
}
```

The `value` returned by `validator.call(...)` is fed to
`decode_validation_result` **by reference, untransformed**. So a native
validator that returns the right `QueryValue` shape via `ShamirFunction::call`
works through the identical path — no special-casing of the invocation site
is required for native validators.

---

## Seam 3 — SDK proc-macros

### 3.1 Four attribute macros, all WASM-guest-only

`crates/shamir-sdk-macros/src/lib.rs` (single file):

| Macro        | Line     | Expected guest fn shape |
|---|---|---|
| `#[validator]`  | `:43–44`  | `async fn(Value, Option<Value>, Ctx) -> Validation` |
| `#[function]`   | `:175–176`| `async fn(Ctx, Batch, Params) -> Result<Value>` |
| `#[procedure]`  | `:306–307`| `async fn(Ctx, Params) -> Result<Value>` |
| `#[scalar]`     | `:463–464`| `async fn(Params) -> Result<Value>` |

All four are `#[proc_macro_attribute]`. No derives, no function-like macros.

### 3.2 Emitted shape — `#[validator]`

None of the macros emit `impl ShamirFunction`. They emit raw WASM guest
exports. For `#[validator]` (`lib.rs:91–150`) the expansion is:

```rust
// 1. user fn renamed to __shamir_impl_<name>(record, old_record, ctx) -> Validation
// 2. two C-ABI exports injected into the guest module:
#[no_mangle] pub extern "C" fn shamir_alloc(len: i32) -> i32 { /* bump alloc */ }
#[no_mangle] pub extern "C" fn shamir_call(ptr: i32, len: i32) -> i64 {
    // msgpack-decode params → record/old_record/ctx
    // block_on(__shamir_impl_*(...))
    // validation.into_value() → msgpack encode → leak_result as packed (ptr,len) i64
}
```

`#[function]`/`#[procedure]`/`#[scalar]` follow the same skeleton with their
own arg arity; the non-validator ones call `__rt::trap` on `Err` (WASM trap),
whereas `#[validator]` always returns a value.

### 3.3 Not reusable for native registration

The macros hard-code: `#[no_mangle] extern "C"` exports, `shamir_sdk::__rt`
(msgpack + leak + block_on), and the WASM guest ABI. They cannot produce a
host-side `impl ShamirFunction for ...`.

> **Integration point for Phase ≥2 (native ergonomics):** a *new* macro is
> needed if we want `#[native_validator] fn ...` to emit `impl ShamirFunction
> for ...` + a registration helper. The simplest first cut is no macro at all
> — Phase 1's `FnAdapter<F>` (see Seam 5) plus a one-line
> `db.validators().register_native(...)` is enough. Macro ergonomics can come
> later and reuse the SDK's argument-extraction helpers (`params.get(...)`,
> `Validation::into_value`) — those are plain Rust and not WASM-bound.

---

## Seam 4 — `FunctionRegistry` vs `ValidatorRegistry`

### 4.1 `FunctionRegistry` definition

```rust
// crates/shamir-wasm-host/src/registry.rs:20
pub struct FunctionRegistry {
    functions: scc::HashMap<String, Arc<dyn ShamirFunction>, THasher>,
}
```

Re-exported as `shamir_engine::function::FunctionRegistry` via
`crates/shamir-engine/src/lib.rs:12` (`pub use shamir_wasm_host as function;`)
and `crates/shamir-wasm-host/src/lib.rs:44`. **Held as `Arc<FunctionRegistry>`
in `ShamirDb`** (`crates/shamir-db/src/shamir_db/shamir_db/core.rs:59`),
instantiated at `core.rs:115` via `FunctionRegistry::with_builtins()`.

### 4.2 Key API (file:line in `shamir-wasm-host/src/registry.rs`)

| Method     | Line  | Signature |
|---|---|---|
| `register`   | `:41` | `(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) -> FnResult<()>` |
| `replace`    | `:50` | `(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>)` |
| `get`        | `:57` | `(&self, name: &str) -> Option<Arc<dyn ShamirFunction>>` |
| `contains`   | `:62` | `(&self, name: &str) -> bool` |
| `rename`     | `:73` | `(&self, from: &str, to: &str) -> FnResult<()>` |
| `invoke`     | `:109`| `async (&self, name, ctx, batch, params) -> FnResult<QueryValue>` |

### 4.3 Differences vs `ValidatorRegistry`

| Axis | FunctionRegistry | ValidatorRegistry (`validator/registry.rs:34–41`) |
|---|---|---|
| **Key** | `name: String` (name IS identity) | `RecordId` (catalogue identity); name is a secondary index |
| **Maps** | 1: `functions` | 3: `by_id`, `name_to_id`, `bound_in` |
| **Binding scope** | none — global | per-table (`bound_in: HashMap<RecordId, BTreeSet<String>>`, keys are `"db/repo/table"`; `drop` is refused while bound — `is_bound:146`) |
| **Replace** | `replace` (`:50`): remove + insert keyed by name, returns `()` | `replace_artifact` (`:93`): `scc::HashMap::update` keyed by `RecordId`, in-place, preserves name+bindings, returns `bool` |
| **Rename** | re-key (remove + insert) | touches only `name_to_id`; `by_id` and bindings untouched |

> **Integration point:** a native function slot already exists in
> `FunctionRegistry` — `register(name, Arc<dyn ShamirFunction>)` is type-agnostic.
> For validators, the existing native path is the awkward
> `register(id, name, Arc::new(wf))` then `replace_artifact(id, native)` dance
> used by `tests/cas_sequenced_e2e.rs:154`. Phase 1 should add a first-class
> `register_native_validator(name, factory)` on the `ValidatorRegistry` (or its
> `ShamirDb` facade) that handles `RecordId` allocation + binding bookkeeping
> internally and writes a `kind = Native` catalogue row.

---

## Seam 5 — Closure adapter feasibility

### 5.1 Verdict: blanket impl does NOT compile; a concrete `FnAdapter<F>` IS required

A blanket

```rust
impl<F, Fut> ShamirFunction for F
where F: Fn(&FnCtx, &FnBatch, &Params) -> Fut, ...
```

fails Rust's coherence rules: `ShamirFunction` is defined in `shamir-wasm-host`,
so the blanket would apply to *all* `F: Fn(...)` including types downstream
crates might also impl `ShamirFunction` for — that's an overlap the checker
rejects at the blanket site. The `#[async_trait]` desugaring (boxed `Pin<Box<dyn
Future + Send>>` with lifetimes tied to `&self` and the arg refs) makes the
lifetime relationship between `Fut` and the borrow non-expressible in a blanket.

There is no existing blanket impl in the workspace (verified by search).
Today's 11 impls are all concrete named types (`WasmFunction`,
`Argon2idFunction`, plus test types `Const`, `CasValidator`, `AddProc`,
`TableReader`, etc.).

### 5.2 Signature that compiles — `FnAdapter<F>`

```rust
// proposed home: crates/shamir-wasm-host/src/fn_adapter.rs
use crate::context::{FnBatch, FnCtx};
use crate::contract::ShamirFunction;
use crate::error::FnResult;
use crate::params::Params;
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::future::Future;

pub struct FnAdapter<F>(pub F);

#[async_trait]
impl<F, Fut> ShamirFunction for FnAdapter<F>
where
    F: Fn(&FnCtx, &FnBatch, &Params) -> Fut + Send + Sync,
    Fut: Future<Output = FnResult<QueryValue>> + Send,
{
    async fn call(&self, ctx: &FnCtx, batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        self.0(ctx, batch, params).await
    }
}
```

Required bounds:
- `F: ... + Send + Sync` — because `ShamirFunction: Send + Sync` and `FnAdapter<F>` auto-derives both from `F`.
- `Fut: ... + Send` — because `async_trait` boxes the future as `Pin<Box<dyn Future + Send>>`.
- `call` body just forwards: `self.0(ctx, batch, params).await`.

Usage: `registry.register("my_fn", Arc::new(FnAdapter(|ctx, batch, params| async move { ... })))`.

> **Integration point for Phase 1:** the validator form needs a *different*
> adapter because the user closure has signature `|new: &QueryValue, prev:
> Option<&QueryValue>, ctx: &FnCtx| -> Validation` (human-friendly), not the
> raw `ShamirFunction::call` shape. That adapter (call it
> `NativeValidatorAdapter`) will, inside its `call` impl, extract `record` /
> `old_record` from `params` (`params.get("record")`, see §2.5 call site),
> invoke the user closure, then encode the resulting `Validation` to
> `QueryValue` via the host-side encoder from Seam 2.4. The seam this depends
> on — `params` carrying `record`/`old_record` keys — is already pinned by
> `table_manager_validators.rs:228–237`.

---

## Phase 0 foundation change (companion to this doc)

**Single addition:** an `ArtifactKind` enum + a `from_record` decode helper
that defaults missing/unknown values to `Wasm`. Written into both catalogue
build sites (Seam 1.2, 1.3). **No read sites change** — Phase 0 is purely
additive; later phases consume `ArtifactKind::from_record(&rec)` at the boot
loops (Seam 1.5) and at any place that needs to branch on origin.

**Migration safety** rests on the `from_record` default, NOT on serde: the
existing persisted rows have no `kind` key, and `from_record(&old_row) ==
ArtifactKind::Wasm` for all of them. Verified by a unit round-trip test in
`crates/shamir-db/src/shamir_db/tests/artifact_kind_tests.rs`.
