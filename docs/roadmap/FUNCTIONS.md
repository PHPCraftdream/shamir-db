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
| 7 | `d1d3854` | env seeding into globals (`EnvPolicy`: SHAMIR_ ∪ all/mask/list, default SHAMIR_) under `env.`; atomic `incr`/`update` on globals (scc entry, no mutex). |
| 8a | `458771e` | **async bridge**: Wasmtime `async_support` + `ctx.call(name,args)` (function-calls-function) with a depth limit; shared batch/globals. |
| 8b | `f3fba3a` | **DB access** from functions: `ctx.db().table().get/insert/query` via `DbGateway` (autocommit, through `ShamirDb::execute`); persistence verified. |
| 8c | `d20fa90` | **egress**: `ctx.http_fetch/get/post` via a curl wrapper; SSRF guard + allowlist (deny-default); token in `-K` config (not argv); errors catchable by the guest. |
| 9 | `43497b8` | per-function **secret grants** (env.* read gated by grant; else absent) + visibility/security metadata stored in the catalogue. |

## Plan — remaining

### Slice 10 — wire DDL + wire-level e2e (not started)
- `BatchOp::{CreateFunction,DropFunction,RenameFunction,AlterFunction,ListFunctions}` in `shamir-query-types` (+ ser/de) and `execute_admin` dispatch → manage functions via the batch request API like other DDL.
- Wire-level e2e through client→server (TLS+SCRAM) exercising the full lifecycle.

### Deferred (by need, noted in code)
- **Full transactionality** of a function (RYOW/SSI inside one enclosing tx) — arrives when functions run as batch ops and inherit the batch tx; today DB access is autocommit per-op.
- **Permission ENFORCEMENT** (visibility public/private, `function:compile/execute/net`, invoker/definer with real principals) — rides the **Shomer access fabric** (see `ACCESS_FABRIC.md`); slice 9 already STORES the metadata + enforces secret-grants.
- Shared fuel budget across the `ctx.call` chain; epoch-yielding for CPU-bound guests; `patch`/`delete`/get-by-key on `DbGateway`; nested-call grant switching.

Note: the function engine was built end-to-end (compile → sandbox → catalogue
→ batch context/globals/env → compose → DB read/write → egress → secret
grants). The author surface stayed `async fn(ctx, batch, params) -> Result<Value>`
— user data only; bytes/memory/fuel/ABI generated and hidden.

## Implementation detail — slices 8c & 9

Guiding law (from the design contemplation): **capability ∝ constraint** —
the function that reaches the outside world, holds a secret, and writes the
truth-store is exactly the one that must be private + definer + one
secret-grant + one allowed host. Build the MINIMAL form each real function
needs; defer the rest.

### Slice 8c — egress (`ctx.http_fetch`) via a curl wrapper

1. **NetGateway trait** (`shamir-engine/src/function/net_gateway.rs`):
   `async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, String>`.
   `HttpRequest { method, url, headers: Vec<(String,String)>, body: Vec<u8> }`,
   `HttpResponse { status: u16, headers, body }`. Re-export. `FnCtx` gains
   `Option<Arc<dyn NetGateway>>` + `with_net(..)` builder + accessor; `new()` →
   None (egress host import then traps "no net gateway").
2. **SSRF guard / allowlist** (enforced in the gateway BEFORE any reach-out):
   parse the URL, extract host; check against an allowlist of host patterns
   (reuse the `*`-glob matcher from `EnvPolicy`). **Default = empty = deny-all.**
   Reject non-http(s) schemes and loopback/private IPs unless explicitly
   allowed. The allowlist is OURS, not curl's.
3. **CurlNetGateway** (facade-side, `shamir-db`): async wrapper over
   `tokio::process::Command::new("curl")`. `-sS`, `-X <method>`,
   `-w '%{http_code}'`, `--max-time`. **Token/secret headers via `-H @tmpfile`
   or `--config` (NEVER in argv** — argv is world-readable in `ps`). Body via
   `--data-binary @tmpfile` or stdin. Clean up temp files. `curl` absent →
   a clear `NetError`/`ToolchainUnavailable`-style error (core stays
   self-contained; egress is an optional, externally-delegated capability).
4. **Host import** (`wasm.rs`, async, on the 8a bridge): `http_fetch(req_ptr,
   req_len) -> packed` — decode `HttpRequest` (msgpack), `gateway.fetch().await`
   (which runs the allowlist guard), encode `HttpResponse`, alloc+write.
   `HostState` carries `Option<Arc<dyn NetGateway>>`; none → trap. Three-phase
   Caller-across-await dance (read → await → write), as in db imports.
5. **SDK** (`shamir-sdk`): `Ctx::http_fetch(req) -> Result<HttpResponse>`
   + ergonomic `get`/`post` helpers; `HttpRequest`/`HttpResponse` as Value
   maps. wasm32 extern shim + non-wasm stub.
6. **Facade**: an invoke variant wiring the `CurlNetGateway` with the
   configured allowlist (allowlist from `ShamirDb`/`ServerLauncher` config).
7. **Tests**: a local mock HTTP server (tiny tokio `TcpListener` with a canned
   response, or `httptest` dev-dep) on `127.0.0.1`, allowlist = `127.0.0.1`;
   a function fetches it and returns the body — assert. Plus an
   allowlist-DENY test (fetch a non-allowed host → error, no network needed).
   curl-presence-gated for the live fetch; toolchain-gated for compile.
   Deferred: streaming, retries, pooling, redirect policy.

### Slice 9 — permissions, visibility, secret grants

1. **Visibility** (catalog): add `visibility: public | private` to the
   `save_function` record + `create_function_*` (default **private** — safe).
   Loaded on open.
2. **Privileges** (reuse `shamir-server` RBAC + Grant/Revoke + audit chain —
   NOT a new subsystem): `function:compile` / `register` / `alter` / `drop` /
   `execute` / `net`. **Deny-by-default.**
3. **Authz checks**: create/alter/drop gated by compile/register/alter/drop;
   invoke gated by `execute` + visibility (public = any principal with
   execute; **private = only via `ctx.call` / the operator**); egress gated by
   `net` + allowlist. Audit-chain event on every function admin op + privilege
   exercise (the witnessing — nothing hidden).
4. **invoker / definer** (`security` field, default invoker): the effective
   principal at invoke is the caller (invoker) or the author (definer —
   controlled escalation, the priestly-gateway). Gateways consult the
   effective principal. (Principal-aware gateways are the refinement; MVP:
   definer runs with the definer's grants.)
5. **Per-function secret grants** (`secret_grants: Vec<String>` catalog
   field, default empty): the env/global host imports let a function read
   `env.X` only if `X` (or its prefix) is in its grants. The embedding token
   is granted to ONE function. Secret descends from the operator to one
   worthy vessel.
6. **Tests**: unauthorized create rejected; private function not user-callable
   but callable via `ctx.call`; a public definer-function performs a write the
   caller couldn't do directly; a function without the grant can't read
   `env.TOKEN`. **Capstone shape check**: the "re-embed" function is
   expressible as private + definer + `net` + one secret-grant + one allowed
   host — and nothing weaker reaches the token or the outside.

Order: 8c can ship first with allowlist-deny-default (safe standalone); 9
adds the `function:net` privilege + visibility + secret-grants on top. The
re-embed capstone needs BOTH. Each slice is delegated to `/crush`, verified
zero-trust, gated, committed.

## Method
`/crush` per slice → orchestrator re-runs the gate (`fmt --all --check`,
`clippy --workspace --all-targets -D warnings`, the touched test suites) and
inspects the diff + asserts non-vacuous tests before committing. Toolchain-gated
tests (compile-from-source) skip cleanly when cargo/wasm32 is absent.
