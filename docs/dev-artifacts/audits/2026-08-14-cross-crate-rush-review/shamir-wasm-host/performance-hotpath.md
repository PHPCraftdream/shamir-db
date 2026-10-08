<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-wasm-host — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Unbounded globals, compiler buffering, engine retention, and redundant copies remain structurally demonstrated. Latency multipliers and two Wasmtime-specific optimization premises are not verified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Pinned Wasmtime's async linker requires boxed futures; do not retain removing Box::pin as a directly implementable fix.

<a id="review-1"></a>

### Claim 1 — Unbounded process-lifetime growth of `GlobalVars` via guest `global_set`

Status: `confirmed-open`. Current risk: `medium`.

Non-env guest writes reach a shared host map without cardinality or byte quota. The sanctioned ABI has no removal import, and per-Store guest memory limits do not bound retained host entries across invocations.

Evidence: [crates/shamir-wasm-host/src/wasm/host_globals.rs:55](../../../../../crates/shamir-wasm-host/src/wasm/host_globals.rs#L55); [crates/shamir-wasm-host/src/context.rs:164](../../../../../crates/shamir-wasm-host/src/context.rs#L164); [crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs:87](../../../../../crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs#L87).

Grouping/duplicate: `SUMMARY.md#4.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy

Status: `confirmed-open`. Current risk: `medium`.

call still clones the entire parameter map, serializes it into Bytes, and copies those bytes into a Vec before copying into guest memory. Allocation/copy presence is proven; the claimed threefold workload cost is unmeasured.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:407](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L407); [crates/shamir-types/src/types/value.rs:33](../../../../../crates/shamir-types/src/types/value.rs#L33); [crates/shamir-types/src/types/value.rs:52](../../../../../crates/shamir-types/src/types/value.rs#L52).

Grouping/duplicate: `SUMMARY.md#4.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Unbounded stdout/stderr buffering of the guest `cargo build`

Status: `confirmed-open`. Current risk: `low`.

Both readers accumulate unrestricted Vec buffers and stderr enters an allocated error message. Hostile source can induce large diagnostics; guest-supplied build scripts are not supported by this compile interface.

Evidence: [crates/shamir-wasm-host/src/compile.rs:571](../../../../../crates/shamir-wasm-host/src/compile.rs#L571); [crates/shamir-wasm-host/src/compile.rs:579](../../../../../crates/shamir-wasm-host/src/compile.rs#L579); [crates/shamir-wasm-host/src/compile.rs:619](../../../../../crates/shamir-wasm-host/src/compile.rs#L619).

Grouping/duplicate: `SUMMARY.md#4.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Epoch-ticker thread + full engine leaked per `WasmEngine::new()` — no shutdown path

Status: `confirmed-open`. Current risk: `low`.

Each construction detaches an endless thread holding an Engine clone. Database initialization creates an engine per database; benchmark setup also creates engines repeatedly. Engine resources cannot be released while the ticker retains them.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:155](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L155); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:165](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L165); [crates/shamir-engine/benches/wasm_invoke.rs:153](../../../../../crates/shamir-engine/benches/wasm_invoke.rs#L153).

Grouping/duplicate: `SUMMARY.md#4.4`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — `host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc`

Status: `confirmed-open`. Current risk: `low`.

host_call clones the existing grants Arc, then iterates cloned Strings into with_secret_grants, which constructs another set and Arc. These allocations are unnecessary for unchanged inherited grants.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:84](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L84); [crates/shamir-wasm-host/src/wasm/host_call.rs:119](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L119); [crates/shamir-wasm-host/src/context.rs:395](../../../../../crates/shamir-wasm-host/src/context.rs#L395).

Grouping/duplicate: `SUMMARY.md#4.5`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `BatchContext::put` / `GlobalVars::set` pay two hash ops per write (remove_sync + insert_sync)

Status: `confirmed-open`. Current risk: `low`.

Both writes still perform separate remove and insert lookups, unlike the existing single-entry RMW path. This proves redundant map operations and a visibility gap, not a measured speedup from replacement.

Evidence: [crates/shamir-wasm-host/src/context.rs:52](../../../../../crates/shamir-wasm-host/src/context.rs#L52); [crates/shamir-wasm-host/src/context.rs:166](../../../../../crates/shamir-wasm-host/src/context.rs#L166); [crates/shamir-wasm-host/src/context.rs:228](../../../../../crates/shamir-wasm-host/src/context.rs#L228).

Grouping/duplicate: `SUMMARY.md#2.2`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Dead `Arc` clones on every `batch_get` / `global_get` host import

Status: `confirmed-open`. Current risk: `nit`.

Both successful read handlers clone batch and globals handles that are used only in discard expressions. The extra clones remain; absent or denied reads return before reaching them.

Evidence: [crates/shamir-wasm-host/src/wasm/host_batch.rs:54](../../../../../crates/shamir-wasm-host/src/wasm/host_batch.rs#L54); [crates/shamir-wasm-host/src/wasm/host_batch.rs:90](../../../../../crates/shamir-wasm-host/src/wasm/host_batch.rs#L90); [crates/shamir-wasm-host/src/wasm/host_globals.rs:95](../../../../../crates/shamir-wasm-host/src/wasm/host_globals.rs#L95); [crates/shamir-wasm-host/src/wasm/host_globals.rs:130](../../../../../crates/shamir-wasm-host/src/wasm/host_globals.rs#L130).

Grouping/duplicate: `SUMMARY.md#4.6`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Host-import export re-resolution + `typed()` rebuild per call

Status: `confirmed-open`. Current risk: `nit`.

Repeated export resolution and typed-function construction are visible, with no handles cached in HostState. The alleged O(exports) scan and its latency are unverified without resolved Wasmtime implementation evidence.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:333](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L333); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:349](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L349); [crates/shamir-wasm-host/src/wasm/host_batch.rs:57](../../../../../crates/shamir-wasm-host/src/wasm/host_batch.rs#L57).

Grouping/duplicate: `SUMMARY.md#4.7`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `Box<dyn Future>` heap allocation per nested async host import

Status: `not-applicable`. Current risk: —.

Parent inspection of checksummed Wasmtime 46.0.2 shows Linker::func_wrap_async requires a for<'a> closure returning Box<dyn Future<Output=Args> + Send + 'a>. The visible boxing is required by this pinned API; the proposed direct unboxed closure is incompatible. An alternative API/architecture is optional optimization and has no measured benefit here.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:44](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L44); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:197](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L197); [Cargo.lock:4741](../../../../../Cargo.lock#L4741).

Pinned dependency evidence: [wasmtime 46.0.2, src/runtime/linker.rs:581](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/linker.rs).

Grouping/duplicate: `SUMMARY.md#4.8`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Redundant artifact copies in `maybe_wasm_opt`

Status: `confirmed-open`. Current risk: `nit`.

The owned compiled artifact is passed by slice and copied on unavailable/failed optimization paths. Ownership-preserving pass-through remains possible; this is off-hot-path allocation cleanup.

Evidence: [crates/shamir-wasm-host/src/compile.rs:638](../../../../../crates/shamir-wasm-host/src/compile.rs#L638); [crates/shamir-wasm-host/src/compile.rs:655](../../../../../crates/shamir-wasm-host/src/compile.rs#L655); [crates/shamir-wasm-host/src/compile.rs:688](../../../../../crates/shamir-wasm-host/src/compile.rs#L688).

Grouping/duplicate: `SUMMARY.md#4.9`. This row is not another independent defect.

## Corrections and qualified non-findings

- GlobalVars retention is scoped to the database's shared globals instance, not necessarily one universal process-global map.
- A removal import alone does not protect against a guest that refuses to remove entries; a quota is necessary for adversarial retention control.
- read_to_end is unbounded, but arbitrary tenant build scripts cannot be added to the generated crate. Use source-controlled diagnostic volume as the supported threat scenario.
- String::from_utf8_lossy borrows valid UTF-8 stderr; it does not always allocate a second full-size buffer. Formatting the final error does allocate.
- The 768-GiB virtual-reservation number is a configuration/comment estimate, not verified allocation or RSS.
- The benchmark creates fresh engines per iteration in some setup paths, not merely once per scenario.
- Repeated export lookup is confirmed; linear lookup complexity is not.
- The impl-Future recommendation must be checked against the exact Wasmtime API before becoming a task.
- The crate retains InstancePre and pooling/CoW setup, has no in-crate benchmark target, and its invocation path has no identified full-registry scan. Strict aggregate fuel and unconditional wall-clock guarantees are not established.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wasm-host -- Performance & O(x->0)

## Summary

The crate is structurally sound under pillar 3: `scc`/Fx-hash primitives everywhere, `scc::*::len()` correctly annotated with `// O(N) ack:` allowances, pooling allocator + `InstancePre` to amortize instantiation, and no hidden full-scan on the per-invocation path. The real O(x->0) gaps are two unbounded-growth vectors (process-lifetime `GlobalVars` writable by guests with no quota and no guest-side removal import; unbounded pipe buffering in the compile pipeline) and a small set of avoidable per-invocation copies/allocations in `WasmFunction::call` (deep params clone + double buffer copy) and `host_call` (secret-grants set rebuilt per nested call). The hot-path cost model is otherwise bounded by design: fuel + epoch + wall-clock cap per-call work, and per-batch `BatchContext` is scope-limited. No in-crate benches exist (the `wasm_invoke` bench lives in `shamir-engine`); tests cover limit correctness, not cost.

## Findings

1. **Unbounded process-lifetime growth of `GlobalVars` via guest `global_set`**
   - File:line: `crates/shamir-wasm-host/src/context.rs:164-168` (via `crates/shamir-wasm-host/src/wasm/host_globals.rs:55`)
   - Severity: medium
   - Issue: `host_global_set` forwards arbitrary guest-supplied keys/values into the process-lifetime, shared-across-all-batches `scc::HashMap` with no entry-count or byte-size quota, no eviction, and — critically — no host import exposing removal (`global_remove` exists only on the native `FnCtx`, not in the linker surface). Every distinct key a guest ever writes persists for the life of the process, outside guest memory (so the per-Store 64 MiB `ResourceLimiter` does not bound it).
   - Failure scenario: a malicious or buggy guest function loops `global_set(format!("k{i}"), ...)` with distinct keys. Each top-level call gets a fresh fuel budget and 30 s of wall clock, so aggregate host RSS grows monotonically across calls and eventually OOMs the server; even benign functions permanently leak every transient key they set. The crate's own sanitizer doc treats guests as untrusted bytecode, so this is within the stated threat model.
   - Suggested fix: add a cardinality/byte cap (e.g. `AtomicUsize` mirror of `scc` len per pillar 3, enforced in `GlobalVars::set` with a trap on overflow), or expose a `global_remove` host import plus per-function namespacing so entries are reclaimable when a function is dropped.

2. **Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy**
   - File:line: `crates/shamir-wasm-host/src/wasm/wasm_function.rs:407-410`
   - Severity: medium
   - Issue: every `WasmFunction::call` does `QueryValue::Map(params.raw().clone())` — a full deep clone of the `TMap<String, QueryValue>` (every key String and every nested value recursively cloned) — then `to_bytes()` (full msgpack encode), then `.to_vec()` (a third full copy of the encoded `Bytes`). Only `.len()` and `copy_from_slice(&input)` are used afterwards, so the `.to_vec()` is pure waste (`Bytes` derefs to `&[u8]`).
   - Failure scenario: the contract doc says the same function serves the `where` / `set` / key-generation sites, i.e. per-row invocation. With a large param map (bulk row payload) over N rows, the host churns ~3×N×payload bytes per query where ~1×N×payload is required; for where/set-per-row workloads this is hidden linear overhead stacked under every WASM call (each row also pays Store setup, so this compounds).
   - Suggested fix: drop the `.to_vec()` (keep `Bytes` and pass `&input`); eliminate the deep clone by encoding from a borrowed view (a `Params::to_bytes()` helper that serializes the map without re-owning it, or constructing the `QueryValue::Map` once at the call site and threading it through).

3. **Unbounded stdout/stderr buffering of the guest `cargo build`**
   - File:line: `crates/shamir-wasm-host/src/compile.rs:571-586` (readers), `:614-622` (error path)
   - Severity: low
   - Issue: the two pipe-drainer threads `read_to_end` into unbounded `Vec<u8>`s, and the failure path additionally does `String::from_utf8_lossy(&stderr)` (a second full-size allocation) embedded into a `FunctionError::Compute` string. The 120 s `wait_timeout` bounds duration but not volume — a hostile or pathological build script can emit GBs of output within the window.
   - Failure scenario: an authorized actor compiles a guest whose build script spams stderr in a tight loop; host memory balloons to the pipe output size ×2 (raw + lossy copy) before the timeout kills the child.
   - Suggested fix: drain with a bounded cap (read into a fixed-size ring/limited buffer, e.g. first 64 KiB + total-bytes counter) and truncate what goes into the error string.

4. **Epoch-ticker thread + full engine leaked per `WasmEngine::new()` — no shutdown path**
   - File:line: `crates/shamir-wasm-host/src/wasm/wasm_engine.rs:154-163`
   - Severity: low
   - Issue: every `WasmEngine::new()` spawns a detached, never-terminating ticker thread and builds a full wasmtime `Engine` (JIT infra, disk cache worker, 128-slot pooling allocator with ~768 GiB virtual reservation). The doc justifies the leak by "engines here are long-lived singletons", but nothing enforces that: `shamir-db` constructs one per `ShamirDb` instance (`core.rs:165`), and `shamir-engine/benches/wasm_invoke.rs:153` constructs one per bench scenario.
   - Failure scenario: an application that opens/closes databases repeatedly (per-tenant DBs, test suites, CLI one-shots) accumulates one thread + one pooled engine per open for the whole process lifetime; VA reservation and thread count grow without bound.
   - Suggested fix: share one engine via a process-wide `OnceLock<Arc<WasmEngine>>` (engines are config-identical here), or give `WasmEngine` a `Drop`/shutdown signal (e.g. `Arc<AtomicBool>` checked by the ticker) so the thread dies with the engine.

5. **`host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc`**
   - File:line: `crates/shamir-wasm-host/src/wasm/host_call.rs:119` (`with_secret_grants(secret_grants.iter().cloned())`)
   - Severity: low
   - Issue: `FnCtx::with_secret_grants` does `Arc::new(grants.into_iter().collect())`, so each nested `ctx.call` re-allocates a fresh `TFxSet` and re-inserts all G grants, even though the parent's `Arc<TFxSet<String>>` was already cloned into `secret_grants` at `host_call.rs:84` and could be shared as-is. (`repo` is likewise re-cloned per nested call; unavoidable with the current `String` field but cheap to switch to `Arc<str>`.)
   - Failure scenario: a guest recursion/fan-out chain of depth D with G grants performs D set builds + D `Arc` allocations + D×G string clones per request instead of D `Arc` refcount bumps; small today (depth limit 32) but it is per-call allocation in the recursion loop for zero benefit.
   - Suggested fix: add a `with_secret_grants_arc(Arc<TFxSet<String>>)` builder (pub(crate), like `with_fuel_budget`) and use it in `host_call`.

6. **`BatchContext::put` / `GlobalVars::set` pay two hash ops per write (remove_sync + insert_sync)**
   - File:line: `crates/shamir-wasm-host/src/context.rs:50-54` (and `:164-168` for `GlobalVars`)
   - Severity: low
   - Issue: upsert is emulated as `remove_sync(&key)` followed by `insert_sync(key, value)` — two bucket-lock acquisitions and two hashes per put on the `batch_put` host-import hot path (guests can put in a tight loop), where a single-pass upsert through the entry API does one. Note (correctness aside for the sibling reviewer): the two-step form is also non-atomic — a concurrent `get` between remove and insert observes the key absent, the very race `update()`'s doc claims cannot happen.
   - Failure scenario: a guest loop of N `batch_put` calls pays 2N lock ops instead of N; concurrent readers can transiently miss a key mid-put.
   - Suggested fix: implement `put` via `entry_sync` (occupied → `get_mut`/insert into entry; vacant → `insert_entry`) for one hash op and atomicity; measure, since `entry_sync` requires an owned key upfront.

7. **Dead `Arc` clones on every `batch_get` / `global_get` host import**
   - File:line: `crates/shamir-wasm-host/src/wasm/host_batch.rs:53-55, 89-90`; `crates/shamir-wasm-host/src/wasm/host_globals.rs:94-96, 130`
   - Severity: nit
   - Issue: both handlers clone `Arc<BatchContext>` and `Arc<GlobalVars>` out of `caller.data()` purely for "borrow-scope hygiene" and then discard them via `let _ = (batch, globals);` — two atomic refcount bump/decrement pairs per call of the most loop-friendly host imports.
   - Failure scenario: none functional; constant-factor waste on guest read loops.
   - Suggested fix: delete the clones (the borrow conflict they worked around no longer exists — `alloc_fn`/`alloc_typed` are obtained after the data reads) and drop the `let _` suppressors.

8. **Host-import export re-resolution + `typed()` rebuild per call**
   - File:line: `crates/shamir-wasm-host/src/wasm/wasm_function.rs:333-341, 349-352` (`write_value_to_guest`/`write_bytes_to_guest`); `crates/shamir-wasm-host/src/wasm/host_batch.rs:57-61, 76-79`
   - Severity: nit
   - Issue: every host import that writes back into guest memory re-runs `get_export("shamir_alloc")` + `get_export("memory")` and rebuilds a `TypedFunc` from the untyped `Func` — a scan of the instance export table plus wrapper construction per call (twice per `batch_get`/`global_get`: once for alloc, once for memory re-acquisition).
   - Failure scenario: guest loops hammering `batch_get`/`global_get` pay a fixed export-lookup tax on every iteration; N exports makes each lookup O(exports).
   - Suggested fix: resolve `shamir_alloc`/`memory` once in `WasmFunction::call` (or at first use) and cache the `TypedFunc`/`Memory` handles in `HostState`; fall back to the current path if absent.

9. **`Box<dyn Future>` heap allocation per nested async host import**
   - File:line: `crates/shamir-wasm-host/src/wasm/host_call.rs:44` (pattern shared by `host_db.rs:19, 67, 113, 169`, `host_http.rs:117`)
   - Severity: nit
   - Issue: the `func_wrap_async` handlers return `Box<dyn Future ... + Send + '_>` — one heap allocation per db/call/http import invocation — where a concrete `impl Future + Send + '_` return type on the free function is accepted by wasmtime and keeps the future inline.
   - Failure scenario: constant-factor allocation per host-import call inside guest loops; negligible next to the decode/encode work, hence nit.
   - Suggested fix: return `impl Future` (naming the concrete async-block type) from the handler functions; keep `Box` only if a MSRV/toolchain constraint forces it.

10. **Redundant artifact copies in `maybe_wasm_opt`**
    - File:line: `crates/shamir-wasm-host/src/compile.rs:651-655, 688`
    - Severity: nit
    - Issue: on every compile without `wasm-opt` installed (the common case) the artifact is copied once via `wasm_bytes.to_vec()` despite `wasm_bytes` already being an owned `Vec` upstream; the same copy recurs on wasm-opt failure arms.
    - Failure scenario: none beyond a full extra copy of the (small, `opt-level="z"` + LTO) artifact at DDL/compile time — off the request hot path.
    - Suggested fix: pass/return `Vec<u8>` by value and `std::mem::take` in the pass-through arms.

</details>
