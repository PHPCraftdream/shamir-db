<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wasm-host — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Most concrete source mechanisms remain unchanged. Fuel accounting, compiler execution/lifecycle, header loss, malformed HTTP decoding and retained globals warrant follow-up. Several current open labels instead describe optional design or unreachable future failures. Exact dependency inspection resolves previously uncertain lookup/trap semantics and exposes additional caller and ABI defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 69 | 61 | 0 | 0 | 1 | 0 | 7 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Aggregate fuel budget is enforced retroactively: in-flight chain can draw (depth+1) × fuel; doc overclaims the bound

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A parent consumes fuel, suspends in call, and the child loads the still-undebited shared balance. Both retain independent grants; even a finite returning parent/child pair can exceed the advertised aggregate budget. Epoch/time limits do not restore instruction accounting.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:438](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L438); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:587](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L587); [crates/shamir-wasm-host/src/wasm/host_call.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L120).

<a id="review-1-2"></a>

### Claim 1.2 — `fuel > i64::MAX` immediately errors AND makes the wall-clock/epoch test vacuous

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A fresh call with fuel=u64::MAX seeds -1 and returns before creating the Store. The registered test accepts that error and would not detect removal of epoch interruption. Public custom limits expose the bug; production defaults do not exceed i64::MAX.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:436](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L436); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:246](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L246); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L258).

<a id="review-1-3"></a>

### Claim 1.3 — `wasm_aggregate_fuel_exhausted_across_nested_calls` cannot distinguish aggregate from per-Store fuel — vacuous regression test

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

The WAT recursively calls itself without a successful base case. expect_err is satisfied by depth, per-Store fuel or epoch failure after reverting shared accounting. A finite workload that succeeds under reset-per-Store behavior is needed; exact loop fuel counts were not measured.

Evidence: [crates/shamir-wasm-host/src/tests/wasm_tests.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L120); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:313](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L313).

<a id="review-1-4"></a>

### Claim 1.4 — `GlobalVars::seed_env` panics on any non-UTF-8 environment variable

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

seed_env enumerates Unicode entries before applying policy, including excluded entries. Database initialization calls it directly. Invalid Unicode can panic; abort is not unconditional. Pinned behavior: [Rust 1.94.0 env source](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/env.rs).

Evidence: [crates/shamir-wasm-host/src/context.rs:211](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L211); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L168); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

<a id="review-1-5"></a>

### Claim 1.5 — Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Custom function limits above 64 MiB remain accepted while the pool imposes the default ceiling. Wasmtime 46.0.2 explicitly caps memory.grow at max_memory_size; on-demand lacks this pool cap. This concerns the public custom-limit API, not default DDL limits.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:190](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L190); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:458](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L458); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741).

<a id="review-1-6"></a>

### Claim 1.6 — Forbidden-macro scanner desyncs on the '\'' char literal

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The escape branch stops at the escaped quote in the four-byte apostrophe literal, leaving its actual closing quote unblanked. That violates the stripping contract. A compilable file-read bypass attributable specifically to this defect is not established.

Evidence: [crates/shamir-wasm-host/src/compile.rs:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L334); [crates/shamir-wasm-host/src/compile.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L234).

<a id="review-1-7"></a>

### Claim 1.7 — blocking `compile_rust_source` on async paths

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Both async creation routes directly call the synchronous compiler; uploaded binaries also undergo inline Wasmtime compilation.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L172); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:232](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L232).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — non-atomic remove-then-insert in registry/context stores

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Readers can enter the remove/insert gap. Registration can prevent replacement, and a target collision plus source reoccupation prevents rename restoration. Discarded insertion errors remain.

Evidence: [crates/shamir-wasm-host/src/registry.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L52); [crates/shamir-wasm-host/src/registry.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L86); [crates/shamir-wasm-host/src/context.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L166).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — contradictory `net_grants` doc contract

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Options still promise full inheritance; the user-function empty-grant branch actually denies. This is documentation divergence, not implemented fail-open behavior.

Evidence: [crates/shamir-wasm-host/src/meta.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L189); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:841](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L841).

Grouping/duplicate: [SUMMARY.md#3.2](SUMMARY.md#review-3-2). This is not an additional independent defect.

<a id="review-1-10"></a>

### Claim 1.10 — `decode_http_request` silent defaults

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A POST with a Str body is accepted with an empty body. Missing method/url and wrong header containers similarly become empty instead of reporting the malformed boundary value.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L33); [crates/shamir-wasm-host/src/wasm/host_http.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L67).

Grouping/duplicate: [SUMMARY.md#5.5](SUMMARY.md#review-5-5). This is not an additional independent defect.

<a id="review-1-11"></a>

### Claim 1.11 — host-import/ABI test-coverage gap

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registered tests do not invoke local batch/global/db/http guest behaviors or cover the Params boundary matrix. Native context tests and downstream skipped integration paths are not equivalent seams.

Evidence: [crates/shamir-wasm-host/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/mod.rs#L1); [crates/shamir-wasm-host/src/tests/env_globals_tests.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/env_globals_tests.rs#L54); [crates/shamir-wasm-host/Cargo.toml:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/Cargo.toml#L46).

Grouping/duplicate: [SUMMARY.md#6.3](SUMMARY.md#review-6-3). This is not an additional independent defect.

<a id="review-1-12"></a>

### Claim 1.12 — Nits bundle (correctness-tdd finding 12, five items, distributed to their primary lenses)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Duplicated call prose and glob implementations, retained ticker, empty path fallbacks and malformed grant filtering all remain. They have separate mechanisms and should not be treated as five security exploits.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L22); [crates/shamir-wasm-host/src/net_gateway.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L487); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L158); [crates/shamir-wasm-host/src/compile.rs:542](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L542); [crates/shamir-wasm-host/src/meta.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L129).

<a id="review-2-1"></a>

### Claim 2.1 — `compile_rust_source` blocks up to 120 s and is called directly on tokio workers (pillar 2)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Source and validator DDL perform subprocess waits and JIT compilation inline. The 120-second wait does not cover probes, optimizer, reader joins or JIT compilation. Authorized source submission or permitted binary DDL can block a worker; exact starvation duration is unmeasured.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L172); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:183](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L183); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L221).

<a id="review-2-2"></a>

### Claim 2.2 — Non-atomic remove-then-insert overwrite in `replace` / `put` / `set`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Remove old; read sees absent; another writer inserts; intended insert fails silently is a concrete witness. scc 3.8.4 insert_sync does not overwrite an occupied key, so the historical A-after-B overwrite outcome is false. Exact source: [scc 3.8.4 hash_map.rs](https://docs.rs/crate/scc/3.8.4/source/src/hash_map.rs).

Evidence: [crates/shamir-wasm-host/src/registry.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L52); [crates/shamir-wasm-host/src/context.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L52); [crates/shamir-wasm-host/src/context.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L166); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-2-3"></a>

### Claim 2.3 — `FunctionRegistry::rename` rollback can silently drop the function

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

After source removal, separate registrations occupy target and source. Target insertion fails, source restoration also fails, and the original Arc is dropped. The caller receives only AlreadyExists(target). Holding two entry guards naively is not a safe remedy because bucket aliasing can deadlock.

Evidence: [crates/shamir-wasm-host/src/registry.rs:79](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L79); [crates/shamir-wasm-host/src/registry.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L84); [crates/shamir-wasm-host/src/registry.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L86).

<a id="review-2-4"></a>

### Claim 2.4 — epoch-ticker spawn failure swallowed with `.ok()`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The spawn result is converted with ok() and engine creation continues, losing independent epoch advancement without a diagnostic.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L129); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L162).

Grouping/duplicate: [SUMMARY.md#6.2](SUMMARY.md#review-6-2). This is not an additional independent defect.

<a id="review-2-5"></a>

### Claim 2.5 — cargo child not killed on the `wait_timeout` error path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

wait_timeout failure goes directly to reader joins. A still-running process or descendant holding pipes prevents those joins from completing.

Evidence: [crates/shamir-wasm-host/src/compile.rs:607](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L607).

Grouping/duplicate: [SUMMARY.md#6.8](SUMMARY.md#review-6-8). This is not an additional independent defect.

<a id="review-2-6"></a>

### Claim 2.6 — aggregate fuel budget does not bound the nested-call descent

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

The child grant is calculated before the suspended parent debits its consumed instructions; outstanding grants are invisible to admission.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:447](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L447); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:587](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L587).

Grouping/duplicate: [SUMMARY.md#1.1](SUMMARY.md#review-1-1). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — Forbidden-macro scan bypassed by whitespace/comment between macro name and `!`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

include_bytes /*c*/!(...) is a valid invocation but cleaned comments leave spaces and the adjacent-bang condition misses it. Compilation can embed a compiler-readable file and guest output can return it. Requires a working source pipeline; validator routing also bypasses the separate compiler permission.

Evidence: [crates/shamir-wasm-host/src/compile.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L168); [crates/shamir-wasm-host/src/compile.rs:365](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L365); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L169).

<a id="review-3-2"></a>

### Claim 3.2 — Contradictory fail-open vs fail-closed documentation of the `net_grants` default

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The permissive options sentence contradicts both FunctionMeta documentation and the explicit empty effective allowlist. Existing downstream tests distinguish catchable denial, not a gateway-missing trap.

Evidence: [crates/shamir-wasm-host/src/meta.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L83); [crates/shamir-wasm-host/src/meta.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L189); [crates/shamir-db/tests/functions_lifecycle.rs:907](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L907).

<a id="review-3-3"></a>

### Claim 3.3 — SSRF guard misses `0.0.0.0` (and other non-routable IPv4 ranges) — wildcard allowlist reaches loopback services

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

0.0.0.0 under a wildcard passes the common IPv4 predicate, unlike IPv6 unspecified. Effective grants must permit the destination. Actual localhost connection behavior is unverified; CGNAT, benchmark and multicast ranges require explicit policy, not automatic classification as private.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:459](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L459); [crates/shamir-wasm-host/src/net_gateway.rs:186](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L186); [crates/shamir-wasm-host/src/tests/net_gateway_tests.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/net_gateway_tests.rs#L47).

<a id="review-3-4"></a>

### Claim 3.4 — No bound on concurrent Argon2id invocations; cost parameters fully caller-supplied at this layer

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The wrapper reaches canonical crypto/argon2id with a 16-permit process-wide gate and parameter bounds. These precede the original review, so this is counter-evidence, not a later fix. The distinct semaphore lost-notification defect does not make concurrent hashing uncapped.

Evidence: [crates/shamir-wasm-host/src/builtin.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L64); [crates/shamir-wasm-host/src/scalar.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/scalar.rs#L20); [crates/shamir-funclib/src/crypto.rs:76](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L76); [crates/shamir-funclib/src/crypto.rs:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L203); [crates/shamir-funclib/src/crypto.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L221).

<a id="review-3-5"></a>

### Claim 3.5 — `wasm-opt`/toolchain probes run with the full inherited host environment

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Inheritance exists, but the documented scrub guarantee specifically concerns guest cargo build. No source-controlled access to probe environments or secret-exfiltration channel through these trusted tools is established. Uniform scrubbing is optional hardening, not a demonstrated present defect.

Evidence: [crates/shamir-wasm-host/src/compile.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L24); [crates/shamir-wasm-host/src/compile.rs:550](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L550); [crates/shamir-wasm-host/src/compile.rs:650](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L650); [crates/shamir-wasm-host/src/compile.rs:700](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L700).

<a id="review-3-6"></a>

### Claim 3.6 — compile timeout kills `cargo` but not its `rustc`/build-script grandchildren

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The launched cargo process has no group/job ownership and only child.kill is invoked. Descendants can retain pipe handles; returning at the advertised timeout is therefore not established.

Evidence: [crates/shamir-wasm-host/src/compile.rs:563](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L563); [crates/shamir-wasm-host/src/compile.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L598); [crates/shamir-wasm-host/src/compile.rs:600](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L600).

Grouping/duplicate: [SUMMARY.md#6.8](SUMMARY.md#review-6-8). This is not an additional independent defect.

<a id="review-3-7"></a>

### Claim 3.7 — `seed_env` panics on non-UTF-8 env vars

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Policy filtering follows vars() conversion, so even an excluded invalid-Unicode entry can panic during initialization.

Evidence: [crates/shamir-wasm-host/src/context.rs:211](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L211); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L168).

Grouping/duplicate: [SUMMARY.md#1.4](SUMMARY.md#review-1-4). This is not an additional independent defect.

<a id="review-3-8"></a>

### Claim 3.8 — epoch-ticker spawn failure silently disables the CPU-guest kill switch

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Failure to start the ticker is discarded. Finite fuel remains an instruction limit; this is loss of independent wall-clock interruption, not unlimited CPU instructions.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L162); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:474](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L474).

Grouping/duplicate: [SUMMARY.md#6.2](SUMMARY.md#review-6-2). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — Unbounded process-lifetime growth of `GlobalVars` via guest `global_set`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Repeated authorized invocations can insert distinct non-env keys into the shared host map. Store memory/fuel limits reset per call and do not cap retained host entries. No guest removal import or quota exists; exposure is per shared database globals instance.

Evidence: [crates/shamir-wasm-host/src/wasm/host_globals.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_globals.rs#L55); [crates/shamir-wasm-host/src/context.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L164); [crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs#L87).

<a id="review-4-2"></a>

### Claim 4.2 — Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The owned parameter map is cloned, encoded to Bytes and copied to Vec before the necessary guest-memory copy. Removing the extra copy is supported; workload multipliers and latency impact are not measured.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L407); [crates/shamir-types/src/types/value.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L33); [crates/shamir-types/src/types/value.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L52).

<a id="review-4-3"></a>

### Claim 4.3 — Unbounded stdout/stderr buffering of the guest `cargo build`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both drains retain all output and the error formatter retains stderr text. Guest source can generate large diagnostics, but cannot supply a build script through the fixed manifest. A time cap is not a byte cap.

Evidence: [crates/shamir-wasm-host/src/compile.rs:501](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L501); [crates/shamir-wasm-host/src/compile.rs:575](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L575); [crates/shamir-wasm-host/src/compile.rs:583](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L583); [crates/shamir-wasm-host/src/compile.rs:619](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L619).

<a id="review-4-4"></a>

### Claim 4.4 — Epoch-ticker thread + full engine leaked per `WasmEngine::new()` — no shutdown path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A successful spawn captures an Engine clone in an endless loop. Dropping database/function handles cannot release it. Database initialization and benchmark iterations construct separate engines; no singleton or termination mechanism exists.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L155); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L165); [crates/shamir-engine/benches/wasm_invoke.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/wasm_invoke.rs#L153).

<a id="review-4-5"></a>

### Claim 4.5 — `host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The parent Arc is cloned, then its Strings are cloned into a newly collected set and Arc. Sharing the immutable existing set preserves current inheritance semantics; adding callee-policy evaluation would be a different change.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L84); [crates/shamir-wasm-host/src/wasm/host_call.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L119); [crates/shamir-wasm-host/src/context.rs:395](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L395).

<a id="review-4-6"></a>

### Claim 4.6 — Dead `Arc` clones on every `batch_get` / `global_get` host import

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Successful batch/global reads clone two handles that only reach discard expressions. Absent/denied reads return earlier. These are structural cleanup, not measured performance regressions.

Evidence: [crates/shamir-wasm-host/src/wasm/host_batch.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L54); [crates/shamir-wasm-host/src/wasm/host_batch.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L90); [crates/shamir-wasm-host/src/wasm/host_globals.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_globals.rs#L95).

<a id="review-4-7"></a>

### Claim 4.7 — Host-import export re-resolution + `typed()` rebuild per call

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Repeated calls exist, but their asserted linear scan is false: Wasmtime 46.0.2 Instance::get_export uses string-pool and export-map lookups, and exports are lazily cached. Additional HostState caching is optional unmeasured optimization, not an established defect. Exact source: [Wasmtime instance.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/instance.rs).

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L333); [crates/shamir-wasm-host/src/wasm/host_batch.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L57); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741).

<a id="review-4-8"></a>

### Claim 4.8 — `Box<dyn Future>` heap allocation per nested async host import

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Independent archive inspection confirms Wasmtime 46.0.2 func_wrap_async requires a lifetime-indexed closure returning Box&lt;dyn Future + Send&gt;. Direct impl-Future substitution is incompatible. Exact source: [Wasmtime linker.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/linker.rs).

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L44); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L197); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741).

<a id="review-4-9"></a>

### Claim 4.9 — Redundant artifact copies in `maybe_wasm_opt`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The already-owned Vec is passed as a slice and copied when optimization is unavailable or fails. Passing ownership through avoids the copy without altering fallback behavior.

Evidence: [crates/shamir-wasm-host/src/compile.rs:638](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L638); [crates/shamir-wasm-host/src/compile.rs:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L655); [crates/shamir-wasm-host/src/compile.rs:688](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L688).

<a id="review-4-10"></a>

### Claim 4.10 — `put`/`set` pay two hash ops per write

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Separate removal and insertion perform redundant lookup/locking work and expose absence. Exact scc entry/upsert APIs can combine them; no speedup is measured.

Evidence: [crates/shamir-wasm-host/src/context.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L52); [crates/shamir-wasm-host/src/context.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L166).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — HTTP wire codec collapses duplicate headers (`Set-Cookie` loss on both directions)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Two identically named Set-Cookie response entries overwrite in host encoding. Two identically named request entries survive SDK serialization but overwrite in QueryValue decoding. Requires configured egress; this proves silent multiplicity loss, not an authentication compromise.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L89); [crates/shamir-sdk/src/http.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L99); [crates/shamir-sdk/src/value.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L58); [crates/shamir-types/src/types/value.rs:269](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L269).

<a id="review-5-2"></a>

### Claim 5.2 — `FnCtx` docs promise secret-grant gating on `global_get` that only the WASM host import enforces

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

An empty-grant native context can read a seeded env key through its direct getter despite the type/builder promise. Guest reads have a separate gate; trusted compiled-in implementations are not untrusted bytecode.

Evidence: [crates/shamir-wasm-host/src/context.rs:289](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L289); [crates/shamir-wasm-host/src/context.rs:426](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L426); [crates/shamir-wasm-host/src/wasm/host_globals.rs:79](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_globals.rs#L79).

<a id="review-5-3"></a>

### Claim 5.3 — Stringly-typed errors across the public gateway traits and egress guards

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Gateway methods and guards flatten failure classification and causes into String, contrary to the project's library-error convention. This is API/error-design debt; typed host errors alone would not make guest DB traps catchable.

Evidence: [CLAUDE.md:642](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L642); [crates/shamir-wasm-host/src/db_gateway.rs:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/db_gateway.rs#L65); [crates/shamir-wasm-host/src/net_gateway.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L60).

<a id="review-5-4"></a>

### Claim 5.4 — Inconsistent guest-facing error contract across sibling host imports (envelope vs uncatchable trap)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

HTTP runtime envelopes and DB/call traps are explicit current contracts. No guarantee requires sibling imports to have identical recoverability. Converting db_execute into a catchable API is coordinated protocol design, not repair of demonstrated nonconformance.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L99); [crates/shamir-wasm-host/src/wasm/host_db.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_db.rs#L187); [crates/shamir-sdk/src/db.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L142).

<a id="review-5-5"></a>

### Claim 5.5 — `decode_http_request` silently coerces malformed `headers`/`body` to empty while `method`/`url` default to `""`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Wrong body/header container types silently become empty, while wrong required-string types fail and missing strings become empty. A guest can send a materially different accepted request than the documented shape describes.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L26); [crates/shamir-wasm-host/src/wasm/host_http.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L64); [crates/shamir-wasm-host/src/wasm/host_http.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L67).

<a id="review-5-6"></a>

### Claim 5.6 — `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Source compilation canonicalizes the build-time manifest-relative SDK path without override. A deployed binary lacking that tree cannot use this optional facility. The guide already recommends uploaded WASM in production, so binary-only execution is unaffected.

Evidence: [crates/shamir-wasm-host/src/compile.rs:485](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L485); [docs/guide-docs/guide/05-functions.md:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/05-functions.md#L136); [docs/guide-docs/guide/05-functions.md:369](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/05-functions.md#L369).

<a id="review-5-7"></a>

### Claim 5.7 — Catalogue record decoding has silent fallbacks and no format versioning

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Present invalid enums and non-string grants disappear without diagnostics, indistinguishable from missing-field compatibility defaults. Dropped grants narrow sets; Invoker fallback is not universally privilege-reducing. Catalogue rows do have an artifact version field, but no checked metadata-schema dispatch.

Evidence: [crates/shamir-wasm-host/src/meta.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L113); [crates/shamir-wasm-host/src/meta.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L129); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L231).

<a id="review-5-8"></a>

### Claim 5.8 — `ResolvedPin::pinned_ips` uses an empty-Vec sentinel for "do not pin"

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Exact-match no-resolution behavior is explicitly documented and tested. The current curl helper emits no resolve lines for that vector. Option would improve representation only; it does not add destination validation.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L125); [crates/shamir-wasm-host/src/tests/net_gateway_tests.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/net_gateway_tests.rs#L169); [crates/shamir-db/src/shamir_db/curl_gateway.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L195).

<a id="review-5-9"></a>

### Claim 5.9 — Internal audit-tracking references leaked into public API docs

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Public guard and pin documentation still references finding 2c without a consumer-facing reference. The substantive pinning explanation can remain.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L109); [crates/shamir-wasm-host/src/net_gateway.rs:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L148).

<a id="review-5-10"></a>

### Claim 5.10 — `glob_matches` duplicated in two security-relevant matchers

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Environment and egress matchers remain separately maintained copies. They currently share the same repeated-suffix false-negative defect; deduplication alone would not fix that behavior.

Evidence: [crates/shamir-wasm-host/src/env_policy.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L75); [crates/shamir-wasm-host/src/net_gateway.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L487).

Grouping/duplicate: [SUMMARY.md#7.2](SUMMARY.md#review-7-2). This is not an additional independent defect.

<a id="review-5-11"></a>

### Claim 5.11 — wire codecs, `db_*` imports, and the depth limit have no in-crate tests

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No registered local test asserts HTTP codec shapes, DB import behavior or the specific depth-limit result. Actor probes exercise successful call imports; generic recursive expect_err is not a depth oracle.

Evidence: [crates/shamir-wasm-host/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/mod.rs#L1); [crates/shamir-wasm-host/src/tests/nested_actor_tests.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/nested_actor_tests.rs#L125); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:313](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L313).

Grouping/duplicate: [SUMMARY.md#6.3](SUMMARY.md#review-6-3). This is not an additional independent defect.

<a id="review-5-12"></a>

### Claim 5.12 — duplicated doc-comment block on `host_call`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The same identifying paragraph appears twice consecutively.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L16); [crates/shamir-wasm-host/src/wasm/host_call.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L22).

Grouping/duplicate: [SUMMARY.md#7.3](SUMMARY.md#review-7-3). This is not an additional independent defect.

<a id="review-5-13"></a>

### Claim 5.13 — unused `serde` dependency

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

No source use consumes the direct dependency. Transitive/workspace serde users prevent inferring a whole extra proc-macro build from this declaration.

Evidence: [crates/shamir-wasm-host/Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/Cargo.toml#L14); [crates/shamir-wasm-host/src/lib.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/lib.rs#L20).

Grouping/duplicate: [SUMMARY.md#7.5](SUMMARY.md#review-7-5). This is not an additional independent defect.

<a id="review-5-14"></a>

### Claim 5.14 — `CreateFunctionOptions` doc states the opposite of the actual empty-`net_grants` semantics

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The options default's permissive documentation still disagrees with explicit empty-grant denial.

Evidence: [crates/shamir-wasm-host/src/meta.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L189); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:841](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L841).

Grouping/duplicate: [SUMMARY.md#3.2](SUMMARY.md#review-3-2). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — Aggregate fuel-budget debit is skipped when the call future is cancelled (cancelled task permanently leaks budget)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Dropping a child call during an await bypasses its final fetch_sub. A top-level timeout can do this while the parent still charges its own Store. The error leaves too much remaining budget, not permanently reserved capacity; an ordinary fresh top-level context does not preserve that counter.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:550](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L550); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:581](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L581); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:587](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L587).

<a id="review-6-2"></a>

### Claim 6.2 — Epoch-ticker thread spawn failure is swallowed with `.ok()` — wall-clock pre-emption silently disabled, nothing logged

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Thread creation failure is neither propagated nor logged. The resulting Engine can run with no epoch increments, while a non-yielding guest prevents Tokio timeout polling. Finite fuel remains effective.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L129); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L162).

<a id="review-6-3"></a>

### Claim 6.3 — No in-crate test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, OOB result pointers, missing exports)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The manifest wires ten topics, but no local WAT invokes batch/global/db/http behaviors or deliberately asserts missing exports, bad output ranges, malformed HTTP, Params boundaries or depth rejection. Actor tests do invoke call; all-import sanitizer fixtures only declare imports. Downstream integration coverage is separate and skippable.

Evidence: [crates/shamir-wasm-host/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/mod.rs#L1); [crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs#L93); [crates/shamir-wasm-host/src/tests/nested_actor_tests.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/nested_actor_tests.rs#L85); [crates/shamir-db/tests/functions_lifecycle.rs:1116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L1116).

<a id="review-6-4"></a>

### Claim 6.4 — public gateway traits return `Result<_, String>`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

All inspected DbGateway methods retain String errors and lose typed causes at the host boundary.

Evidence: [crates/shamir-wasm-host/src/db_gateway.rs:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/db_gateway.rs#L65); [crates/shamir-wasm-host/src/db_gateway.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/db_gateway.rs#L86).

Grouping/duplicate: [SUMMARY.md#5.3](SUMMARY.md#review-5-3). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Panic inside the Argon2id blocking task is collapsed into `FunctionError::Cancelled`, discarding the panic payload

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Any JoinError maps to Cancelled without inspecting is_panic. The enum comment includes panic, but display and returned cause are misleading. Panic hooks can still report the original panic independently.

Evidence: [crates/shamir-wasm-host/src/builtin.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L83); [crates/shamir-wasm-host/src/error.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/error.rs#L36).

<a id="review-6-6"></a>

### Claim 6.6 — `map_wasm_error` classifies traps by substring-matching the error message

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Classification examines Display text instead of the root Trap. Wasmtime 46.0.2 exposes Trap::OutOfFuel and Trap::Interrupt via downcast_ref; Interrupt displays 'interrupt', not 'epoch'. Context/backtrace names can also contaminate substring matches. The fuel test checks only Compute.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:212](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L212); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741); [Cargo.lock:4781](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4781).

<a id="review-6-7"></a>

### Claim 6.7 — `compile.rs` degrades invalid-UTF-8 temp paths into `unwrap_or("")` arguments for cargo instead of a typed error

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both path arguments silently substitute empty strings after failed Unicode conversion. Native OsStr arguments can preserve valid OS paths; rejecting them is not required.

Evidence: [crates/shamir-wasm-host/src/compile.rs:542](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L542); [crates/shamir-wasm-host/src/compile.rs:544](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L544).

<a id="review-6-8"></a>

### Claim 6.8 — Compile child-process teardown is incomplete on every abnormal-exit path (no Job Object; kill-failure waits unbounded; error path never kills)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Timeout ignores kill failure then waits and joins without bounds; wait failure joins without kill/reap; only cargo is targeted. A running descendant retaining a pipe can block reader joins even after cargo exits. No deterministic teardown oracle observes descendants or EOF.

Evidence: [crates/shamir-wasm-host/src/compile.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L598); [crates/shamir-wasm-host/src/compile.rs:600](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L600); [crates/shamir-wasm-host/src/compile.rs:607](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L607); [crates/shamir-wasm-host/src/tests/compile_tests.rs:263](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L263).

<a id="review-6-9"></a>

### Claim 6.9 — `rename`'s error-path rollback insert can itself fail silently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Concurrent occupation of both names after removal makes target insert and restoration fail; the restoration error is discarded.

Evidence: [crates/shamir-wasm-host/src/registry.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L83); [crates/shamir-wasm-host/src/registry.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L86).

Grouping/duplicate: [SUMMARY.md#2.3](SUMMARY.md#review-2-3). This is not an additional independent defect.

<a id="review-6-10"></a>

### Claim 6.10 — one leaked OS thread per `WasmEngine`; tests instantiate engines freely

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each successful ticker owns a retained Engine clone and has no termination condition. Repeated construction is allowed.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L155); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L155).

Grouping/duplicate: [SUMMARY.md#4.4](SUMMARY.md#review-4-4). This is not an additional independent defect.

<a id="review-6-11"></a>

### Claim 6.11 — Forbidden-macro scanner fails open (`unwrap_or_default`) on an invariant break

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The fallback is present, but the current transformations preserve complete UTF-8 spans for &str input. No input reaches conversion failure. Replacing it with an invariant assertion is optional maintenance hardening, not a present scanner bypass.

Evidence: [crates/shamir-wasm-host/src/compile.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L151); [crates/shamir-wasm-host/src/compile.rs:252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L252).

<a id="review-6-12"></a>

### Claim 6.12 — `FunctionMeta::from_record` silently coerces malformed catalogue fields

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Malformed present fields and filtered grant entries remain indistinguishable from missing defaults, without a diagnostic. Grant filtering itself does not widen the grant set.

Evidence: [crates/shamir-wasm-host/src/meta.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L113); [crates/shamir-wasm-host/src/meta.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L129).

Grouping/duplicate: [SUMMARY.md#5.7](SUMMARY.md#review-5-7). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Mid-body `use` statements violate the "Imports at the top" rule (7 sites)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Two pipe-reader imports and five scanner-test imports remain inside bodies. None has the required collision/cfg justification. This violates the explicit convention, not runtime behavior.

Evidence: [CLAUDE.md:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L617); [crates/shamir-wasm-host/src/compile.rs:574](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L574); [crates/shamir-wasm-host/src/compile.rs:582](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L582); [crates/shamir-wasm-host/src/tests/compile_tests.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L111); [crates/shamir-wasm-host/src/tests/compile_tests.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L169).

<a id="review-7-2"></a>

### Claim 7.2 — `glob_matches` duplicated in `net_gateway.rs` under a doc comment falsely claiming reuse

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The egress helper is a separate copy despite the reuse comment. Both have the same current suffix-matching defect; no difference between their present behavior is claimed.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:483](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L483); [crates/shamir-wasm-host/src/env_policy.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L75).

<a id="review-7-3"></a>

### Claim 7.3 — Verbatim-duplicated doc-comment block on `host_call`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The six-line summary repeats before the borrow-dance section.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L16); [crates/shamir-wasm-host/src/wasm/host_call.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L22).

<a id="review-7-4"></a>

### Claim 7.4 — `net_gateway.rs` carries two primary concerns (one-file-one-export, borderline)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The rule permits closely coupled groups, and gateway behavior explicitly requires these guards. A split is optional organization and has no demonstrated behavioral benefit.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-wasm-host/src/net_gateway.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L58).

<a id="review-7-5"></a>

### Claim 7.5 — Unused direct dependency `serde` in Cargo.toml

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The direct declaration has no source consumer. Removal is scoped manifest cleanup; extra workspace compilation cost is not established.

Evidence: [crates/shamir-wasm-host/Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/Cargo.toml#L14).

<a id="review-7-6"></a>

### Claim 7.6 — security-bearing host imports tested only downstream

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Local env tests access native GlobalVars. The downstream granted/ungranted read and write-preservation tests exercise the real guest seam but can return on missing toolchains and are excluded by lib-only selection.

Evidence: [crates/shamir-wasm-host/src/tests/env_globals_tests.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/env_globals_tests.rs#L54); [crates/shamir-db/tests/functions_lifecycle.rs:1116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L1116); [crates/shamir-db/tests/functions_lifecycle.rs:1228](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L1228); [scripts/test.sh:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L180).

Grouping/duplicate: [SUMMARY.md#6.3](SUMMARY.md#review-6-3). This is not an additional independent defect.

<a id="review-7-7"></a>

### Claim 7.7 — `wasm/mod.rs` doc list omits the sanitizer re-exports

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The declared public-surface list omits verify_wasm_module and SANCTIONED_HOST_IMPORTS, both reexported below.

Evidence: [crates/shamir-wasm-host/src/wasm/mod.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/mod.rs#L3); [crates/shamir-wasm-host/src/wasm/mod.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/mod.rs#L18).

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 30 | 28 | 0 | 0 | 0 | 0 | 2 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No strict aggregate/cancellation accounting exists. Reserving all parent fuel starves nested calls. Account consumed parent fuel at suspension and transfer/reconcile remaining child capacity with cancellation-safe ownership; a guard must charge before its Store is destroyed.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:447](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L447); [crates/shamir-wasm-host/src/wasm/host_call.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L120); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:587](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L587).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Seed overflow and weak tests remain. Silent clamping changes the public u64 limit; use a compatible unsigned accounting design or explicitly validate/document a smaller domain. An aggregate oracle needs a finite workload, and deadline assertions should use typed causes rather than the currently broken 'epoch' text mapping.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:436](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L436); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:246](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L246); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:313](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L313).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Adjacent-bang scanner remains. Delimited whitespace/comment fixtures are needed; historical undelimited examples are invalid Rust. Correct recognition does not substitute for fixing the uncovered validator compiler-authority route.

Evidence: [crates/shamir-wasm-host/src/compile.rs:365](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L365); [crates/shamir-wasm-host/src/tests/compile_tests.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L68).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Both DDL routes still compile inline. Offloading source and JIT work is appropriate, but spawn_blocking cancellation does not stop a running compiler closure; retain process ownership, admission bounds and explicit completion cleanup.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L172); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:232](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L232).

<a id="plan-p0-5"></a>

### Plan P0.5 — P0.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Header shapes remain unchanged. Update host response encoding, SDK request encoding and SDK response decoding together. Preserve legacy Map decoding and arrange compatibility with stored guests whose old decoder would silently discard List headers.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L94); [crates/shamir-sdk/src/http.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L108); [crates/shamir-sdk/src/http.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L139).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No process-tree ownership or bounded teardown exists. Tree termination must cover inherited pipe handles; a bounded direct-child wait followed by unbounded joins is insufficient. Do not return while uncontrolled writers still own temporary paths.

Evidence: [crates/shamir-wasm-host/src/compile.rs:563](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L563); [crates/shamir-wasm-host/src/compile.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L598); [crates/shamir-wasm-host/src/compile.rs:607](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L607).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ticker errors remain discarded. Logging alone is observability, not completion of the preemption guarantee; propagate construction failure or provide a proven equivalent interrupt driver.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L162).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Two matcher copies remain. Sharing a helper is safe only after validating its actual glob contract; the repeated-suffix false negative must not simply be centralized.

Evidence: [crates/shamir-wasm-host/src/env_policy.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L90); [crates/shamir-wasm-host/src/net_gateway.rs:499](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L499).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Options prose remains stale. Existing downstream empty-grants denial coverage should be retained; no gateway-missing trap is expected. Add a toolchain-independent local behavior oracle if local coverage is required.

Evidence: [crates/shamir-wasm-host/src/meta.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L189); [crates/shamir-db/tests/functions_lifecycle.rs:907](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L907).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Silent malformed request defaults remain. Reject malformed required fields and containers, while deciding explicitly whether absent optional headers/body stay supported. Str-body acceptance would be a documented extension, not an incidental coercion.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L33); [crates/shamir-wasm-host/src/wasm/host_http.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L64); [crates/shamir-wasm-host/src/wasm/host_http.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L69).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Overwrites and rename are unchanged. scc 3.8.4 entry/upsert supports atomic single-key replacement, but holding target and source entry guards can self-deadlock on a shared bucket. Rename needs a proved multi-key/generation protocol; logging loss or inventing a new name does not preserve re-key semantics.

Evidence: [crates/shamir-wasm-host/src/registry.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L52); [crates/shamir-wasm-host/src/registry.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L86); [crates/shamir-wasm-host/src/context.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L94); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="plan-p1-12"></a>

### Plan P1.12 — P1.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

vars() is unchanged. Prefer vars_os with fallible conversion and skip/report invalid entries. Lossy conversion of variable names can alias distinct names and change secret-policy matching, so it is not automatically safe.

Evidence: [crates/shamir-wasm-host/src/context.rs:211](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L211); [crates/shamir-wasm-host/src/env_policy.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L38).

<a id="plan-p1-13"></a>

### Plan P1.13 — P1.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Native getter docs remain inaccurate. Correcting their scope preserves the trusted-native API. Adding gating must deliberately cover the other direct native accessors as well, and cannot be sold as an untrusted-guest fix.

Evidence: [crates/shamir-wasm-host/src/context.rs:392](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L392); [crates/shamir-wasm-host/src/context.rs:426](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L426); [crates/shamir-wasm-host/src/context.rs:448](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L448).

<a id="plan-p1-14"></a>

### Plan P1.14 — P1.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

String contracts remain. Typed gateway errors require coordinated trait implementations and source preservation. Wire rendering and intentionally opaque guest error contracts should not change accidentally.

Evidence: [crates/shamir-wasm-host/src/db_gateway.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/db_gateway.rs#L86); [crates/shamir-wasm-host/src/net_gateway.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L60); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L292).

<a id="plan-p1-15"></a>

### Plan P1.15 — P1.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The named local behavior matrices are absent. Use real WAT calls and assert resulting values/state and exact failure mechanisms. Include mixed async-import getter coverage; import declarations and ActorProbe alone cannot catch that defect.

Evidence: [crates/shamir-wasm-host/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/mod.rs#L1); [crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs#L93); [crates/shamir-wasm-host/src/wasm/host_batch.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L67).

<a id="plan-p1-16"></a>

### Plan P1.16 — P1.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No quota exists. Removal/namespacing alone does not bound adversarial retention. Cardinality and byte admission must be atomic across concurrent insert/replace paths, account released values, and preserve OS-seeded env entries.

Evidence: [crates/shamir-wasm-host/src/context.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/context.rs#L164); [crates/shamir-wasm-host/src/wasm/host_globals.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_globals.rs#L55).

<a id="plan-p1-17"></a>

### Plan P1.17 — P1.17

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

IPv4 omissions remain. Define unspecified/internal/special-range policy explicitly and preserve intentional exact-host exceptions. Use guard-result assertions and canonical aliases, without claiming unexecuted platform connection behavior.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:459](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L459); [crates/shamir-wasm-host/src/tests/net_gateway_tests.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/net_gateway_tests.rs#L47).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No SDK deployment override exists. A host-controlled path option or installed SDK asset can address source compilation; it must not let untrusted guest input choose arbitrary dependency trees. Precompiled WASM remains the documented production alternative.

Evidence: [crates/shamir-wasm-host/src/compile.rs:485](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L485); [docs/guide-docs/guide/05-functions.md:369](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/05-functions.md#L369).

<a id="plan-p2-19"></a>

### Plan P2.19 — P2.19

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

Catchable DB envelopes are an optional new protocol, not a correction to the explicit trap contract. BatchResponse has no error field, and stored old guests must not silently reinterpret a new envelope. A deliberate host/SDK ABI versioning decision is required.

Evidence: [crates/shamir-wasm-host/src/wasm/host_db.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_db.rs#L187); [crates/shamir-sdk/src/db.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L142); [crates/shamir-query-types/src/batch/batch_response.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_response.rs#L30).

<a id="plan-p2-20"></a>

### Plan P2.20 — P2.20

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Collectors remain uncapped. Bound retained diagnostics while continuing to drain discarded bytes, or the fix introduces pipe backpressure deadlock. Cap formatting and optimizer/probe collection too where they share the intended budget.

Evidence: [crates/shamir-wasm-host/src/compile.rs:575](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L575); [crates/shamir-wasm-host/src/compile.rs:583](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L583); [crates/shamir-wasm-host/src/compile.rs:669](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L669).

<a id="plan-p2-21"></a>

### Plan P2.21 — P2.21

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Public custom limits and the pool ceiling remain unreconciled. Validate/document a shared ceiling or configure the engine appropriately; raising pool reservations has address-space/admission costs and is not a free per-function fix.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:190](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L190); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:458](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L458).

<a id="plan-p2-22"></a>

### Plan P2.22 — P2.22

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Escaped-quote stripping is still incorrect. A lexical regression should pin the closing quote and following real macro. The UTF-8 fallback subtask is optional because no current input reaches it; a blanket panic change is not a security fix.

Evidence: [crates/shamir-wasm-host/src/compile.rs:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L334); [crates/shamir-wasm-host/src/compile.rs:252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L252).

<a id="plan-p2-23"></a>

### Plan P2.23 — P2.23

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Extra map/buffer copies, grants recollection, dead Arcs and artifact copies remain. Direct unboxed futures are incompatible with Wasmtime 46.0.2. Export caching is optional and must keep handles scoped to the owning Store/instance, including start-time calls.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L407); [crates/shamir-wasm-host/src/wasm/host_call.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L119); [crates/shamir-wasm-host/src/wasm/host_batch.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L54); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741).

<a id="plan-p2-24"></a>

### Plan P2.24 — P2.24

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ticker retention remains. A shutdown design must trigger on the last external owner, not every cloned WasmEngine drop. A shared singleton also shares the 128-slot pool/admission capacity across databases, so that semantic change needs review.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L27); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L155); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L178).

<a id="plan-p2-25"></a>

### Plan P2.25 — P2.25

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Malformed metadata is still silently coerced. Distinguish intentionally missing legacy fields from invalid present data. Any schema marker needs actual supported-version dispatch; the existing artifact version is not that dispatch. Avoid logging secret contents.

Evidence: [crates/shamir-wasm-host/src/meta.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L113); [crates/shamir-wasm-host/src/meta.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/meta.rs#L151); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L231).

<a id="plan-p2-26"></a>

### Plan P2.26 — P2.26

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Current sentinel semantics are documented and correctly consumed. Option conversion is an API preference and does not repair a demonstrated bug.

Evidence: [crates/shamir-wasm-host/src/net_gateway.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L125); [crates/shamir-db/src/shamir_db/curl_gateway.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L195).

<a id="plan-p2-27"></a>

### Plan P2.27 — P2.27

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Substring mapping remains. Exact Wasmtime 46.0.2 source supports downcast_ref::&lt;Trap&gt;() with OutOfFuel and Interrupt; there is no need for the proposed unverified TrapCode spelling. Preserve host-origin errors and do not retain substring fallback as authority for typed causes.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:212](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_tests.rs#L212); [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741); [Cargo.lock:4781](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4781).

<a id="plan-p2-28"></a>

### Plan P2.28 — P2.28

Status: `confirmed-open`.

Prior-cycle decision: `partially-fixed`.

The cap subtask was based on a refuted premise: bounds already existed before the review, not as completion of this plan. Panic classification remains unchanged. Validate and fix the existing gate's separate lost-notification mechanism instead of adding a redundant wrapper semaphore.

Evidence: [crates/shamir-funclib/src/crypto.rs:76](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L76); [crates/shamir-funclib/src/crypto.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L140); [crates/shamir-funclib/src/crypto.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L145); [crates/shamir-wasm-host/src/builtin.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L83).

<a id="plan-p2-29"></a>

### Plan P2.29 — P2.29

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Path fallback and lost JoinError cause remain. Preserve native OS paths with Command arguments; historical to_str().map_err is invalid because to_str returns Option. Panic payload rendering must not disclose sensitive input or invoke uncontrolled Debug implementations.

Evidence: [crates/shamir-wasm-host/src/compile.rs:542](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L542); [crates/shamir-wasm-host/src/builtin.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L83).

<a id="plan-p2-30"></a>

### Plan P2.30 — P2.30

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Mandatory import placement and inaccurate prose/manifest items remain. File splitting and uniform trusted-probe environment scrubbing are optional, not necessary security fixes. Removing direct serde must preserve any intentionally unified dependency features.

Evidence: [crates/shamir-wasm-host/src/compile.rs:574](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L574); [crates/shamir-wasm-host/Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/Cargo.toml#L14); [crates/shamir-wasm-host/src/wasm/mod.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/mod.rs#L3); [crates/shamir-wasm-host/src/wasm/host_call.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L22).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 5 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Validator source creation bypasses the separate WasmCompiler permission gate

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

The wire CreateValidator handler checks FunctionNamespace Create, then create_validator_from_source_as reaches create_validator_inner, whose Source branch directly calls compile_rust_source without WasmCompiler Execute authorization. A principal permitted namespace creation but denied Execute by a hardened compiler mode still reaches host compilation. This is distinct from the macro scanner bypass and remains even with a corrected scanner. The permission tests exercise function creation only; a discriminating validator test must prove denial before toolchain/scanner execution. Parent acceptance: direct top-level wire admin operations are superuser-gated. The ordinary-user wire witness is a permitted nontransactional ForEach body, whose outer operation is not is_admin, followed by the real per-operation namespace checks; this is the already-open facade compiler-gate defect, not an extra independent vulnerability.

Evidence: [crates/shamir-wasm-host/src/compile.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L32); [crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs#L81); [crates/shamir-db/src/shamir_db/execute/admin_validator.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_validator.rs#L36); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L199); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:220](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L220); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L169); [crates/shamir-db/src/shamir_db/tests/wasm_compiler_permission_tests.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/wasm_compiler_permission_tests.rs#L98).

Grouping/duplicate: [../shamir-db/security-crypto.md#3](../shamir-db/security-crypto.md#review-3). This is not an additional independent defect.

<a id="observation-new-2"></a>

### Observation NEW.2 — Present batch/global reads fail in modules importing any async host function

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

batch_get and global_get use TypedFunc::call to reenter shamir_alloc. Wasmtime 46.0.2 InstancePre records async imported definitions and pre_instantiate_raw marks the Store async-required; synchronous call then rejects it. A sanctioned module importing call plus global_get, even without invoking call, cannot read a present granted global. Absent/denied reads return before allocation and conceal the defect. Exact checksummed published sources: [instance.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/instance.rs), [store.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/store.rs), [fiber.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/fiber.rs), and [typed.rs](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/func/typed.rs). The all-import fixture never calls these getters; the oracle needs a real present-value read with an async import declared.

Evidence: [Cargo.lock:4741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4741); [crates/shamir-wasm-host/src/wasm/host_batch.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_batch.rs#L67); [crates/shamir-wasm-host/src/wasm/host_globals.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_globals.rs#L108); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L197); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:496](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L496); [crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/wasm_sanitizer_tests.rs#L93).

Grouping/duplicate: [../shamir-sdk/SUMMARY.md#NEW.3](../shamir-sdk/SUMMARY.md#observation-new-3). This is not an additional independent defect.

<a id="observation-new-3"></a>

### Observation NEW.3 — Reserved colon-prefixed map keys violate guest-to-host value identity

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

SDK Value::Map serializes literal keys, but host QueryValue is the same Value&lt;String&gt; decoder used for prefixed fixtures. A guest map containing i:count with Int(7) becomes count with Int(7); including both names can overwrite a field, while a noninteger i:count can reject an otherwise supported SDK Map. The defect reaches returned values, nested-call parameters, globals and DB documents. rmp-serde 1.3.1 dispatches to that custom map visitor; its depth guard does not repair key semantics. Exact archive source: [rmp-serde 1.3.1 decode.rs](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs). Existing SDK conformance fixtures use ordinary keys and cannot catch this; test literal reserved-prefix keys against the unchanged-key contract.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-sdk/src/value.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L1); [crates/shamir-sdk/src/value.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L58); [crates/shamir-types/src/types/value.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L29); [crates/shamir-types/src/types/value.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L217); [crates/shamir-types/src/types/value.rs:268](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L268); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:577](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L577); [crates/shamir-wasm-host/src/wasm/host_call.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L91); [crates/shamir-sdk/src/tests/value_tests.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/value_tests.rs#L179).

Grouping/duplicate: [../shamir-types/SUMMARY.md#NEW.5](../shamir-types/SUMMARY.md#observation-new-5). This is not an additional independent defect.

<a id="observation-new-4"></a>

### Observation NEW.4 — The delegated Argon2 concurrency semaphore can lose its final wakeup

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

At saturation, a waiter holds notify's mutex, observes available=0 at the while predicate, and is descheduled before Condvar::wait. Hash holders can release all permits and notify without acquiring that mutex. Notifications occur before the waiter registers and are lost; it can then sleep with permits available and no remaining releaser. This is a reachable blocking-pool liveness defect, not failure of the 16-hash cap. [Pinned Rust 1.94.0 Condvar source](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sync/poison/condvar.rs) confirms notifications are not buffered. The existing barrier/KDF-duration test cannot force this interval; a deterministic check-to-wait schedule or model is the discriminating oracle. Commit 190e1f08's no-lost-wakeup assurance is contradicted by this witness.

Evidence: [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15); [crates/shamir-wasm-host/src/builtin.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L63); [crates/shamir-funclib/src/crypto.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L140); [crates/shamir-funclib/src/crypto.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L145); [crates/shamir-funclib/src/crypto.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:204](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L204).

Grouping/duplicate: [../shamir-funclib/concurrency-lockfree.md#2](../shamir-funclib/concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="observation-new-5"></a>

### Observation NEW.5 — Both glob matchers reject valid repeated-suffix matches

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

The documented star matches zero or more characters, but the algorithm chooses the first occurrence of the final literal and then requires that position to end the text. Pattern *_KEY with text X_KEY_KEY therefore returns false although the suffix matches; *.example.com similarly rejects a.example.com.example.com. Both environment seeding and egress policy use this algorithm. This is a present false-negative contract defect, separate from copied-code drift, and does not widen authorization. Existing fixtures lack repeated final literals; an explicit suffix witness must remain after deduplication.

Evidence: [crates/shamir-wasm-host/src/env_policy.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L31); [crates/shamir-wasm-host/src/env_policy.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L90); [crates/shamir-wasm-host/src/env_policy.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/env_policy.rs#L102); [crates/shamir-wasm-host/src/net_gateway.rs:499](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L499); [crates/shamir-wasm-host/src/net_gateway.rs:510](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/net_gateway.rs#L510); [crates/shamir-wasm-host/src/tests/env_policy_tests.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/env_policy_tests.rs#L20).

## Evidence and recipe corrections

- Current 4.7 is not a proven performance defect: the checksummed Wasmtime 46.0.2 implementation uses mapped export lookups and lazy export caching, refuting the historical O(exports) scan. Source: https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/instance.rs; lock evidence Cargo.lock:4741.
- Current 5.4 and P2.19 describe an optional ABI redesign. Deliberately documented DB traps are not a defect merely because HTTP offers recovery.
- Current 3.5 and 6.11 are optional hardening/preferences with no demonstrated current failure path, rather than open security mechanisms.
- The compiler-permission assurance does not cover validator source creation. Hardened WasmCompiler permissions can be bypassed by that reachable route.
- The .cwasm suggestion in wasm_function.rs is unsafe and inaccurate. Module::from_binary does not deserialize compiled native artifacts; an ELF/header check does not authenticate provenance or satisfy unsafe deserialize requirements.
- The pooling singleton recipe must account for shared slot capacity and last-owner shutdown. Ordinary WasmEngine Clone plus a Drop signal can prematurely stop another clone's ticker.
- Current code and source-derived conclusions are unchanged from the prior source snapshot in the inspected production files; report-only history does not establish any later source fix.
- The summary has 69 claim rows, not 69 unique defects. Repeated lens roots, bundled nits, optional proposals and historical counts must not be added into a fresh bug census.

## Module scope and limitations

Coverage: 8 assigned documents, 138 current claim rows, 30 plan rows, 0 pre-existing observation rows; 5 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md); [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md).

- All eight assigned documents were read, including current ledgers, parent refinements, corrections, historical bodies and recipes. There are no current NEW.N Observation rows or assigned TASK_GROUPS.md.
- Strictly read-only: no files written, builds, tests, reproductions, benchmarks, dependency commands, process termination, git mutations or child agents.
- HEAD matched the required commit and the checkout was clean at both inspections.
- Inspected locked dependency source from existing registry sources and cached archives. Wasmtime 46.0.2, scc 3.8.4 and rmp-serde 1.3.1 archive SHA-256 values matched Cargo.lock. No archives were extracted to disk.
- Toolchain 1.94.0 was treated as metadata. Relevant standard-library behavior was checked against upstream Rust 1.94.0 source; the toolchain was not executed.
- OS-specific unspecified-address connections, compiler-descendant survival duration, exact guest fuel costs, timing, RSS and allocation-performance benefits were not measured.
- Coverage is complete for the assigned claim checklist, not an exhaustive security or Rust Intel audit.

## Guarantee checks

- **Shared registries and contexts are lock-free.** — `diverges`. Explicit banned locks are absent from this crate, but scc 3.8.4 serializes entry access with bucket read-write locks and warns against synchronous blocking methods in async code. Lock-free resizing is not lock-free entry access. Exact source: [scc 3.8.4](https://docs.rs/crate/scc/3.8.4/source/src/hash_map.rs). Reference: crates/shamir-wasm-host/src/registry.rs:1; crates/shamir-wasm-host/src/context.rs:35; Cargo.lock:3123; scc 3.8.4 published src/hash_map.rs.
- **One shared fuel budget bounds all nested Stores and is charged on every exit.** — `diverges`. Outstanding ancestor grants are not reserved or synchronized with child consumption. Dropping an awaited child skips its final debit. Fresh ordinary top-level contexts do not retain the locally created counter. Reference: crates/shamir-wasm-host/src/wasm/wasm_function.rs:58; crates/shamir-wasm-host/src/wasm/wasm_function.rs:438; crates/shamir-wasm-host/src/wasm/wasm_function.rs:587.
- **The wall-clock deadline bounds the entire logical request.** — `diverges`. The Tokio timeout encloses shamir_call only. Instantiation/start, initial shamir_alloc, preparation and decoding are outside it. Epoch traps interrupt guest execution, not an indefinitely pending host future during those excluded phases. Reference: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:219; crates/shamir-wasm-host/src/wasm/wasm_function.rs:496; crates/shamir-wasm-host/src/wasm/wasm_function.rs:525; crates/shamir-wasm-host/src/wasm/wasm_function.rs:550.
- **CPU-bound operations leave async workers free.** — `diverges`. The named Argon2 wrapper offloads work, but source compilation and Wasmtime compilation execute synchronously on async DDL paths. Guest execution also remains on the runtime worker until yielding or trapping. Reference: crates/shamir-wasm-host/src/contract.rs:18; crates/shamir-wasm-host/src/builtin.rs:63; crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172.
- **Every source-compilation route requires WasmCompiler Execute permission.** — `diverges`. Function creation checks the compiler permission. Validator source creation reaches the same compiler without that check, including through the wire admin handler. Reference: crates/shamir-wasm-host/src/compile.rs:32; crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:169; crates/shamir-db/src/shamir_db/execute/admin_validator.rs:36; crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:220.
- **Forbidden file/environment macros are rejected before compilation.** — `diverges`. Valid delimited invocations with whitespace or comments before ! evade the adjacent-byte match. The grammar is token-based: [Rust Reference macro invocation](https://doc.rust-lang.org/reference/macros.html#macro-invocation). Reference: crates/shamir-wasm-host/src/compile.rs:18; crates/shamir-wasm-host/src/compile.rs:365.
- **The structural sanitizer permits only function imports with the sanctioned ABI.** — `diverges`. The sanitizer itself checks names and component encoding, not matching import kinds or signatures. Wasmtime's subsequent instantiate_pre performs type validation. The complete loading chain rejects mismatched imports; the sanitizer's individual promise is broader than its implementation. Reference: crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs:100; crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs:160; crates/shamir-wasm-host/src/wasm/wasm_function.rs:177.
- **Denied guest environment reads look absent and guests cannot overwrite env.*.** — `supported`. Writes trap before mutation; denied and absent reads return 0. This supports return-value semantics, not timing indistinguishability or successful granted reads in every async-import configuration. Reference: crates/shamir-wasm-host/src/wasm/host_globals.rs:46; crates/shamir-wasm-host/src/wasm/host_globals.rs:79.
- **FnCtx::global_get enforces secret_grants.** — `diverges`. The trusted native getter directly reads GlobalVars. Enforcement is confined to the guest import. Reference: crates/shamir-wasm-host/src/context.rs:289; crates/shamir-wasm-host/src/context.rs:392; crates/shamir-wasm-host/src/context.rs:426.
- **Empty user-function net_grants deny egress.** — `supported`. Explicit empty metadata produces an empty effective allowlist and a configured gateway returns a catchable denial. Metadata absence is a separate builtin fallback. Nonempty grant intersection is literal-pattern equality, not general pattern subsumption. Reference: crates/shamir-db/src/shamir_db/shamir_db/core.rs:834; crates/shamir-db/tests/functions_lifecycle.rs:907.
- **HTTP header multiplicity survives the guest bridge.** — `diverges`. Identical response names overwrite in the host map. SDK request duplicates survive its Vec-backed map but overwrite when deserialized as host QueryValue. Reference: crates/shamir-wasm-host/src/net_gateway.rs:32; crates/shamir-wasm-host/src/wasm/host_http.rs:89; crates/shamir-sdk/src/http.rs:99; crates/shamir-types/src/types/value.rs:269.
- **Guest values are wire-compatible with QueryValue for all supported Map values.** — `diverges`. Reserved colon-prefixed map keys are interpreted and renamed by host QueryValue decoding. Existing bidirectional fixtures use ordinary keys. Documented Dec/Big-to-Str and Set-to-List normalization is intentional, not a defect. Reference: crates/shamir-sdk/src/value.rs:1; crates/shamir-types/src/types/value.rs:217; crates/shamir-wasm-host/src/wasm/wasm_function.rs:577.
- **Guest memory references are bounds-checked.** — `supported`. Inspected input/import/output paths check negative signed inputs where applicable and range endpoints before slicing. This does not bound host-side retained values, serialization work or network response size. Reference: crates/shamir-wasm-host/src/wasm/wasm_function.rs:296; crates/shamir-wasm-host/src/wasm/wasm_function.rs:532; crates/shamir-wasm-host/src/wasm/wasm_function.rs:569.
- **Nested calls inherit the parent actor.** — `supported`. Actor cloning and child with_actor are present. The User(42) probe would distinguish reverting that propagation. The probe does not establish callee authorization or independently test DB/network/grant inheritance. Reference: crates/shamir-wasm-host/src/wasm/host_call.rs:78; crates/shamir-wasm-host/src/wasm/host_call.rs:118; crates/shamir-wasm-host/src/tests/nested_actor_tests.rs:125.
- **Argon2 hashing has effective concurrency and per-call cost bounds.** — `supported`. The actual delegated implementation caps simultaneous hashes at 16 and rejects excessive parameters before acquiring a permit. This establishes the cap, not semaphore liveness; a separate lost-notification witness remains. Reference: crates/shamir-wasm-host/src/builtin.rs:64; crates/shamir-funclib/src/crypto.rs:203; crates/shamir-funclib/src/crypto.rs:221.
- **The public from_binary path accepts raw WASM or serialized cwasm.** — `diverges`. Wasmtime 46.0.2 Module::from_binary compiles WASM bytes; serialized artifacts require unsafe deserialize APIs. Skipping the sanitizer on an ELF header would neither enable safe loading nor authenticate an artifact. Exact source: [Wasmtime 46.0.2 module API](https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/module.rs). Reference: crates/shamir-wasm-host/src/wasm/wasm_function.rs:260; Cargo.lock:4741; wasmtime 46.0.2 published src/runtime/module.rs.
- **Function gateway operations share the enclosing transaction.** — `diverges`. The broad authoring prose is stale. The specific gateway contract documents independent autocommit calls, and Known Limitations documents rejecting Call inside open transactions. Changes must preserve those explicit current semantics. Reference: crates/shamir-wasm-host/src/contract.rs:9; crates/shamir-wasm-host/src/db_gateway.rs:7; docs/guide-docs/KNOWN_LIMITATIONS.md:30.
- **Assigned tests are registered and provide discriminating resource/security oracles.** — `diverges`. All ten local topics are registered and selected by lib testing; downstream integration targets require full selection. Registration is supported, but the high-u64 deadline and unconditional-recursion aggregate assertions cannot identify their intended mechanisms. Compilation-dependent tests can return successfully after a skip. Reference: crates/shamir-wasm-host/src/lib.rs:57; crates/shamir-wasm-host/src/tests/mod.rs:1; scripts/test.sh:180; crates/shamir-wasm-host/src/tests/wasm_tests.rs:246.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- Current SUMMARY 4.7 / performance Claim 8: confirmed-open -&gt; not-applicable. Exact Wasmtime 46.0.2 source refutes linear export-table scanning; repeated mapped lookups alone do not prove an optimization defect.
- Current SUMMARY 5.4 / API Claim 5 and P2.19: confirmed-open -&gt; not-applicable. DB traps and HTTP runtime envelopes are explicitly documented different contracts; uniform recoverability is an optional new ABI.
- Current SUMMARY 3.5 / security Claim 7: confirmed-open -&gt; not-applicable. Trusted probe/optimizer environment inheritance is outside the cargo-build-specific scrub guarantee and has no demonstrated secret channel.
- Current SUMMARY 6.11 / lifecycle Claim 11: confirmed-open -&gt; not-applicable. UTF-8 fallback failure is unreachable under current transformations; prospective invariant-break hardening is not a present bypass.
- P2.28: partially-fixed -&gt; confirmed-open. The cap premise was refuted by pre-review code, not fixed by this plan; panic classification remains open, and independent inspection found a separate existing-gate liveness defect.
- Lowered copy-only 4.2 to low and HTTP header-loss 5.1 to medium: structural waste and multiplicity loss are established, but neither measured workload multipliers nor authentication compromise is demonstrated.
- The prior refutation of unbounded Argon2 hashing, required boxed futures, documented pin sentinel and optional net-guard split is independently supported. No later source fix is inferred from those counterexamples.
- Broad compiler-authority and ABI assurances require correction: validator compilation omits the compiler gate, synchronous getters reject async-required Stores, and reserved-prefix Map keys are reinterpreted.
- Rust Intel semantic-conformance and oracle discipline informed the distinction between current defects, deliberate contracts and unsupported performance premises. No implementation or verification execution was performed.

## Current follow-up order

1. Close validator source-compilation authority bypass together with valid-token forbidden-macro recognition; preserve uploaded-WASM behavior and the operator-configured compiler permission model.
2. Implement reentrant cancellation-safe aggregate fuel accounting, preserve or explicitly constrain the public u64 domain, and replace both resource-limit non-oracles.
3. Repair mixed-async getter allocator reentry and add actual guest present-value/control tests; declared imports are insufficient.
4. Offload source/JIT compilation and give compiler descendants, diagnostic collection and pipe readers explicit bounded ownership.
5. Preserve literal Map keys and duplicate HTTP headers across host/SDK boundaries, including compatibility with stored guests; reject malformed accepted requests explicitly.
6. Fix the existing KDF semaphore's check-to-wait protocol without adding redundant hashing caps.
7. Add atomic host-global retention quotas, require a functioning epoch driver, reconcile pool limits and implement last-owner engine lifecycle.
8. Repair single-key overwrites and prove multi-key rename semantics; correct stale grant docs, suffix glob behavior and metadata observability.
9. Keep optional DB-envelope redesign, pin representation and export caching separate from defect remediation; perform measured or convention-only cleanup last.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wasm-host — Consolidated 7-lens review (synthesized from the 2026-08-14 cross-crate sweep)

Crate: `crates/shamir-wasm-host/` — the WASM guest-function sandbox: guest compilation
(`cargo → wasm32` + sanitizer + linker allowlist), execution engine (wasmtime, fuel/epoch/
wall-clock/depth limits), host imports (`db_*`, `batch_*`, `global_*`, `http_fetch`, `call`),
and the catalogue metadata / registry / context types behind them.

Review basis: the seven lens reports produced by the 2026-08-14 cross-crate review, all read
in full and consolidated here — `correctness-tdd.md`, `concurrency-lockfree.md`,
`security-crypto.md`, `performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md` (this directory). Structure/tone/rigor
calibrated on the two exemplar syntheses:
`../shamir-client-node/SUMMARY.md` and `../shamir-transport-ipc/SUMMARY.md`. During
synthesis, 12 of the most load-bearing `file:line` references were spot-checked against the
crate source (`wasm_function.rs`, `compile.rs`, `net_gateway.rs`, `host_http.rs`,
`context.rs`, `registry.rs`, `wasm_engine.rs`, `meta.rs`, `host_call.rs`) — all verified
accurate; no defect found in the sources was missed by all seven lenses, so nothing is added
below. Read-only pass — no build/test/lint commands, no source modifications.

## Executive summary

The crate is structurally one of the sweep's model citizens — `scc::HashMap` + `THasher`
everywhere, zero `std::sync`/`parking_lot` locks, exemplary test/layout conformance, an
allowlist sanitizer with a linker-sync test, and thorough SSRF non-canonical-IP handling —
but it lands in the sweep's "needs focused remediation" band (0c / 7h raw) because three of
its load-bearing controls do not actually hold. First, the task-#612 aggregate fuel budget is
enforced in arrears: a nested `ctx.call` chain draws `(depth+1) × fuel` before any debit
lands, `fuel: u64::MAX` wraps the seed and rejects every invocation at the entry gate, and
both dedicated regression tests pass for reasons unrelated to what they claim to pin.
Second, the guest-compiler sandbox boundary leaks: the forbidden-macro scan is bypassed by
grammar-legal whitespace between macro name and `!` (host file reads baked into artifacts),
while the compile pipeline itself is a plain sync `pub fn` that pins tokio workers for up to
120 s on the async DDL path. Third, the HTTP egress codec collapses duplicate headers in
both directions (string-keyed map wire shape), silently breaking `Set-Cookie`-style auth.
Fix the fuel-budget trio, the scanner bypass, the blocking-compile pillar-2 violation, and
the header codec before anything else ships from this crate.

---

## 1. correctness-tdd

Section verdict (from the lens file): generally well-tested (10 topic-split test files, real
revert-proof narratives, a genuine sanitizer/linker cross-check), but two of the three
headline resource-limit tests are vacuous — they pass for reasons unrelated to what they
claim to pin.

### 1.1 — high — Aggregate fuel budget is enforced retroactively: in-flight chain can draw (depth+1) × fuel; doc overclaims the bound
*(primary; also flagged in [concurrency-lockfree] as its finding 1 — one defect, two lenses)*
- File:line: `crates/shamir-wasm-host/src/wasm/wasm_function.rs:433-447` (grant),
  `:583-587` (debit), module doc `:58-68`; test `src/tests/wasm_tests.rs:271-317`.
- Issue: the shared `Arc<AtomicI64>` budget is read at **entry** (`remaining <= 0` gate +
  `grant = min(remaining, limits.fuel)`) but debited only **after a call exits** (`fetch_sub`
  at `:587`). In a nested `ctx.call` chain, no level exits before the deepest level is
  reached, so every level's entry check sees the full budget and grants a full `limits.fuel`
  — exactly the per-Store-reset behavior #612 was meant to eliminate. The module doc claims
  "a guest that recurses via `ctx.call` can no longer execute N × `limits.fuel` instructions
  across N nested Stores", but worst-case total is `(depth_limit + 1) × fuel` (default:
  33 × 1e9) before any debit lands; the gate trips only in arrears (and `fetch_sub` happily
  drives the counter negative). The load-then-grant is also not an atomic reservation, so
  the bound would be overshootable N× under any future concurrent sharing of one budget.
- Failure scenario: a guest burns its full grant at each level before recursing and executes
  ~33 billion instructions (default limits) instead of the documented 1-billion aggregate
  ceiling; the depth-0 30 s wall-clock is the only remaining bound, and a mostly-awaiting
  guest stretches even that per-level via epoch resets. Any deployment relying on fuel as
  the metering/fairness bound silently loses the aggregate control.
- Suggested fix: reserve the grant atomically at entry (`fetch_sub(grant)` via a
  `fetch_update` CAS loop clamped to remaining; refund `grant − consumed` on exit — the Drop
  guard from finding 6.1 implements exactly this) or hold a reservation list; alternatively
  document the true bound (in-flight-depth × fuel) and enforce the strict bound at depth 0.
  Strengthen the test to assert the *mechanism* (see 1.3).

### 1.2 — high — `fuel > i64::MAX` immediately errors AND makes the wall-clock/epoch test vacuous
- File:line: `wasm_function.rs:436, 438-443`; `src/tests/wasm_tests.rs:239-268`.
- Issue: the lazily-seeded budget is `AtomicI64::new(limits.fuel as i64)` (`:436`).
  `WasmLimits.fuel` is `u64`, so any value above `i64::MAX` — e.g. `u64::MAX`, the natural
  "effectively unlimited" setting — wraps to `-1`, and the very next `remaining <= 0` gate
  returns `Compute("aggregate fuel budget exhausted across nested calls")` on the first
  call, even at depth 0 with zero work done. `wasm_wall_clock_deadline_interrupts_cpu_bound_guest`
  uses exactly `fuel: u64::MAX` (test line 246), so its `assert!(result.is_err())` and
  `elapsed < 10s` are satisfied microseconds in, by this early return — it never reaches
  `shamir_call`, never exercises the 300 ms deadline or the epoch deadline, and would stay
  green if epoch interruption were deleted entirely.
- Failure scenario: (a) an operator configuring `fuel: u64::MAX` gets every function
  invocation rejected with a misleading "budget exhausted" error; (b) the one dedicated test
  for the epoch-interruption pre-emption mechanism cannot fail on regression — the
  Red/Green discipline is violated in the exact place it matters most.
- Suggested fix: seed with `i64::MAX.min(limits.fuel)` (or switch the counter to
  `AtomicU64`/`AtomicI128`), and fix the test to use a large-but-in-range fuel (e.g.
  `i64::MAX as u64`) plus assert the error message is the epoch/deadline variant
  (`"wall-clock"`), not merely `is_err()`.

### 1.3 — high — `wasm_aggregate_fuel_exhausted_across_nested_calls` cannot distinguish aggregate from per-Store fuel — vacuous regression test
- File:line: `src/tests/wasm_tests.rs:270-317` (with `wasm_function.rs` grant/debit
  mechanics).
- Issue: the test descends a linear `ctx.call` recursion and asserts any error before
  `depth_limit=1000` proves the shared budget. Mechanically it cannot: (a) debits land only
  when a level exits, so during descent **every** level's entry check sees the full 10,000
  budget and gets a full grant — the shared counter can never stop a pure descent (finding
  1.1); the recursion ends at the depth-limit trap, which fires identically under the old
  reset-per-Store behavior; (b) if instead the WAT's per-level burn exceeds the grant
  (~16k fuel vs a 10,000 grant), level 0 traps on its **first** loop before ever calling
  `shamir_host.call`, again indistinguishable from plain per-Store fuel. Either branch makes
  `expect_err` pass with or without the #612 fix — the elaborate "under the OLD behavior
  this test would NOT catch a regression" comment describes a failure mode the test never
  sets up (no sequential *sibling* calls, the only shape the arrears-budget can catch).
- Failure scenario: a regression that silently reverts to per-Store fuel resets keeps this
  test green — precisely the regression it was written to catch.
- Suggested fix: reshape the guest to make **sequential sibling** nested calls in a loop
  from one level (each sibling exits and debits before the next is granted), size `fuel` so
  sibling k+1's entry gate trips after ~N siblings, and assert the specific
  `"aggregate fuel budget exhausted"` message. (Depends on fixing 1.1's reservation
  semantics for the chain case too.)

### 1.4 — medium — `GlobalVars::seed_env` panics on any non-UTF-8 environment variable
*(primary; also flagged in [security-crypto] as its finding 8 (nit) — one defect, two lenses)*
- File:line: `crates/shamir-wasm-host/src/context.rs:210-216` (verified).
- Issue: `std::env::vars()` panics if any env var's name or value is not valid Unicode
  (documented std behavior). `seed_env` iterates it unconditionally.
- Failure scenario: one non-UTF-8 env var in the host process (set by any external software;
  routine on Unix) turns every seeding call into a process abort — violating the crate's own
  "avoid `panic!`" error-handling rule — and it fails deterministically at every startup
  until the var is removed.
- Suggested fix: iterate `std::env::vars_os()` and skip (or lossy-convert) non-UTF-8
  entries, e.g. `filter_map(|(k, v)| k.into_string().ok().zip(v.into_string().ok()))`.

### 1.5 — low — Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits
- File:line: `src/wasm_engine.rs:96-98, 190` vs per-invocation `limits.max_memory_bytes` in
  `wasm_function.rs:457-459`.
- Issue: `pool.max_memory_size(WasmLimits::default().max_memory_bytes)` (64 MiB) is fixed at
  engine construction, while each `WasmFunction` may carry custom `WasmLimits` with a larger
  memory cap. Under the (default) pooling allocator, growth beyond 64 MiB fails at the pool
  ceiling regardless of the configured per-Store limit; the on-demand fallback
  (`SHAMIR_WASM_NO_POOL=1`) honors it — two allocators disagree on the same config.
- Failure scenario: a function configured with `max_memory_bytes = 256 MiB` works with
  pooling disabled and traps with "memory … exceeded" when pooling is on — an env-var-
  dependent behavior change that looks like a guest bug.
- Suggested fix: clamp/document that pooling caps memory at the 64 MiB default, or derive
  the pool's `max_memory_size` from the maximum limit the engine is intended to serve
  (engine-level config, not `default()`).

### 1.6 — low — Forbidden-macro scanner desyncs on the `'\''` char literal
- File:line: `src/compile.rs:327-346` (`find_char_literal_end` returns the *escaped* quote's
  index for `'\''`, leaving the real closing quote unblanked), `:229-244`.
- Issue: for source containing `'\''`, the escape branch stops at the escaped quote instead
  of the closing one, blanking `'\'` and leaving a stray `'` in the cleaned text. The
  scanner and rustc then disagree on lexing by one quote; subsequent code can be swallowed
  into a phantom literal (false negative) or scanned as code that rustc treats as a literal.
  The module doc disclaims completeness ("defense-in-depth check, not a sandbox") and no
  working bypass was demonstrated, so this is a correctness wart in a control, not an open
  hole (contrast the open hole at 3.1).
- Failure scenario: a guest source whose scanner-cleaned text differs lexically from what
  rustc compiles could in principle hide a forbidden invocation from the scan while
  remaining valid Rust.
- Suggested fix: in the escape branch, skip the escaped character and continue scanning for
  the closing quote; add a `'\''`-containing test to `compile_tests.rs` asserting a
  following `env!` is still detected.

### 1.7 — *(same defect as 2.1)* — blocking `compile_rust_source` on async paths
- (correctness-tdd finding 4, medium there; full write-up at 2.1 under its primary lens,
  which rated it high.)

### 1.8 — *(same defect as 2.2/2.3)* — non-atomic remove-then-insert in registry/context stores
- (correctness-tdd finding 7; full write-up at 2.2 (overwrite) and 2.3 (rename rollback).)

### 1.9 — *(same defect as 3.2)* — contradictory `net_grants` doc contract
- (correctness-tdd finding 5; full write-up at 3.2 under its primary lens.)

### 1.10 — *(same defect as 5.5)* — `decode_http_request` silent defaults
- (correctness-tdd finding 8; full write-up at 5.5 under its primary lens.)

### 1.11 — *(same defect as 6.3)* — host-import/ABI test-coverage gap
- (correctness-tdd finding 11 — which additionally notes `Params` has no test file at all
  and `EnvPolicy`'s doc example is dead since doctests are disabled crate-wide
  (`Cargo.toml:46`); full write-up at 6.3 under its primary lens.)

### 1.12 — nit — Nits bundle (correctness-tdd finding 12, five items, distributed to their primary lenses)
- `host_call.rs:16-27` duplicated doc block → **7.3** · duplicated `glob_matches` →
  **7.2** · leaked epoch-ticker thread per engine → **4.4** · `compile.rs:542,544`
  `unwrap_or("")` cargo args → **6.7** · `meta.rs:124-141` silent grant-entry drops →
  **5.7**.

## 2. concurrency-lockfree

Section verdict (from the lens file): with two exceptions, a model citizen of the five
pillars — all shared state on `scc::HashMap` with `THasher`, every `scc::*::len()` carries a
`// O(N) ack:` allow, zero banned locks, host imports follow a documented borrow-dance so no
lock/guard is held across `.await`, and Argon2id correctly offloads via `spawn_blocking`.
The two exceptions are serious (2.1 and 1.1); the rest are low-severity atomicity nits.

### 2.1 — high — `compile_rust_source` blocks up to 120 s and is called directly on tokio workers (pillar 2)
*(primary; also flagged in [correctness-tdd] as its finding 4 (medium) — one defect, two lenses)*
- File:line: `crates/shamir-wasm-host/src/compile.rs:454-456` (pub sync
  `compile_rust_source` — verified), `:462` (`compile_rust_source_with_timeout`), `:471`
  (`check_toolchain` spawns two subprocesses), `:592` (`wait_timeout`), `:648`
  (`maybe_wasm_opt`); live async call sites:
  `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172` (inside `async fn
  create_function_with_opts_as`), `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221`;
  related: `WasmFunction::from_binary`'s Cranelift compilation
  (`wasm_function.rs:275-289`) is likewise called inline at `function_management.rs:183` /
  `validator_management.rs:232`.
- Issue: pillar 2 mandates CPU-bound work cross to `tokio::task::spawn_blocking` (the crate
  itself honors this for Argon2id, `builtin.rs:63`). The compile pipeline is fully
  synchronous and long-running: a `cargo build` subprocess with a 120 s wall-clock cap, two
  toolchain probe subprocesses, and an optional `wasm-opt` pass. It is exposed as a plain
  `pub fn` with no async wrapper, and the workspace's only callers (both `async fn`s;
  `grep spawn_blocking` in `shamir-db` returns nothing) invoke it directly on the runtime.
- Failure scenario: every `CREATE FUNCTION ... FROM SOURCE` / validator creation pins one
  tokio worker thread for tens of seconds (up to 120 s on timeout, plus Cranelift
  compilation right after). Unrelated requests multiplexed on that worker stall for the
  duration; on a `worker_threads = 1` runtime the entire runtime freezes for the compile.
- Suggested fix: add async wrappers in this crate (`compile_rust_source_async` delegating to
  `tokio::task::spawn_blocking`, same for `WasmFunction::from_binary`/`from_wat` — wasmtime
  also recommends compiling `Module`s off the async context) and route the DDL call sites
  through them; at minimum, document the blocking contract on the pub fns and fix the two
  call sites in `shamir-db`.

### 2.2 — low — Non-atomic remove-then-insert overwrite in `replace` / `put` / `set`
*(primary; also flagged in [correctness-tdd] finding 7 and [performance-hotpath] finding 6 — one defect, three lenses)*
- File:line: `src/registry.rs:50-54` (`FunctionRegistry::replace` — verified),
  `src/context.rs:50-54` (`BatchContext::put`), `src/context.rs:164-168` (`GlobalVars::set`).
- Issue: all three implement overwrite as `remove_sync` followed by `insert_sync`, which is
  not a per-key atomic swap. (a) A concurrent reader landing in the window sees the key
  absent — `registry.get` returns `None` (a new invocation fails `NotFound`) or
  `globals.get`/`batch.get` report absent, despite docs claiming new invocations pick up the
  new artifact. (b) Two racing writers have no ordering guarantee: `A.remove; B.remove;
  B.insert(v2); A.insert(v1)` leaves the **older** value `v1` as final state, violating
  last-writer-wins. The crate's own `update`/`incr` methods already use scc's atomic entry
  API (`context.rs:218-227`), so the fix pattern is in-file.
- Failure scenario: two functions concurrently `global_set("shared", ...)` on the
  process-lifetime `GlobalVars`; the stale value survives. Or `replace("f", new_artifact)`
  races a burst of `invoke("f")` calls, some of which fail with a spurious `NotFound`.
  (Perf aside from the perf lens: the two-step form also pays two hash ops + two bucket-lock
  acquisitions per put on the `batch_put` hot path where a single-pass entry-API upsert pays
  one.)
- Suggested fix: use the scc entry API for overwrite (`entry_sync` → `Occupied(occ) =>
  *occ.get_mut() = v` / `Vacant(vac) => vac.insert_entry(v)`), making the swap atomic per
  key.

### 2.3 — low — `FunctionRegistry::rename` rollback can silently drop the function
*(primary; also flagged in [error-handling-lifecycle] finding 9 and inside [correctness-tdd] finding 7 — one defect, three lenses)*
- File:line: `src/registry.rs:73-89` (rollback at `:86` — verified `let _ =
  self.functions.insert_sync(from.to_string(), f)`).
- Issue: `rename` is a check-then-act sequence (`contains(to)` → `remove_sync(from)` →
  `insert_sync(to)`), with a compensating re-insert of `from` if `to` was taken mid-race.
  The compensation itself is fallible (`insert_sync(from, f)` fails if a racing
  `register(from, …)` re-took the name after the rename's remove), and its failure is
  swallowed with `let _ = ...` — the moved-in `Arc<dyn ShamirFunction>` is then dropped, so
  the function silently vanishes from the registry with no error and no log, while the
  caller receives `AlreadyExists(to)` implying a clean rollback.
- Failure scenario: `rename("a","b")` races `register("a", h)` in the window after rename's
  `remove_sync("a")` and before the rollback: `insert_sync("b")` fails (taken), the rollback
  `insert_sync("a")` also fails (re-taken), `f` is silently discarded — `invoke("a")`
  reaches the *other* function and `invoke("b")` returns `NotFound`.
- Suggested fix: handle the rollback failure explicitly (return a distinguishable error
  and/or `log::error!`), or restructure rename via the entry API on `to` plus a conditional
  remove so the compensation cannot lose the artifact.

### 2.4 — *(same defect as 6.2)* — epoch-ticker spawn failure swallowed with `.ok()`
- (concurrency-lockfree finding 5; full write-up at 6.2 under its primary lens.)

### 2.5 — *(same defect cluster as 6.8)* — cargo child not killed on the `wait_timeout` error path
- (concurrency-lockfree finding 6, nit; folded into the compile child-process teardown
  cluster at 6.8.)

### 2.6 — *(same defect as 1.1)* — aggregate fuel budget does not bound the nested-call descent
- (concurrency-lockfree finding 1, high; full write-up at 1.1 under its primary lens.)

## 3. security-crypto

Section verdict (from the lens file): no `unsafe`, no banned locks, no HMAC/SCRAM/TLS in
this crate; the boundary work is largely strong — allowlist sanitizer with linker-sync
regression test, SSRF guard handles non-canonical IP encodings and DNS-rebind pinning with
dedicated tests, secrets fail closed ("denied looks absent"), RI-7 confused-deputy fix
threads the actor through nested calls. Not-findings worth keeping on record (verified
clean): `verify_wasm_module` allowlist posture + two-direction sync test
(`wasm_sanitizer_tests.rs:248`); guest-memory accessors bounds-check on every path; denied
`env.*` reads return the same `Ok(0)` as absent keys (`host_globals.rs:79-83`) and the
`env.*` write path is unconditionally blocked (`:43-50`); actor propagation
(`host_call.rs:76-126`) with tests. The gaps are all untrusted-input handling around the
host compiler and egress guard.

### 3.1 — high — Forbidden-macro scan bypassed by whitespace/comment between macro name and `!`
- File:line: `crates/shamir-wasm-host/src/compile.rs:361-371` (verified: `bytes[after] ==
  b'!'` requires the bang immediately after the name; also `FORBIDDEN_MACROS` at `:112-118`,
  scan entry at `:138-141`).
- Issue: `find_forbidden_macro_in_clean` requires `!` to appear *immediately* after the
  macro name. In Rust, macro-invocation grammar (`SimplePath ! DelimTokenTree`) is
  token-based, so whitespace or a comment between the name and the bang is legal:
  `env !"HOME"`, `env /*x*/!("HOME")`, `include_str !"C:/Users/me/secret.txt"` all compile
  as real invocations, but the scanner (which blanks comments to spaces and then demands `!`
  at `name.len()` offset) sees `env    !(` and passes the source. The scan is documented
  (compile.rs module docs, CRIT-6 / audit #440 part A) as the control that "closes the
  cheapest exfiltration paths", and `tests/compile_tests.rs` exercises only the tight
  `env!("X")` form.
- Failure scenario: an authorized-but-malicious tenant submits a function body containing
  `include_str !"C:/Users/<operator>/.aws/credentials"` (or `env !"PATH"`). The scan passes,
  the child `cargo build` reads the host file at compile time and embeds it as a `const` in
  the `.wasm` artifact (or the scrubbed-but-identifying env values: `USERPROFILE`,
  `APPDATA`, `HOME` leak usernames/paths), which the tenant then retrieves/exfiltrates.
  `include_str!` has no path restriction — the env scrub does not contain it.
- Suggested fix: in the cleaned text, when a forbidden name is found, skip spaces/comments
  between the name and the next non-blank character before testing for `!`. Add adversarial
  regression tests: `env !"X"`, `env /*c*/!("X")`, `include_str \n !"f"`.

### 3.2 — medium — Contradictory fail-open vs fail-closed documentation of the `net_grants` default
*(primary; also flagged in [correctness-tdd] finding 5 and [api-wire-protocol] finding 2 — one defect, three lenses)*
- File:line: `crates/shamir-wasm-host/src/meta.rs:83-91` vs `:188-190` (both verified);
  enforcement consumer `crates/shamir-db/src/shamir_db/shamir_db/core.rs:805-851`
  (`build_net_gateway`).
- Issue: `FunctionMeta::net_grants`' doc (task #609) states "an EMPTY/absent `net_grants`
  means NO egress for this function" (fail-closed, restrictive-by-default, matching
  `secret_grants`). Twenty lines below, `CreateFunctionOptions`' doc states "empty
  `net_grants` = full DB-wide `net_allowlist`" (fail-open) — and even cross-references
  `[FunctionMeta::net_grants]` as its authority. Both descriptions live in the same file and
  cannot both be true; the enforcing code (`build_net_gateway`: `Some(grants) if
  grants.is_empty() => Vec::new()`, plus the `net_grants_empty_denies_all_egress` test)
  confirms restrictive-by-default, so `CreateFunctionOptions`' doc is stale pre-#609 text
  falsely promising default-created functions inherit the DB-wide allowlist.
- Failure scenario: a future change to egress wiring is implemented from the wrong doc — if
  the fail-open reading is (or becomes) the actual behavior, creating a user function
  without explicit `net_grants` silently grants it DB-wide egress, the exact default #609
  was landed to prevent; or a reviewer/operator reasons about the security posture (or ports
  the option downstream) from the wrong contract.
- Suggested fix: rewrite the `CreateFunctionOptions` doc to match #609 (empty = deny all
  egress; non-empty = intersect with the DB ceiling), cross-link `build_net_gateway`, and
  add a cross-crate test asserting empty-grants ⇒ `fetch` traps.

### 3.3 — medium — SSRF guard misses `0.0.0.0` (and other non-routable IPv4 ranges) — wildcard allowlist reaches loopback services
- File:line: `src/net_gateway.rs:455-481` (`is_private_or_loopback_ip` — verified: the IPv4
  arm checks only 127/8, 10/8, 172.16/12, 192.168/16, 169.254/16), canonicalization at
  `:305-334`.
- Issue: the IPv4 arm misses `0.0.0.0/8` ("this network" — on both Windows and Linux an
  outbound connect to `0.0.0.0` lands on the local host), plus `100.64.0.0/10` (CGNAT /
  some cloud-internal), `198.18.0.0/15`, `192.0.0.0/24`, and `224.0.0.0/4`/`240.0.0.0/4`.
  Both the string-level check (`check_host_allowed`) and the resolved check
  (`check_url_allowed_resolved`) key off the same predicate, so the miss survives both
  layers.
- Failure scenario: operator configures a broad allowlist entry (e.g. `*` or
  `*.guest-egress.example`). A guest calls `http_fetch({url: "http://0.0.0.0:5984/..."})`.
  `canonicalize_ip("0.0.0.0")` → not "private" by this predicate → string check passes;
  `lookup_host("0.0.0.0")` returns `0.0.0.0` → pin returned; the gateway connects to
  `0.0.0.0` and reaches a localhost-bound (metadata-style or admin) service that the
  exact-entry-only policy was designed to protect.
- Suggested fix: add `octets[0] == 0` to the IPv4 arm (and consider `100.64..191.255` CGNAT
  and `198.18/15`), mirroring the existing `private_ip_wildcard_denied` test with
  `0.0.0.0` / `00.00.00.00`-style encodings.

### 3.4 — low — No bound on concurrent Argon2id invocations; cost parameters fully caller-supplied at this layer
- File:line: `src/builtin.rs:42-83`.
- Issue: `argon2id` accepts caller-supplied `memory_kb`/`time`/`parallelism` and runs each
  call on `spawn_blocking` with no semaphore/permit limiting concurrent KDFs. Per-call
  ranges are delegated to funclib (the `"out_of_range"`/`"bad_params"` mapping at
  `builtin.rs:66-74` shows bounds exist), but nothing here caps how many KDFs run
  simultaneously, and guest fuel/epoch/wall-clock budgets do not apply inside
  `spawn_blocking` work.
- Failure scenario: a hostile tenant fans out many concurrent invocations hitting
  `argon2id`; with the tokio blocking pool defaulting to ~512 threads at ~19.45 MiB default
  cost each, that is ~9.5 GiB of hashing memory plus CPU saturation, degrading other tenants
  on a shared node.
- Suggested fix: wrap the `spawn_blocking` call in a process-wide `tokio::sync::Semaphore`
  sized to a small concurrency (e.g. 2–8), and/or clamp the maximum accepted
  `memory_kb`/`time` at this layer regardless of funclib's looser bounds.

### 3.5 — nit — `wasm-opt`/toolchain probes run with the full inherited host environment
- File:line: `src/compile.rs:648-669` (`maybe_wasm_opt`), `:699-710` (`check_toolchain`).
- Issue: the guest `cargo build` gets the CRIT-6 env scrub (`env_clear` + allowlist), but
  the `wasm-opt` and `cargo --version`/`rustup` probe invocations inherit the host's full
  environment. No realistic exfiltration channel today (they process only host-generated
  artifacts and log diagnostics), but the "no host secret reaches a child spawned from
  untrusted input" story is only true for one of the child processes.
- Suggested fix: apply the same `scrubbed_env()` allowlist to these invocations for
  uniformity.

### 3.6 — *(same defect cluster as 6.8)* — compile timeout kills `cargo` but not its `rustc`/build-script grandchildren
- (security-crypto finding 4, low; folded into the compile child-process teardown cluster at
  6.8 — the no-Job-Object manifestation.)

### 3.7 — *(same defect as 1.4)* — `seed_env` panics on non-UTF-8 env vars
- (security-crypto finding 8, nit; full write-up at 1.4 under its primary lens.)

### 3.8 — *(same defect as 6.2)* — epoch-ticker spawn failure silently disables the CPU-guest kill switch
- (security-crypto finding 6, low; full write-up at 6.2 under its primary lens.)

## 4. performance-hotpath

Section verdict (from the lens file): structurally sound under pillar 3 — `scc`/Fx-hash
primitives everywhere, annotated `O(N) ack` len calls, pooling allocator + `InstancePre`
amortizing instantiation, no hidden full scan on the per-invocation path, fuel/epoch/
wall-clock bounding per-call work by design. The real gaps are two unbounded-growth vectors
and a small set of avoidable per-invocation copies. No in-crate benches (the `wasm_invoke`
bench lives in `shamir-engine`).

### 4.1 — medium — Unbounded process-lifetime growth of `GlobalVars` via guest `global_set`
- File:line: `src/context.rs:164-168` via `src/wasm/host_globals.rs:55`.
- Issue: `host_global_set` forwards arbitrary guest-supplied keys/values into the
  process-lifetime, shared-across-all-batches `scc::HashMap` with no entry-count or
  byte-size quota, no eviction, and — critically — no host import exposing removal
  (`global_remove` exists only on the native `FnCtx`, not in the linker surface). Every
  distinct key a guest ever writes persists for the life of the process, outside guest
  memory (so the per-Store 64 MiB `ResourceLimiter` does not bound it).
- Failure scenario: a malicious or buggy guest loops `global_set(format!("k{i}"), ...)` with
  distinct keys. Each top-level call gets a fresh fuel budget and 30 s of wall clock, so
  aggregate host RSS grows monotonically across calls and eventually OOMs the server; even
  benign functions permanently leak every transient key they set. The sanitizer doc treats
  guests as untrusted bytecode, so this is within the stated threat model.
- Suggested fix: add a cardinality/byte cap (e.g. `AtomicUsize` mirror of `scc` len per
  pillar 3, enforced in `GlobalVars::set` with a trap on overflow), or expose a
  `global_remove` host import plus per-function namespacing so entries are reclaimable when
  a function is dropped.

### 4.2 — medium — Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy
- File:line: `src/wasm/wasm_function.rs:407-410`.
- Issue: every `WasmFunction::call` does `QueryValue::Map(params.raw().clone())` — a full
  deep clone of the `TMap<String, QueryValue>` (every key String and every nested value
  recursively cloned) — then `to_bytes()` (full msgpack encode), then `.to_vec()` (a third
  full copy of the encoded `Bytes`). Only `.len()` and `copy_from_slice(&input)` are used
  afterwards, so the `.to_vec()` is pure waste (`Bytes` derefs to `&[u8]`).
- Failure scenario: the contract doc says the same function serves the `where`/`set`/
  key-generation sites, i.e. per-row invocation. With a large param map (bulk row payload)
  over N rows, the host churns ~3×N×payload bytes per query where ~1×N×payload is required —
  hidden linear overhead stacked under every WASM call (each row also pays Store setup, so
  it compounds).
- Suggested fix: drop the `.to_vec()` (keep `Bytes` and pass `&input`); eliminate the deep
  clone by encoding from a borrowed view (a `Params::to_bytes()` helper that serializes the
  map without re-owning it, or constructing the `QueryValue::Map` once at the call site).

### 4.3 — low — Unbounded stdout/stderr buffering of the guest `cargo build`
- File:line: `src/compile.rs:571-586` (readers), `:614-622` (error path).
- Issue: the two pipe-drainer threads `read_to_end` into unbounded `Vec<u8>`s, and the
  failure path additionally does `String::from_utf8_lossy(&stderr)` (a second full-size
  allocation) embedded into a `FunctionError::Compute` string. The 120 s `wait_timeout`
  bounds duration but not volume — a hostile or pathological build script can emit GBs of
  output within the window.
- Failure scenario: an authorized actor compiles a guest whose build script spams stderr in
  a tight loop; host memory balloons to the pipe output size ×2 (raw + lossy copy) before
  the timeout kills the child.
- Suggested fix: drain with a bounded cap (fixed-size ring/limited buffer, e.g. first
  64 KiB + total-bytes counter) and truncate what goes into the error string.

### 4.4 — low — Epoch-ticker thread + full engine leaked per `WasmEngine::new()` — no shutdown path
*(primary; also flagged in [error-handling-lifecycle] finding 10 (nit) and in the [correctness-tdd] nits bundle — one defect, three lenses)*
- File:line: `src/wasm/wasm_engine.rs:150-163`.
- Issue: every `WasmEngine::new()` spawns a detached, never-terminating ticker thread
  (`.ok()` at `:162` discards even spawn failure — see 6.2 for that half) and builds a full
  wasmtime `Engine` (JIT infra, disk cache worker, 128-slot pooling allocator with ~768 GiB
  virtual reservation). The doc justifies the leak by "engines here are long-lived
  singletons", but nothing enforces that: `shamir-db` constructs one per `ShamirDb`
  instance (`core.rs:165`), and `shamir-engine/benches/wasm_invoke.rs:153` constructs one
  per bench scenario.
- Failure scenario: an application that opens/closes databases repeatedly (per-tenant DBs,
  test suites, CLI one-shots) accumulates one thread + one pooled engine per open for the
  whole process lifetime; VA reservation and thread count grow without bound (the crate's
  own ~10 tests each leak one).
- Suggested fix: share one engine via a process-wide `OnceLock<Arc<WasmEngine>>` (engines
  are config-identical here), or give `WasmEngine` a `Drop`/shutdown signal (e.g.
  `Arc<AtomicBool>` checked by the ticker) so the thread dies with the engine.

### 4.5 — low — `host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc`
- File:line: `src/wasm/host_call.rs:119` (`with_secret_grants(secret_grants.iter().cloned())`).
- Issue: `FnCtx::with_secret_grants` does `Arc::new(grants.into_iter().collect())`, so each
  nested `ctx.call` re-allocates a fresh `TFxSet` and re-inserts all G grants, even though
  the parent's `Arc<TFxSet<String>>` was already cloned into `secret_grants` at
  `host_call.rs:84` and could be shared as-is. (`repo` is likewise re-cloned per nested
  call; unavoidable with the current `String` field but cheap to switch to `Arc<str>`.)
- Failure scenario: a recursion/fan-out chain of depth D with G grants performs D set builds
  + D `Arc` allocations + D×G string clones per request instead of D refcount bumps; small
  today (depth limit 32) but it is per-call allocation in the recursion loop for zero
  benefit.
- Suggested fix: add a `with_secret_grants_arc(Arc<TFxSet<String>>)` builder (pub(crate),
  like `with_fuel_budget`) and use it in `host_call`.

### 4.6 — nit — Dead `Arc` clones on every `batch_get` / `global_get` host import
- File:line: `src/wasm/host_batch.rs:53-55, 89-90`; `src/wasm/host_globals.rs:94-96, 130`.
- Issue: both handlers clone `Arc<BatchContext>` and `Arc<GlobalVars>` out of
  `caller.data()` purely for "borrow-scope hygiene" and then discard them via
  `let _ = (batch, globals);` — two atomic refcount bump/decrement pairs per call of the
  most loop-friendly host imports.
- Suggested fix: delete the clones (the borrow conflict they worked around no longer exists
  — `alloc_fn`/`alloc_typed` are obtained after the data reads) and drop the `let _`
  suppressors.

### 4.7 — nit — Host-import export re-resolution + `typed()` rebuild per call
- File:line: `src/wasm/wasm_function.rs:333-341, 349-352` (`write_value_to_guest`/
  `write_bytes_to_guest`); `src/wasm/host_batch.rs:57-61, 76-79`.
- Issue: every host import that writes back into guest memory re-runs
  `get_export("shamir_alloc")` + `get_export("memory")` and rebuilds a `TypedFunc` from the
  untyped `Func` — a scan of the instance export table plus wrapper construction per call
  (twice per `batch_get`/`global_get`).
- Failure scenario: guest loops hammering `batch_get`/`global_get` pay a fixed export-lookup
  tax on every iteration; N exports makes each lookup O(exports).
- Suggested fix: resolve `shamir_alloc`/`memory` once in `WasmFunction::call` (or at first
  use) and cache the `TypedFunc`/`Memory` handles in `HostState`; fall back to the current
  path if absent.

### 4.8 — nit — `Box<dyn Future>` heap allocation per nested async host import
- File:line: `src/wasm/host_call.rs:44` (pattern shared by `host_db.rs:19, 67, 113, 169`,
  `host_http.rs:117`).
- Issue: the `func_wrap_async` handlers return `Box<dyn Future ... + Send + '_>` — one heap
  allocation per db/call/http import invocation — where a concrete `impl Future + Send + '_`
  return type on the free function is accepted by wasmtime and keeps the future inline.
- Suggested fix: return `impl Future` (naming the concrete async-block type) from the
  handler functions; keep `Box` only if a MSRV/toolchain constraint forces it.

### 4.9 — nit — Redundant artifact copies in `maybe_wasm_opt`
- File:line: `src/compile.rs:651-655, 688`.
- Issue: on every compile without `wasm-opt` installed (the common case) the artifact is
  copied once via `wasm_bytes.to_vec()` despite `wasm_bytes` already being an owned `Vec`
  upstream; the same copy recurs on wasm-opt failure arms. Off the request hot path (DDL/
  compile time only), hence nit.
- Suggested fix: pass/return `Vec<u8>` by value and `std::mem::take` in the pass-through
  arms.

### 4.10 — *(same defect as 2.2)* — `put`/`set` pay two hash ops per write
- (performance-hotpath finding 6's cost framing; folded into 2.2 with its fix.)

## 5. api-wire-protocol

Section verdict (from the lens file): the guest ABI (packed `ptr<<32|len` returns,
`0 = absent`, msgpack `QueryValue` payloads) is consistent between host and `shamir-sdk`,
the import surface is a single auditable `SANCTIONED_HOST_IMPORTS` const kept in sync with
the linker by a dedicated test, and the crate is fully builder-rule compliant (no
`serde_json` anywhere; queries/filters stay opaque msgpack delegated to the guest's query
builder). The main weakness is the HTTP egress codec; the rest are contract/doc mismatches
and stringly-typed errors.

### 5.1 — high — HTTP wire codec collapses duplicate headers (`Set-Cookie` loss on both directions)
- File:line: `crates/shamir-wasm-host/src/wasm/host_http.rs:86-97` (encoder — verified:
  headers serialized into a string-keyed `QueryValue::Map`), `:39-65` (decoder); peer codec
  `crates/shamir-sdk/src/http.rs:97-111, 139-148` (same defect lives in both crates — one
  shared wire shape).
- Issue: `encode_http_response` serialises response headers into a `QueryValue::Map`
  (string-keyed `TMap`), so repeated header names from a real HTTP response — most
  importantly multiple `Set-Cookie`, but also duplicated `WWW-Authenticate`/`Via` — are
  silently collapsed to the last value before the guest ever sees them. The request decoder
  accepts headers as Map *or* List-of-pairs, but the SDK's `HttpRequest::to_value` emits a
  Map, so guest-set duplicate request headers are collapsed guest-side too. The wire shape
  (`Map<Str,Str>`) cannot represent valid HTTP traffic, and the loss is silent and
  unfixable from the guest.
- Failure scenario: a function calls an API that responds `Set-Cookie: a=1` +
  `Set-Cookie: b=2` (session + CSRF token); the guest's `resp.headers()` contains only one
  cookie; cookie-jar auth silently breaks with no error anywhere.
- Suggested fix: encode response headers as `QueryValue::List` of `[name, value]` pairs
  (the decoder already accepts this shape for requests); switch the SDK encoder to the same
  list shape; keep accepting the Map form for back-compat but document it as
  legacy/deprecated, and add a round-trip test with duplicate header names.

### 5.2 — medium — `FnCtx` docs promise secret-grant gating on `global_get` that only the WASM host import enforces
- File:line: `src/context.rs:289-297` and `:390-397` (docs) vs `:426-428` (verified:
  `FnCtx::global_get` reads `GlobalVars` unguarded); scope note in
  `src/wasm/host_globals.rs:17-24`.
- Issue: the `FnCtx` type doc ("`global_get(\"env.X\")` returns absent when `X` is not in
  `secret_grants`") and `with_secret_grants`' doc ("Only env variable names listed here can
  be read via `global_get`") attribute the enforcement to `FnCtx::global_get`. It is not
  there: gating exists only in the guest-facing `shamir_host::global_get` import.
  `host_globals.rs` explicitly documents this split, but the `FnCtx` docs contradict it —
  the public native API's documented contract is not its implemented contract.
- Failure scenario: a native (compiled-in) `ShamirFunction` author relies on the documented
  `ctx.global_get("env.X")` gating and gets the secret anyway; conversely a security audit
  of the native path reads the wrong guarantee.
- Suggested fix: either enforce the grant check in `FnCtx::global_get`/`global_keys`
  (making the docs true for both native and guest paths), or correct the `FnCtx` docs to
  state that `secret_grants` are enforced only at the guest host-import boundary and that
  `FnCtx::secret_grants()` is provided for native impls to self-enforce.

### 5.3 — medium — Stringly-typed errors across the public gateway traits and egress guards
*(primary; also flagged in [error-handling-lifecycle] finding 4 (low) — one defect, two lenses)*
- File:line: `src/db_gateway.rs:56-87`; `src/net_gateway.rs:55-61, 69, 110, 157-209`.
- Issue: `DbGateway::{get,insert,query,execute}` and `NetGateway::fetch` return
  `Result<_, String>`, and the exported guard fns (`check_host_allowed`, `check_url_allowed`,
  `check_url_allowed_resolved`) are `Result<_, String>`. This is a library crate whose own
  `FunctionError` (`thiserror`) is the house style per CLAUDE.md; the gateway boundary
  discards all structure — callers (and the host imports that re-wrap them into trap
  messages, e.g. `host_db.rs:53` `format!("db_get: {e}")`) cannot distinguish
  deny-vs-unavailable-vs-transport failure, causes can't be chained, and no variant can be
  added without string-format coupling.
- Failure scenario: a `db_execute` batch failure's structured error codes become a formatted
  trap string; a guest or embedder wanting to retry on timeout but fail on allowlist denial
  (or distinguish "operator forgot to allowlist" from "transient DNS failure") must
  substring-match English error text.
- Suggested fix: introduce a small `thiserror` enum per gateway (e.g. `DbGatewayError`,
  `NetGatewayError::{Denied, DnsBlocked, Transport}`) with `Display` used only at the
  trap/format boundary; keep the `String`-returning fns as thin wrappers if external callers
  depend on them.

### 5.4 — medium — Inconsistent guest-facing error contract across sibling host imports (envelope vs uncatchable trap)
- File:line: `src/wasm/host_http.rs:99-114` (catchable envelope) vs `src/wasm/host_db.rs:12-58,
  160-190` and `src/wasm/host_call.rs:19-27` (traps).
- Issue: `http_fetch` deliberately returns runtime failures as a catchable
  `[false, "error"]` envelope and traps only for config bugs, while
  `db_get`/`db_insert`/`db_query`/`db_execute`/`call` trap on every gateway failure. Within
  one ABI, identical failure classes (denied, not-found-at-runtime, transport error) are
  catchable for HTTP and fatal for DB. The SDK docs say "Traps on error", but as API design
  the asymmetry means guest code can gracefully handle an egress failure yet cannot handle a
  `db_execute` batch rejection — on `wasm32-unknown-unknown` with `panic=abort`, a trap
  terminates the whole function invocation.
- Failure scenario: a function runs a validation batch via `db_execute` that fails a
  uniqueness check; the guest cannot inspect the failure or return
  `FunctionError::User`-style feedback — the entire invocation traps as `Compute`, and the
  wire client sees an opaque host error instead of a structured batch error.
- Suggested fix: adopt the `http_fetch` envelope convention for `db_execute` at minimum
  (its `BatchResponse` already has an error channel — return it as payload instead of
  converting to a trap), and document the per-import error contract in one place (the
  `wasm_function.rs` ABI doc block).

### 5.5 — medium — `decode_http_request` silently coerces malformed `headers`/`body` to empty while `method`/`url` default to `""`
*(primary; also flagged in [correctness-tdd] finding 8 (low) — one defect, two lenses)*
- File:line: `src/wasm/host_http.rs:26-34` (`get_str` maps absent → `Ok(String::default())`
  — verified via `.unwrap_or_default()`), `:39-70` (`headers` wrong shape and non-`Bin`
  body → `Vec::new()` — verified).
- Issue: a `headers` value of the wrong shape and a `body` that is not `Bin` (e.g. the very
  plausible `Value::Str` body) are silently replaced with empty defaults, whereas
  `method`/`url` of the wrong *type* are hard errors — but *absent* `method`/`url` proceed
  to the gateway as empty strings. A decoded-but-wrong request is sent with no body/headers
  instead of failing the fetch. The codec's strictness is asymmetric in both directions.
- Failure scenario: a guest builds `{"method": "POST", "url": ..., "body": Str(json)}`; the
  host sends a body-less POST; the remote API returns 400/empty and the function's error
  handling blames the remote service — the actual protocol mistake is invisible. A request
  missing `method` fails later with a confusing allowlist/URL error instead of "missing
  field".
- Suggested fix: reject non-`Bin` `body` and wrong-shaped `headers` with the same `Err` used
  for `method`/`url` (or explicitly accept `Str` body via UTF-8 encoding, but then document
  it); treat absent `method`/`url` as errors too; add codec unit tests for the
  malformed-input matrix.

### 5.6 — medium — `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR`
- File:line: `src/compile.rs:484-497`.
- Issue: the public `compile_rust_source`/`compile_rust_source_with_timeout` API resolves
  `shamir-sdk` via `env!("CARGO_MANIFEST_DIR")/../shamir-sdk` with no parameter or
  environment override. The compiled binary retains a path into the developer's source tree;
  on any deployment where that layout doesn't exist, every `CREATE FUNCTION ... SOURCE`
  fails at `canonicalize` with `resolving sdk path`. The function is public API whose only
  working environment is a dev checkout, and that constraint is undocumented.
- Failure scenario: the single shipped binary (project goal: self-contained, no external
  runtime deps) is installed on a server; the first user attempts a source-based function
  and gets `resolving sdk path: ...` with no recourse.
- Suggested fix: allow an override (function parameter or `SHAMIR_SDK_PATH`-style env
  checked before the manifest-relative default), and document in the function's doc comment
  that the default only works in a source checkout.

### 5.7 — low — Catalogue record decoding has silent fallbacks and no format versioning
*(primary; also flagged in [error-handling-lifecycle] finding 12 (nit) and in the [correctness-tdd] nits bundle — one defect, three lenses)*
- File:line: `src/meta.rs:110-148`.
- Issue: `FunctionMeta::from_record` silently coerces unknown `visibility`/`security`
  strings to Private/Invoker and silently drops non-string entries in
  `secret_grants`/`net_grants` (`filter_map`). The fallback direction is fail-safe
  (fail-closed), but there is no schema/version marker on the persisted record, so a future
  enum variant (or a corrupt field) is indistinguishable from a default — a newer node's
  `Security` variant read by an older node silently downgrades with nothing in logs, and
  corrupt grant arrays truncate silently.
- Suggested fix: at minimum `log::warn!` on any fallback/dropped-entry path (name + raw
  value); consider a `format_version` field injected by `inject_into` so forward-compat
  decisions are explicit.

### 5.8 — low — `ResolvedPin::pinned_ips` uses an empty-Vec sentinel for "do not pin"
- File:line: `src/net_gateway.rs:119-131, 165-175`.
- Issue: "empty means do not pin" overloads a `Vec` with a second meaning (exact-allowlist
  path). An `Option<Vec<IpAddr>>` (`None` = no pin) would make the two paths unambiguous at
  the type level; as written, a future caller that forgets the sentinel treats "no pin" and
  "validated set of IPs" uniformly and may pin nothing when it believed it had validated
  addresses.
- Suggested fix: change `pinned_ips: Vec<IpAddr>` to `Option<Vec<IpAddr>>` (pre-release, no
  compat constraint), or at minimum rename/document the sentinel at the type.

### 5.9 — low — Internal audit-tracking references leaked into public API docs
- File:line: `src/net_gateway.rs:105-109, 118, 133, 148, 224` (e.g. "see finding 2c",
  "finding 2c DNS-rebind TOCTOU fix").
- Issue: exported items (`check_url_allowed`, `check_url_allowed_resolved`, `ResolvedPin`)
  carry doc comments referencing "finding 2c" — identifiers from an internal audit that mean
  nothing to a crate consumer reading generated docs. The TOCTOU/pinning explanation itself
  is genuinely good and should stay.
- Suggested fix: keep the substance but phrase it without the audit-tracking shorthand, or
  move the tracking references to non-doc comments.

### 5.10 — *(same defect as 7.2)* — `glob_matches` duplicated in two security-relevant matchers
- (api-wire-protocol finding 9, low; full write-up at 7.2 under its primary lens.)

### 5.11 — *(same defect as 6.3)* — wire codecs, `db_*` imports, and the depth limit have no in-crate tests
- (api-wire-protocol finding 11, low; folded into 6.3.)

### 5.12 — *(same defect as 7.3)* — duplicated doc-comment block on `host_call`
- (api-wire-protocol finding 13, nit; full write-up at 7.3 under its primary lens.)

### 5.13 — *(same defect as 7.5)* — unused `serde` dependency
- (api-wire-protocol finding 14, nit; full write-up at 7.5 under its primary lens.)

### 5.14 — *(same defect as 3.2)* — `CreateFunctionOptions` doc states the opposite of the actual empty-`net_grants` semantics
- (api-wire-protocol finding 2; full write-up at 3.2 under its primary lens.)

## 6. error-handling-lifecycle

Section verdict (from the lens file): the crate holds the line well — a single `thiserror`
`FunctionError` enum, `?` propagation throughout, no unguarded `unwrap`/`expect` in
production paths, graceful degradation with `log::warn!` for cache/pooling setup, fail-closed
sanitizer. The weak spots are concentrated in error-path cleanup and observability (fuel
debit vs cancellation, the swallowed ticker-spawn failure, compile child-process teardown)
and in the missing trap-path test layer.

### 6.1 — medium — Aggregate fuel-budget debit is skipped when the call future is cancelled (cancelled task permanently leaks budget)
- File:line: `src/wasm/wasm_function.rs:583-589` (debit — verified), `:495-581` (async block
  with the await points).
- Issue: the debit of consumed fuel back into the shared `Arc<AtomicI64>` budget
  (`fuel_budget.fetch_sub(consumed, ...)`) is straight-line code executed *after* the async
  block's `.await`. The task-#612 doc comment promises the debit happens "exactly once per
  `call`, on every exit path", but a dropped future is an exit path it does not cover: if
  the enclosing task is aborted at any of the `.await` points (`instantiate_async`,
  `alloc_fn.call_async`, `call_fn.call_async` inside the depth-0 `timeout`), lines 583-589
  never run and the fuel already consumed by the guest is never returned to the shared
  counter. There is no `Drop` guard. (Also: `store.get_fuel().unwrap_or(0)` at `:585` fails
  toward over-debiting the full `grant`.)
- Failure scenario: a batch executor or connection task with its own upstream timeout/
  disconnect aborts the `WasmFunction::call` future mid-guest-execution. The consumed fuel
  stays debited from the aggregate budget forever; subsequent (perfectly legitimate) calls
  on the same `FnCtx` chain then fail with
  `Compute("aggregate fuel budget exhausted across nested calls")` even though no guest work
  is running — a permanent, invisible capacity regression until the ctx chain is discarded.
- Suggested fix: move the debit into a `Drop` guard (struct holding `Arc<AtomicI64>` +
  `grant` + a handle to the `Store`'s remaining fuel) created before the async block; on
  drop, read `store.get_fuel()` and debit the difference. This makes cancellation, panics,
  and early returns all debit exactly once — and is the same guard the 1.1 reservation fix
  needs. Prefer failing toward *not* debiting on the `get_fuel()` error case.

### 6.2 — medium — Epoch-ticker thread spawn failure is swallowed with `.ok()` — wall-clock pre-emption silently disabled, nothing logged
*(primary; also flagged in [concurrency-lockfree] finding 5 (low) and [security-crypto] finding 6 (low) — one defect, three lenses)*
- File:line: `src/wasm/wasm_engine.rs:154-163` (`.ok()` at line 162 — verified).
- Issue: `spawn_epoch_ticker` discards the `io::Result<JoinHandle>` with `.ok()`. If the OS
  thread cannot be spawned (thread-limit/resource exhaustion), construction succeeds
  silently but the engine epoch never advances, so every Store's `set_epoch_deadline` (set
  in `WasmFunction::call`, `wasm_function.rs:485-487`) never fires. Per the module's own
  analysis (`wasm_engine.rs:43-51`, `wasm_function.rs:449-455`), epoch interruption is the
  *only* mechanism that can preempt a pure-CPU guest that never hits a host `.await` — fuel
  can be set arbitrarily high and `tokio::time::timeout` cannot preempt a future that never
  yields. The two sibling graceful degradations in the same constructor (disk cache, pooling
  allocator) both `log::warn!` on failure; this one — the *safety* mechanism — logs nothing.
- Failure scenario: under thread pressure, an engine is built without a ticker. A pure-CPU
  guest (fuel configured generously, e.g. `u64::MAX` as `wasm_tests.rs:246` itself
  demonstrates is a supported configuration) then runs unimpeded: the `tokio::time::timeout`
  backstop cannot fire either, because a non-yielding guest never lets the timer poll. The
  only remaining bound is fuel exhaustion — the exact "pins a worker indefinitely" hazard
  epoch interruption was added to close.
- Suggested fix: at minimum `log::error!` (matching the file's own degradation-logging
  pattern) naming the lost guarantee; better, treat it as fatal and return
  `Err(FunctionError::Compute(...))` from `WasmEngine::new` — a half-functioning engine
  whose wall-clock bound silently doesn't exist is worse than a failed startup for a
  reliability-first database.

### 6.3 — low — No in-crate test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, OOB result pointers, missing exports)
*(primary; also flagged in [correctness-tdd] finding 11 (which adds: `Params` has no test file, doctests are disabled crate-wide, compile happy-path tests silently SKIP on `ToolchainUnavailable`), [api-wire-protocol] finding 11, and [style-claude-md] finding 6 (locality: coverage lives only in `shamir-db/tests/functions_lifecycle.rs:1116-1280`, with SKIP paths on toolchain-less hosts) — one defect, four lenses)*
- File:line: `src/tests/` (whole directory); cf. `wasm_sanitizer_tests.rs:68-106` (imports
  declared, never called) and `src/wasm/host_call.rs:97-101` (depth-limit trap).
- Issue: no test in this crate ever *invokes* a `shamir_host` import through guest code. All
  eight host imports' error paths are unexercised: `db_get/db_insert/db_query/db_execute`
  "no db gateway" traps (`host_db.rs:45-47, 92-94, 145-147, 181-183`), `http_fetch` "no net
  gateway" trap (`host_http.rs:138-142`), the `env.*` write-protection trap
  (`host_globals.rs:46-50`), secret-grant-denied-looks-absent (`host_globals.rs:79-83`),
  msgpack/UTF-8 decode failures, and `call`'s depth-limit trap (`host_call.rs:97-101`) — the
  recursive-fuel test always exhausts fuel before reaching the depth limit, so that branch
  never runs. Also untested: `decode_http_request`/`encode_http_response` (the exact shape
  contract behind 5.1/5.5), a module exporting no `memory` (`wasm_function.rs:502-504`), a
  guest returning an out-of-bounds result pointer (`wasm_function.rs:570-574`), and
  `Params` (`bytes`/`str`/`u32`/`opt_u32` boundaries — no test file at all).
- Failure scenario: a refactor of `HostState` threading or the borrow-dance in any host
  import can silently break a trap path (e.g. turning the fail-closed `env.*`
  write-protection into a silent no-op) with a green suite — precisely the drift the
  sanitizer's cross-check test was built to prevent for the import *names*, but no
  equivalent exists for the imports' *behaviour*. This is how findings 5.1 and 5.5 survived
  unnoticed.
- Suggested fix: add `tests/host_import_tests.rs` in the established `tests/` layout: small
  WAT modules that actually call `db_get` (with and without a gateway), `http_fetch` (without
  a gateway), `global_set("env.X", ...)` (must trap), `global_get("env.X")` ungranted (must
  return 0), a self-recursive caller with a small `depth_limit` (must surface the depth-limit
  error), plus `host_http_wire_tests.rs` (encode/decode round-trips incl. duplicate headers
  and malformed shapes), a `params_tests.rs`, and a `host_globals_tests.rs` topic (the crate
  demonstrably can test host imports end-to-end without the facade — `nested_actor_tests.rs`
  does it for `call`).

### 6.4 — *(same defect as 5.3)* — public gateway traits return `Result<_, String>`
- (error-handling-lifecycle finding 4, low; full write-up at 5.3 under its primary lens.)

### 6.5 — low — Panic inside the Argon2id blocking task is collapsed into `FunctionError::Cancelled`, discarding the panic payload
- File:line: `src/builtin.rs:63-83` (`.map_err(|_| FunctionError::Cancelled)` at line 83).
- Issue: the `spawn_blocking(...).await` result is mapped with `|_|`, discarding the
  `JoinError`. A panic in the closure (or an aborted blocking task) both become `Cancelled`,
  whose message ("function task cancelled") describes only one of the two cases and names
  neither the panic message nor location.
- Failure scenario: a future funclib change panics on some input (e.g. an assert in the
  KDF); production logs show a stream of "function task cancelled" for user-visible
  failures, sending operators chasing cancellation/backpressure instead of the actual
  defect. The panic info is unrecoverable.
- Suggested fix: branch on the `JoinError`: `if e.is_panic()` →
  `FunctionError::Compute(format!("argon2id task panicked: {payload}"))` (extract via
  `e.into_panic()` + `downcast_ref::<&str>/<String>`), else `FunctionError::Cancelled`.
  Optionally add a dedicated `Panicked(String)` variant.

### 6.6 — low — `map_wasm_error` classifies traps by substring-matching the error message
- File:line: `src/wasm/wasm_function.rs:593-602` (verified: `msg.contains("fuel")` /
  `msg.contains("epoch")`).
- Issue: fuel vs. epoch-deadline vs. generic traps are discriminated with substring matches
  on wasmtime's Display text. This couples error *semantics* to wasmtime's prose: the
  strings are not a stable API across wasmtime major versions (the crate tracks wasmtime
  `46`, so this will be exercised), and any trap whose message merely contains those
  substrings is misclassified.
- Failure scenario: after a wasmtime bump rewords the fuel message, every exhausted-guest
  call starts surfacing as the generic "shamir_call trap: ..." — the fuel-specific operator
  message silently degrades, and no test fails because `wasm_fuel_exhaustion_traps` only
  asserts `FunctionError::Compute(_)`. Conversely, a genuine guest trap whose message
  happens to embed "fuel" is misreported as budget exhaustion.
- Suggested fix: prefer typed discrimination
  (`wasmtime::Error::downcast_ref::<wasmtime::Trap>()` / `TrapCode::OutOfFuel` /
  epoch-interrupt codes where available in wasmtime 46), keeping the substring check only as
  a last-resort fallback, and assert the *specific* mapped message in
  `wasm_fuel_exhaustion_traps`.

### 6.7 — low — `compile.rs` degrades invalid-UTF-8 temp paths into `unwrap_or("")` arguments for cargo instead of a typed error
*(primary; also flagged in the [correctness-tdd] nits bundle — one defect, two lenses)*
- File:line: `src/compile.rs:541-544`.
- Issue: `tmpdir.path().join("Cargo.toml").to_str().unwrap_or("")` (and the same for
  `--target-dir`) feed an empty string to cargo if the temp path is not representable as
  UTF-8 (possible on Windows with unusual temp-dir names). Everywhere else this function
  maps I/O failures to `FunctionError::Compute(...)` with context; this one site
  manufactures a guaranteed-broken cargo invocation instead.
- Failure scenario: on such a host, every `compile_rust_source` call fails with a confusing
  cargo diagnostic about an empty `--manifest-path` (or writes artifacts to an empty
  target-dir path), and the operator-debuggable "temp path" root cause never appears in the
  error.
- Suggested fix: convert once: `let manifest = ...to_str().map_err(|_| FunctionError::Compute("temp path is not valid UTF-8".into()))?;` and reuse for both arguments.

### 6.8 — low — Compile child-process teardown is incomplete on every abnormal-exit path (no Job Object; kill-failure waits unbounded; error path never kills)
*(primary, merging three sibling-branch findings: [security-crypto] finding 4 (timeout kills `cargo` but not `rustc`/build-script grandchildren — no Job Object / process group), [error-handling-lifecycle] finding 8 (ignored `kill()` result + unbounded `child.wait()` after timeout), and [concurrency-lockfree] finding 6 (nit: the `Err` arm of `wait_timeout` never kills/reaps the child at all) — one root cause, three lenses)*
- File:line: `src/compile.rs:592-611` (timeout kill path + error path).
- Issue: on the `Ok(None)` (timed-out) branch, `child.kill()` terminates only the direct
  `cargo` process — cargo's `rustc` children (which evaluate guest `const fn`s, cf. the
  `sum_to(200_000)` heavy-const source in `tests/compile_tests.rs:235-250`, and run
  proc-macro expansion) are not in a Job Object / process group and survive as orphans,
  continuing to burn CPU/RAM, weakening the module doc's "cannot wedge the host
  indefinitely" claim. The `let _ = child.kill();` also discards failure
  (TerminateProcess/OpenProcess can fail on Windows), and the subsequent `child.wait()` has
  no deadline — if the kill fails, the caller blocks indefinitely, recreating on the host's
  own thread the wedge the CRIT-6 timeout exists to prevent. And on the `Err(e)` arm of
  `wait_timeout` the child is never killed at all (can keep writing into the `TempDir`,
  blocking its deletion on Windows).
- Failure scenario: a tenant submits a guest whose const-eval takes 30 minutes; after 120 s
  the wrapper reports "compilation timed out" but an orphaned `rustc` keeps a core pegged
  (repeated submissions multiply this). Or an unkillable child (AV interference, job-object
  weirdness) turns a bounded 120 s compile into a permanently stuck `create_function` call
  with no diagnostic.
- Suggested fix: spawn cargo inside a Windows Job Object with kill-on-close (or a POSIX
  process group and kill the group) so the timeout tears down the whole compiler tree; on
  the error path mirror the timeout arm (`let _ = child.kill(); let _ = child.wait();`);
  log if `kill()` errors and reap with a bounded secondary `wait_timeout` loop, returning
  `Compute("compilation timed out; kill failed: ...")` if the child still hasn't exited.

### 6.9 — *(same defect as 2.3)* — `rename`'s error-path rollback insert can itself fail silently
- (error-handling-lifecycle finding 9; full write-up at 2.3 under its primary lens.)

### 6.10 — *(same defect as 4.4)* — one leaked OS thread per `WasmEngine`; tests instantiate engines freely
- (error-handling-lifecycle finding 10, nit; full write-up at 4.4 under its primary lens.)

### 6.11 — nit — Forbidden-macro scanner fails open (`unwrap_or_default`) on an invariant break
- File:line: `src/compile.rs:250-252`.
- Issue: `String::from_utf8(out).unwrap_or_default()` would silently return an *empty*
  cleaned source on UTF-8 corruption, which `find_forbidden_macro_in_clean` then scans as
  clean — i.e. the one failure mode of this function makes the security scan pass. Today it
  is genuinely infallible (input is `&str`; only ASCII bytes are replaced), which is exactly
  why it should be an `expect` with the invariant named, not a fail-open default.
- Suggested fix: replace with
  `.expect("strip only writes ASCII spaces over non-newline bytes; input was &str")` so a
  future edit that breaks the invariant fails loudly, not permissively.

### 6.12 — *(same defect as 5.7)* — `FunctionMeta::from_record` silently coerces malformed catalogue fields
- (error-handling-lifecycle finding 12, nit; full write-up at 5.7 under its primary lens.)

## 7. style-claude-md

Section verdict (from the lens file): strongly conformant on the structural conventions —
every `mod.rs` (`lib.rs`, `src/wasm/mod.rs`, `src/tests/mod.rs`) is re-exports/manifest
only, tests follow the documented `tests/` layout exactly (topic-split files, manifest-only
`tests/mod.rs`, `#[cfg(test)] mod tests;` wired from the parent, zero inline
`#[cfg(test)]` blocks), every `scc::*::len()` allow carries the mandated `// O(N) ack:`
comment, no lock/Mutex/parking_lot or TODO/FIXME debris. Gaps: imports-at-top (7 sites),
one misleading doc on a security matcher, one copy-pasted doc block, plus borderline
one-file-one-export and manifest drift.

### 7.1 — medium — Mid-body `use` statements violate the "Imports at the top" rule (7 sites)
- File:line: `src/compile.rs:574, 582`; `src/tests/compile_tests.rs:111, 128, 140, 153, 169`.
- Issue: CLAUDE.md requires all `use` statements in the file header, with three narrow
  exceptions; none apply. `compile.rs:574/:582` — `use std::io::Read;` sits inside the two
  `std::thread::spawn` pipe-drain closures; the exception requires a top-level import
  *collision* plus a one-line collision comment — there is no other `Read` in scope and no
  comment. `compile_tests.rs` (5×) — `use crate::compile::test_find_forbidden_macro;` is
  re-declared inside five separate test functions; it is a function (not a trait), there is
  no collision, and the cfg-gating exception does not apply because the whole test file is
  already compiled only under `cfg(test)`.
- Failure scenario: none functional; each new test tends to copy the local-import pattern
  (already repeated 5×), so the violation propagates, and `fmt`/`clippy -D warnings` never
  catch it — only convention review does.
- Suggested fix: hoist `use std::io::Read;` into `compile.rs`'s header (next to
  `use std::fs;`) and a single `use crate::compile::test_find_forbidden_macro;` to the top
  of `compile_tests.rs`; delete the seven in-body imports.

### 7.2 — medium — `glob_matches` duplicated in `net_gateway.rs` under a doc comment falsely claiming reuse
*(primary; also flagged in [api-wire-protocol] finding 9 (low) and in the [correctness-tdd] nits bundle — one defect, three lenses)*
- File:line: `src/net_gateway.rs:483-514` (verified: the doc reads "reuses the same logic as
  `EnvPolicy`") vs `src/env_policy.rs:75-106`.
- Issue: `net_gateway.rs` carries a private `glob_matches` whose doc says it reuses
  `EnvPolicy`'s logic. It does not reuse anything: it is a body-for-body copy — and
  `env_policy::glob_matches` is `pub(crate)`, directly importable from this same crate.
  This is both a misleading comment and a real DRY break in security-relevant logic: the
  copy is the matcher behind the SSRF egress allowlist (`check_host_allowed`,
  `host_has_exact_match`), while the `env_policy` copy gates `env.*` secret seeding.
- Failure scenario: a matcher fix (e.g. the anchor rules for multi-`*` or non-`*`-terminated
  patterns — non-trivial logic, note the `i == 0` / `cursor != text.len()` conditions) lands
  in the `env_policy` copy, the one with direct unit tests (`env_policy_tests.rs`). The SSRF
  guard silently keeps the old behavior, and because the comment asserts the two are "the
  same logic", a maintainer has no reason to look for a second copy. Secret-grant masking
  and egress allowlisting then disagree on what a pattern matches.
- Suggested fix: delete `net_gateway.rs`'s private `glob_matches` and
  `use crate::env_policy::glob_matches;` instead; reword the doc comment to state the shared
  single implementation.

### 7.3 — low — Verbatim-duplicated doc-comment block on `host_call`
*(primary; also flagged in [api-wire-protocol] finding 13 (nit) and in the [correctness-tdd] nits bundle — one defect, three lenses)*
- File:line: `src/wasm/host_call.rs:16-27` (verified: lines 16-21 repeated verbatim at
  22-27, before the `# Borrow dance across await` section).
- Issue: a copy-paste remnant that rustdoc renders twice.
- Failure scenario: cosmetic only, but future edits to one copy will leave the stale twin in
  place (exactly how it likely arose).
- Suggested fix: delete the duplicated six lines.

### 7.4 — low — `net_gateway.rs` carries two primary concerns (one-file-one-export, borderline)
- File:line: `src/net_gateway.rs:24-61` (trait + DTOs) vs `:63-514` (SSRF guard).
- Issue: CLAUDE.md's "one file = one primary export" allows a *closely-coupled group*, but
  this 514-line file mixes the `NetGateway` trait and its wire DTOs (`HttpRequest`,
  `HttpResponse`, plus `ResolvedPin`) with ~350 lines of self-contained pure guard logic
  (`check_host_allowed`, `check_url_allowed*`, `parse_url`, `canonicalize_ip`,
  `parse_inet_aton`, `is_private_or_loopback_*`, `glob_matches`). The guard never touches
  the trait; it has its own dedicated test topic. Calibration note from the lens file:
  `context.rs` and `meta.rs` also define multiple public types but read as documented
  closely-coupled groups — no action needed there.
- Failure scenario: none directly; the cost is diff/blame granularity — a guard tweak and a
  DTO change land in the same file.
- Suggested fix: optionally split the egress guard into e.g. `src/net_guard.rs`, keeping
  `lib.rs`'s flat `pub use net_gateway::{...}` surface intact (re-export from both).

### 7.5 — low — Unused direct dependency `serde` in Cargo.toml
*(primary; also flagged in [api-wire-protocol] finding 14 (nit) — one defect, two lenses)*
- File:line: `Cargo.toml:14`.
- Issue: `serde = { version = "1.0.217", features = ["derive"] }` is declared but never used
  anywhere in `src/` (the only "serde" matches are two doc comments in `compile.rs`
  describing `shamir-sdk`'s own transitive deps). Dead manifest surface, with the `derive`
  feature pulling proc-macro weight into this crate's build for nothing. The absence of
  `serde_json`/`json!` anywhere also means the builder-only query-construction rule is
  satisfied by construction.
- Suggested fix: remove the dependency, or add a comment justifying it the way the other
  non-obvious deps (`wait-timeout`, `wasmparser`, `wat`) are justified.

### 7.6 — *(same defect as 6.3)* — security-bearing host imports tested only downstream
- (style-claude-md finding 6, low — the test-*locality* framing of 6.3: coverage lives only
  in `shamir-db/tests/functions_lifecycle.rs`, with SKIP paths on toolchain-less hosts.)

### 7.7 — nit — `wasm/mod.rs` doc list omits the sanitizer re-exports
- File:line: `src/wasm/mod.rs:3-5` vs `:18`.
- Issue: the module doc lists `WasmEngine`/`WasmLimits` and `WasmFunction`, but line 18 also
  re-exports `verify_wasm_module` and `SANCTIONED_HOST_IMPORTS` (the crate's security-ABI
  surface) — unlisted.
- Suggested fix: add a bullet: `* verify_wasm_module / SANCTIONED_HOST_IMPORTS —
  import-allowlist sanitizer (wasm_sanitizer).`

---

## Finding counts

Raw lens-tagged findings per lens file (each explicitly severity-tagged finding counts once;
the correctness-tdd "Nits" bundle is one severity-tagged item per the workspace counting
note — it unpacks into the five distinct nits distributed above):

| Lens | critical | high | medium | low | nit | total |
|---|---|---|---|---|---|---|
| correctness-tdd | 0 | 3 | 3 | 5 | 1 | 12 |
| concurrency-lockfree | 0 | 2 | 0 | 3 | 1 | 6 |
| security-crypto | 0 | 1 | 2 | 3 | 2 | 8 |
| performance-hotpath | 0 | 0 | 2 | 4 | 4 | 10 |
| api-wire-protocol | 0 | 1 | 6 | 5 | 2 | 14 |
| error-handling-lifecycle | 0 | 0 | 2 | 7 | 3 | 12 |
| style-claude-md | 0 | 2 | 0 | 4 | 1 | 7 |
| **total (lens-tagged)** | **0** | **7** | **17** | **31** | **14** | **69** |

Deduplicated distinct-defect census (same root-cause defect flagged in multiple lenses
counted once, under its primary lens; where lenses disagreed on severity the higher rating
is kept):

| Severity | Lens-tagged | Distinct defects | Distinct finding numbers |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 7 | 6 | 1.1 (+2.6), 1.2, 1.3, 2.1 (+1.7), 3.1, 5.1 |
| medium | 17 | 14 | 1.4 (+3.7), 3.2 (+1.9, 5.14), 3.3, 4.1, 4.2, 5.2, 5.3 (+6.4), 5.4, 5.5 (+1.10), 5.6, 6.1, 6.2 (+2.4, 3.8), 7.1, 7.2 (+5.10) |
| low | 31 | 19 | 1.5, 1.6, 2.2 (+1.8, 4.10), 2.3 (+6.9), 3.4, 4.3, 4.4 (+6.10), 4.5, 5.7 (+6.12), 5.8, 5.9, 6.3 (+1.11, 5.11, 7.6), 6.5, 6.6, 6.7, 6.8 (+3.6, 2.5), 7.3 (+5.12), 7.4, 7.5 (+5.13) |
| nit | 14 | 7 | 3.5, 4.6, 4.7, 4.8, 4.9, 6.11, 7.7 |
| **total** | **69** | **46** | 0 critical · 6 high · 14 medium · 19 low · 7 nit |

*(Counting note: the correctness-tdd nits bundle is 1 lens-tagged item but 5 distinct nit
defects — bullet 1 → 7.3, bullet 2 → 7.2, bullet 3 → 4.4, bullet 4 → 6.7, bullet 5 → 5.7 —
which is why the distinct census (46) is not simply lens-tagged minus dedup savings.)*

Dedup savings: 1.1/2.6 (fuel descent), 2.1/1.7 (blocking compile), 1.4/3.7 (seed_env),
3.2/1.9/5.14 (net_grants docs), 2.2/1.8/4.10 (remove-then-insert), 2.3/6.9 (rename
rollback), 6.2/2.4/3.8 (ticker `.ok()`), 4.4/6.10 (engine leak), 7.2/5.10 (glob_matches),
7.3/5.12 (host_call doc), 6.7/corr-nit (cargo args), 5.7/6.12 (from_record), 5.5/1.10
(decode_http), 6.3/1.11/5.11/7.6 (host-import tests), 6.8/3.6/2.5 (compile teardown),
5.3/6.4 (stringly errors), 7.5/5.13 (serde).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Make the aggregate fuel budget actually aggregate and cancellation-safe.** Reserve the
   grant at entry (`fetch_sub`/CAS clamped to remaining) and refund `grant − consumed` from a
   `Drop` guard created before the async block, so every exit path — including task
   cancellation — debits exactly once. Closes **1.1** and **6.1** (the guard is the same
   edit).
2. **Fix the fuel seed overflow and repair the two vacuous tests.** Seed
   `i64::MAX.min(limits.fuel)` (or `AtomicU64`); reshape the epoch test to in-range fuel +
   assert the wall-clock/epoch message; reshape the aggregate test to sequential sibling
   calls + assert the `"aggregate fuel budget exhausted"` message (and a depth ≪
   `depth_limit` termination). Closes **1.2**, **1.3** — Red first per CLAUDE.md TDD, since
   these are the tests that must be able to fail.
3. **Close the forbidden-macro scanner bypass.** Treat `name + optional-whitespace/comment +
   !` as the invocation shape in the cleaned text; add the adversarial regression tests
   (`env !"X"`, `env /*c*/!("X")`, `include_str \n !"f"`). Closes **3.1**.
4. **Take the blocking compile off the tokio workers.** Add
   `compile_rust_source_async`/`from_binary` wrappers via `tokio::task::spawn_blocking` and
   route the two `shamir-db` DDL call sites through them. Closes **2.1** (and its
   correctness-tdd framing).
5. **Switch the HTTP header wire shape to List-of-pairs** in both the host encoder and the
   `shamir-sdk` peer encoder (the request decoder already accepts the shape), keep Map as a
   documented legacy input, add a duplicate-header round-trip test. Closes **5.1**.

**P1 — soon**
6. **Complete the compile child-process teardown:** Windows Job Object with kill-on-close
   (or POSIX process group), kill on the `wait_timeout` error path, bounded secondary reap
   with a diagnostic when the kill fails. Closes **6.8** (incl. the 3.6 and 2.5 framings).
7. **Surface the epoch-ticker spawn failure:** `log::error!` at minimum; preferably fail
   `WasmEngine::new`. Closes **6.2**.
8. **Dedupe `glob_matches`** into one `pub(crate)` implementation used by both
   `env_policy` and `net_gateway`; fix the false-reuse comment. Closes **7.2**.
9. **Make the `net_grants` docs one truth** (fail-closed per #609) in `meta.rs`, cross-link
   `build_net_gateway`, add the empty-grants-traps test. Closes **3.2**.
10. **Harden `decode_http_request`:** reject absent `method`/`url`, wrong-shaped `headers`,
    and non-`Bin` bodies (or accept `Str` bodies explicitly, documented); add the
    malformed-input codec matrix. Closes **5.5**.
11. **Replace remove-then-insert with the scc entry API** in `replace`/`put`/`set`, and make
    `rename`'s rollback unable to lose the artifact (explicit error/`log::error!` at
    minimum). Closes **2.2**, **2.3** (and the perf framing 4.10).
12. **`seed_env` via `vars_os`** with skip/lossy conversion. Closes **1.4**.
13. **Fix the `FnCtx::global_get` gating contract** — enforce in `FnCtx` or correct the docs
    to the host-import-only scope. Closes **5.2**.
14. **Typed gateway errors:** small `thiserror` enums for `DbGateway`/`NetGateway`/guards,
    `Display` only at the trap boundary. Closes **5.3**.
15. **Add the host-import trap-path test layer** (`tests/host_import_tests.rs`,
    `host_http_wire_tests.rs`, `host_globals_tests.rs`, `params_tests.rs`, depth-limit trap
    test). Closes **6.3** (with 1.11/5.11/7.6).
16. **Cap or reclaim `GlobalVars`:** cardinality/byte quota with a trap on overflow, and/or
    a `global_remove` host import with per-function namespacing. Closes **4.1**.
17. **Add `0.0.0.0/8` (and consider CGNAT `100.64/10`, `198.18/15`) to
    `is_private_or_loopback_ip`** with non-canonical-encoding tests. Closes **3.3**.

**P2 — backlog**
18. **SDK path override** (`SHAMIR_SDK_PATH`-style env or parameter) + doc the dev-checkout
    default. Closes **5.6**.
19. **Adopt the envelope convention for `db_execute`** (structured `BatchResponse` error as
    payload instead of a trap) and document the per-import error contract in one place.
    Closes **5.4**.
20. **Bound the compile pipe buffers** (capped drain + truncated error string). Closes
    **4.3**.
21. **Reconcile the pooling allocator's memory cap** with per-invocation `WasmLimits`
    (derive from engine config or document the 64 MiB ceiling). Closes **1.5**.
22. **Fix the scanner's `'\''` desync** + regression test; replace the fail-open
    `unwrap_or_default` with a named-invariant `expect`. Closes **1.6**, **6.11**.
23. **Hot-path constant-factor cleanup:** drop the `.to_vec()` + deep params clone (4.2);
    `with_secret_grants_arc` (4.5); delete the dead `Arc` clones (4.6); cache
    `shamir_alloc`/`memory` handles (4.7); `impl Future` returns (4.8); `mem::take` in
    `maybe_wasm_opt` (4.9). Closes **4.2, 4.5–4.9**.
24. **Engine lifecycle:** `OnceLock`-shared engine or a `Drop`/shutdown signal for the
    ticker thread. Closes **4.4**.
25. **Catalogue decode observability:** `log::warn!` on fallbacks/dropped grant entries;
    consider `format_version`. Closes **5.7**.
26. **`ResolvedPin::pinned_ips` → `Option<Vec<IpAddr>>`** (pre-release type fix). Closes
    **5.8**.
27. **Typed wasmtime trap discrimination** in `map_wasm_error` (downcast to `Trap`/`TrapCode`,
    substring only as fallback) + assert the specific mapped message in tests. Closes
    **6.6**.
28. **Argon2id hardening:** concurrency `Semaphore` and/or layer-local cost clamps (3.4);
    branch `JoinError::is_panic()` instead of collapsing to `Cancelled` (6.5). Closes
    **3.4, 6.5**.
29. **Error-path hygiene:** typed error for non-UTF-8 temp paths (6.7); Argon2id panic
    payload (covered by 28).
30. **Structure/doc nits:** hoist the 7 mid-body imports (7.1); drop unused `serde` (7.5);
    split `net_guard.rs` (7.4); add the sanitizer re-export bullet to `wasm/mod.rs` (7.7);
    de-audit-track the public `net_gateway` docs (5.9); delete the duplicated `host_call`
    doc block (7.3); scrub env for `wasm-opt`/toolchain probes (3.5). Closes **7.1, 7.5,
    7.4, 7.7, 5.9, 7.3, 3.5**.

</details>
