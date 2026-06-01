בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Function engine — staged roadmap

User-defined functions (the "M" in S.H.A.M.I.R.). Each slice is delegated to
a `/crush` sub-agent, then verified zero-trust by the orchestrator and
committed. Authoring principle (non-negotiable): **the author works only with
user data** — `async fn(ctx, batch, params) -> Result<Value>`; bytes / linear
memory / fuel / msgpack / ABI / storage form are generated and hidden.

The architecture is one descent: a single `Value` light filling nested vessels
of decreasing lifetime — env (given from above) ⊃ durable settings ⊃ process
globals ⊃ batch context ⊃ call locals — each boundary mediated by measure
(sandbox, fuel, memory, permission). The function stands at the bottom and
reaches up through its `.await` seams.

## Done

| Slice | Commit | Content |
|---|---|---|
| 1 | `9136a5f` | Execution model: `ShamirFunction` `(ctx,batch,params)->Value`, lock-free `FunctionRegistry`, `argon2id` on `spawn_blocking`. |
| 2 | `0cb27fc` | Wasmtime backend: `WasmFunction` + fuel/memory limits + guest ABI (hidden seam). |
| 3 | `09fc181` | `shamir-sdk` + `#[shamir::function]` macro (hides the ABI) + `compile_rust_source` (Rust→wasm). |
| 4 | `0b1d349`,`f014110` | Durable catalogue + load-on-open + `ShamirDb` lifecycle API + e2e (create→compile→use→rename→use→delete). |
| 5 | `ad1f299` | `BatchContext` (per-batch scratch) + `GlobalVars` (process-global), real `FnBatch`/`FnCtx`, facade wiring. |
| 6 | `022336c` | Synchronous WASM host imports: guests read/write batch context + globals (`shamir_host` module). |

## Plan

### Slice 7 — env seeding + atomic globals (small)
- `EnvPolicy` (composable): `SHAMIR_` prefix ALWAYS, plus optional `all` / prefix masks / explicit name list (unioned). Default = `SHAMIR_` only.
- Seed matching `std::env::vars()` into `GlobalVars` under the `env.` namespace at `ShamirDb::init` (snapshot at start; OS is the source of truth). env = secrets/tokens/config; globals = caches/runtime state.
- Atomic `update` / `incr` on `GlobalVars` (scc entry API) so counters don't lose updates — no mutex (concurrency invariants).
- Durable globals deferred (cheap later: reuse system-store settings + a per-key `durable` flag). YAGNI.

### Slice 8 — the async bridge (the big one)
Enable Wasmtime `async_support` + fibers; the host-call seam becomes async. Three seams of one nature ride it:
- **8a** async execution model + `ctx.call(name, params)` (function-calls-function): shared batch/globals/tx, **shared fuel budget + depth limit** (bounds recursion).
- **8b** **DB access** from functions: `ctx.db()/repo()/store()` host imports routed through the current `TxContext` (RYOW, read/write-set, SSI, predicate locks). The function is the transaction continued.
- **8c** **egress** `ctx.http_fetch(req)` with host allowlist + `function:net` permission (SSRF/exfiltration surface — trusted-only default). Unblocks the "function calls an embedding AI with an env token, writes vectors back" capstone.

### Slice 9 — permissions & security
- Privileges `function:compile` / `register` / `alter` / `drop` / `execute` / `net`; deny-by-default; Grant/Revoke; audit-chain events.
- `security: invoker | definer` (controlled escalation).
- Per-function **secret grants**: which `env.`/tokens a given function may read (not all functions see all secrets).

### Slice 10 — wire DDL + wire-level e2e
- `BatchOp::{CreateFunction,DropFunction,RenameFunction,AlterFunction,ListFunctions}` in `shamir-query-types` (+ ser/de) and `execute_admin` dispatch → manage functions via the JSON request API like other DDL.
- Wire-level e2e through client→server (TLS+SCRAM) exercising the full lifecycle.

## Method
`/crush` per slice → orchestrator re-runs the gate (`fmt --all --check`,
`clippy --workspace --all-targets -D warnings`, the touched test suites) and
inspects the diff + asserts non-vacuous tests before committing. Toolchain-gated
tests (compile-from-source) skip cleanly when cargo/wasm32 is absent.
