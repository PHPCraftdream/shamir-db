<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-wasm-host — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The principal fuel-accounting defects and weak regression oracles remain. Smaller codec, lifecycle, documentation, and atomicity issues remain; several scenarios and proposed remedies need correction.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 12 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Aggregate fuel budget is enforced retroactively — in-flight chain can draw (depth+1) × fuel; doc overclaims the bound

Status: `confirmed-open`. Current risk: `high`.

Each Store reads the shared counter before execution and debits only after completion; suspended ancestors retain independent grants. No aggregate reservation or parent-to-child fuel transfer exists.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:433](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L433); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:587](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L587); [crates/shamir-wasm-host/src/wasm/host_call.rs:120](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L120).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `fuel > i64::MAX` immediately errors AND makes the wall-clock/epoch test vacuous

Status: `confirmed-open`. Current risk: `high`.

The unchecked u64-to-i64 seed becomes negative above i64::MAX. The epoch test still uses u64::MAX and accepts any error, so the entry rejection satisfies its oracle without executing the guest.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:436](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L436); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:246](../../../../../crates/shamir-wasm-host/src/tests/wasm_tests.rs#L246); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:258](../../../../../crates/shamir-wasm-host/src/tests/wasm_tests.rs#L258).

Grouping/duplicate: `SUMMARY.md#1.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `wasm_aggregate_fuel_exhausted_across_nested_calls` cannot distinguish aggregate from per-Store fuel — vacuous regression test

Status: `confirmed-open`. Current risk: `high`.

The fixture recursively calls itself without a successful base case, and the test only expects an error. Per-Store exhaustion, depth rejection, or epoch interruption also satisfy it; no depth or mechanism is observed.

Evidence: [crates/shamir-wasm-host/src/tests/wasm_tests.rs:120](../../../../../crates/shamir-wasm-host/src/tests/wasm_tests.rs#L120); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:295](../../../../../crates/shamir-wasm-host/src/tests/wasm_tests.rs#L295); [crates/shamir-wasm-host/src/tests/wasm_tests.rs:313](../../../../../crates/shamir-wasm-host/src/tests/wasm_tests.rs#L313).

Grouping/duplicate: `SUMMARY.md#1.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `compile_rust_source` is a blocking (up to 120 s) sync fn invoked on async paths — pillar 2 violation

Status: `confirmed-open`. Current risk: `high`.

Function and validator async creation directly execute synchronous subprocess waits and synchronous Wasmtime compilation. The 120-second limit covers only the cargo wait, not probes or wasm-opt.

Evidence: [crates/shamir-wasm-host/src/compile.rs:454](../../../../../crates/shamir-wasm-host/src/compile.rs#L454); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L172); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221](../../../../../crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L221).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Contradictory contract on empty `net_grants` inside meta.rs (stale permissive doc on a security default)

Status: `confirmed-open`. Current risk: `low`.

The options documentation still promises full allowlist inheritance, while FunctionMeta and build_net_gateway deny egress for an explicit empty grant list. This is documentation drift, not a current fail-open implementation.

Evidence: [crates/shamir-wasm-host/src/meta.rs:188](../../../../../crates/shamir-wasm-host/src/meta.rs#L188); [crates/shamir-wasm-host/src/meta.rs:83](../../../../../crates/shamir-wasm-host/src/meta.rs#L83); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:841](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L841).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `GlobalVars::seed_env` panics on any non-UTF-8 environment variable

Status: `confirmed-open`. Current risk: `medium`.

seed_env still iterates std::env::vars before policy filtering, and database initialization calls it directly. An invalid-Unicode entry can panic; process abort depends on panic strategy and handling.

Evidence: [crates/shamir-wasm-host/src/context.rs:211](../../../../../crates/shamir-wasm-host/src/context.rs#L211); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:168](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L168).

Grouping/duplicate: `SUMMARY.md#1.4`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Registry `replace`/`rename` are non-atomic remove-then-insert: transient `NotFound` and a silent name-theft race

Status: `confirmed-open`. Current risk: `low`.

replace, put, and set still remove before inserting and discard insertion errors. rename still discards a failed restoration. Readers can observe absence and racing registrations can prevent the intended artifact from being installed.

Evidence: [crates/shamir-wasm-host/src/registry.rs:52](../../../../../crates/shamir-wasm-host/src/registry.rs#L52); [crates/shamir-wasm-host/src/registry.rs:86](../../../../../crates/shamir-wasm-host/src/registry.rs#L86); [crates/shamir-wasm-host/src/context.rs:52](../../../../../crates/shamir-wasm-host/src/context.rs#L52); [crates/shamir-wasm-host/src/context.rs:166](../../../../../crates/shamir-wasm-host/src/context.rs#L166).

Grouping/duplicate: `SUMMARY.md#2.2`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `decode_http_request` silently defaults absent `method`/`url` to `""` and silently drops a non-`Bin` body

Status: `confirmed-open`. Current risk: `medium`.

Missing method/url become empty strings; non-Bin bodies become empty bytes. Wrong-shaped headers similarly become empty. Decoding can silently alter an otherwise accepted request.

Evidence: [crates/shamir-wasm-host/src/wasm/host_http.rs:26](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L26); [crates/shamir-wasm-host/src/wasm/host_http.rs:64](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L64); [crates/shamir-wasm-host/src/wasm/host_http.rs:67](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L67).

Grouping/duplicate: `SUMMARY.md#5.5`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits

Status: `confirmed-open`. Current risk: `low`.

Pooling is constructed with the default 64-MiB memory ceiling, while public per-function limits can request more. On-demand construction does not impose that same pool ceiling; no custom-limit validation reconciles them.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:190](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L190); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:458](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L458); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:217](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L217).

Grouping/duplicate: `SUMMARY.md#1.5`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Forbidden-macro scanner desyncs on the '\'' char literal

Status: `confirmed-open`. Current risk: `low`.

The escaped-character branch stops at the escaped quote rather than the literal's closing quote. Scanner lexing remains incorrect; a valid compilable security bypass attributable to this particular defect is unverified.

Evidence: [crates/shamir-wasm-host/src/compile.rs:334](../../../../../crates/shamir-wasm-host/src/compile.rs#L334); [crates/shamir-wasm-host/src/compile.rs:234](../../../../../crates/shamir-wasm-host/src/compile.rs#L234); [crates/shamir-wasm-host/src/tests/compile_tests.rs:168](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L168).

Grouping/duplicate: `SUMMARY.md#1.6`. This row is not another independent defect.

<a id="review-11"></a>

### Claim 11 — TDD-coverage gaps: security-bearing host imports and `Params` have no tests in this crate

Status: `confirmed-open`. Current risk: `low`.

All ten test topics are wired, but no guest invokes batch/global/db/http imports and no dedicated Params boundary matrix exists. Argon2 tests exercise some Params accessors indirectly; compile tests can skip and doctests remain disabled.

Evidence: [crates/shamir-wasm-host/src/tests/mod.rs:1](../../../../../crates/shamir-wasm-host/src/tests/mod.rs#L1); [crates/shamir-wasm-host/src/tests/argon2id_tests.rs:73](../../../../../crates/shamir-wasm-host/src/tests/argon2id_tests.rs#L73); [crates/shamir-wasm-host/src/tests/compile_tests.rs:22](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L22); [crates/shamir-wasm-host/Cargo.toml:46](../../../../../crates/shamir-wasm-host/Cargo.toml#L46).

Grouping/duplicate: `SUMMARY.md#6.3`. This row is not another independent defect.

<a id="review-12"></a>

### Claim 12 — Nits

Status: `confirmed-open`. Current risk: `nit`.

All five bundled patterns remain: duplicated call documentation, duplicated glob matcher, never-ending ticker, empty-string path fallback, and silently discarded malformed grant entries. Their impact differs and should not be counted as five runtime security defects.

Evidence: [crates/shamir-wasm-host/src/wasm/host_call.rs:22](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L22); [crates/shamir-wasm-host/src/net_gateway.rs:487](../../../../../crates/shamir-wasm-host/src/net_gateway.rs#L487); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:158](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L158); [crates/shamir-wasm-host/src/compile.rs:542](../../../../../crates/shamir-wasm-host/src/compile.rs#L542); [crates/shamir-wasm-host/src/meta.rs:129](../../../../../crates/shamir-wasm-host/src/meta.rs#L129).

## Corrections and qualified non-findings

- The fuel ceiling is a structural upper-bound concern, not a measured 33-billion-instruction reproduction. A guest must leave fuel for recursion; exact WAT fuel costs were not established.
- An actual yielding nested call remains inside the depth-zero timeout; merely rearming a child epoch deadline does not prove that a mostly-awaiting chain escapes that timeout.
- Blindly reserving the entire grant at entry would consume all shared headroom in the parent and reject legitimate nested calls. Remediation needs reentrant parent/child accounting, not just a full-grant CAS.
- Concurrent calls sharing an ordinary fresh FnCtx do not automatically share fuel: call creates a local budget without updating FnCtx. Native nested callees can nevertheless clone an already-budgeted child context.
- seed_env panics are not unconditionally process aborts.
- No exact 16k fuel count is needed to establish the recursive test's invalid oracle.
- Malformed grant entries are removed, not widened into additional grants.
- The compile timeout test also permits successful completion, so it is not deterministic teardown coverage.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wasm-host -- Correctness & TDD-coverage

## Summary

The crate is generally well-tested (10 topic-split test files under `src/tests/`, real
revert-proof narratives, a genuine sanitizer/linker cross-check), but two of its three
headline resource-limit tests are **vacuous** — they pass for reasons unrelated to what
they claim to pin, so the epoch-interruption bound and the aggregate cross-Store fuel
budget (#612) have no test that can actually fail on regression. Beneath that, the
aggregate fuel budget itself is only enforced retroactively (debit-on-exit), so the
documented "aggregate instruction count is bounded" invariant does not hold for an
in-flight call chain, and `fuel > i64::MAX` breaks the entry gate outright. Secondary
findings: a blocking 120 s compile exported as a plain sync fn and invoked on async
paths, a contradictory security-relevant doc contract on empty `net_grants`, a
`std::env::vars()` panic path in `seed_env`, and remove-then-insert non-atomicity in the
registry/context stores.

## Findings

### 1. Aggregate fuel budget is enforced retroactively — in-flight chain can draw (depth+1) × fuel; doc overclaims the bound
- File: `crates/shamir-wasm-host/src/wasm/wasm_function.rs:433-447` (grant), `:583-587` (debit), module doc `:58-68`
- Severity: high
- Issue: The shared `Arc<AtomicI64>` budget is read at **entry** (`remaining <= 0` gate + `grant = min(remaining, limits.fuel)`) but debited only **after a call exits** (`fetch_sub` at line 587). In a nested `ctx.call` chain, no level exits before the deepest level is reached, so every level's entry check sees the full budget and grants a full `limits.fuel`. The module doc claims "a guest that recurses via `ctx.call` can no longer execute N × `limits.fuel` instructions across N nested Stores", but worst-case total is `(depth_limit + 1) × fuel` (default: 33 × 1e9) before any debit lands; the gate trips only in arrears (and `fetch_sub` happily drives the counter negative). Concurrency widens it further: two invocations sharing one `FnCtx` both read the same `remaining` before either debits.
- Failure scenario: A guest that burns its full grant at each level before recursing executes ~33 billion instructions (default limits) instead of the documented 1-billion aggregate ceiling; the top-level 30 s wall-clock is the only remaining bound, and a mostly-awaiting guest stretches even that per-level via epoch resets.
- Suggested fix: Reserve the grant atomically at entry (`fetch_sub(grant)` up front, refund `grant - consumed` on exit) or hold a reservation list; alternatively document the true bound (in-flight-depth × fuel) and enforce the strict bound at depth 0 with `fetch_update` CAS.

### 2. `fuel > i64::MAX` immediately errors AND makes the wall-clock/epoch test vacuous
- File: `crates/shamir-wasm-host/src/wasm/wasm_function.rs:436,438-443`; `crates/shamir-wasm-host/src/tests/wasm_tests.rs:239-268`
- Severity: high
- Issue: The lazily-seeded budget is `AtomicI64::new(limits.fuel as i64)` (line 436). `WasmLimits.fuel` is `u64`, so any value above `i64::MAX` — e.g. `u64::MAX`, the natural "effectively unlimited" setting — wraps to `-1`, and the very next `remaining <= 0` gate returns `Compute("aggregate fuel budget exhausted across nested calls")` on the first call, even at depth 0 with zero work done. `wasm_wall_clock_deadline_interrupts_cpu_bound_guest` uses exactly `fuel: u64::MAX` (test line 246), so its `assert!(result.is_err())` and `elapsed < 10s` are satisfied **microseconds in, by this early-return** — it never reaches `shamir_call`, never exercises the 300 ms deadline or the epoch deadline, and would stay green if epoch interruption were deleted entirely.
- Failure scenario: (a) an operator configuring `fuel: u64::MAX` gets every function invocation rejected with a misleading "budget exhausted" error; (b) the one dedicated test for the epoch-interruption pre-emption mechanism (added as a security finding fix) cannot fail on regression — CLAUDE.md's Red/Green discipline is violated in the exact place it matters most.
- Suggested fix: Seed with `i64::MAX.min(limits.fuel)` (or switch the counter to `AtomicU64`/`AtomicI128`), and fix the test to use a large-but-in-range fuel (e.g. `i64::MAX as u64`) plus assert the error message is the epoch/deadline variant (`"wall-clock"`) — not merely `is_err()`.

### 3. `wasm_aggregate_fuel_exhausted_across_nested_calls` cannot distinguish aggregate from per-Store fuel — vacuous regression test
- File: `crates/shamir-wasm-host/src/tests/wasm_tests.rs:270-317` (with `wasm_function.rs` grant/debit mechanics)
- Severity: high
- Issue: The test descends a linear `ctx.call` recursion and asserts any error before `depth_limit=1000` proves the shared budget. Mechanically it cannot: (a) debits land only when a level exits, so during descent **every** level's entry check sees the full 10,000 budget and gets a full grant — the shared counter can never stop a pure descent (finding 1); the recursion ends at the depth-limit trap, which fires identically under the old reset-per-Store behavior; (b) if instead the WAT's per-level burn exceeds the grant — the busy loop is ~8 wasm ops × 2000 iterations ≈ 16k fuel vs a 10,000 grant — level 0 traps on its **first** loop before ever calling `shamir_host.call`, again indistinguishable from plain per-Store fuel. Either branch makes `expect_err` pass with or without the #612 fix, so the elaborate "under the OLD behavior this test would NOT catch a regression" comment describes a failure mode the test never actually sets up (no sequential *sibling* calls, the only shape the arrears-budget can catch).
- Failure scenario: A regression that silently reverts to per-Store fuel resets keeps this test green — precisely the regression it was written to catch.
- Suggested fix: Reshape the guest to make **sequential sibling** nested calls in a loop from one level (each sibling exits and debits before the next is granted), size `fuel` so sibling k+1's entry gate trips after ~N siblings, and assert the specific `"aggregate fuel budget exhausted"` message. (Depends on fixing finding 1's reservation semantics for the chain case too.)

### 4. `compile_rust_source` is a blocking (up to 120 s) sync fn invoked on async paths — pillar 2 violation
- File: `crates/shamir-wasm-host/src/compile.rs:454-462` (sync export); call site `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172` (direct call between two `.await`s)
- Severity: medium
- Issue: CLAUDE.md pillar 2: "CPU-bound work crosses to `tokio::task::spawn_blocking`". The crate honors this for Argon2id (`builtin.rs:63`) but exports the guest compiler as a plain `pub fn` that spawns cargo, blocks on `wait_timeout`, and does blocking fs I/O — with no async wrapper and no doc telling callers to offload. The only production caller invokes it inline in an async fn.
- Failure scenario: `CREATE FUNCTION ... AS SOURCE` pins a tokio worker for the whole cargo build (tens of seconds typical, 120 s worst case); several concurrent DDLs can starve the runtime's worker pool and stall unrelated requests.
- Suggested fix: Add `pub async fn compile_rust_source_async(source: &str)` in this crate that wraps the sync body in `tokio::task::spawn_blocking` (mirroring `Argon2idFunction`), and route the shamir-db call sites through it.

### 5. Contradictory contract on empty `net_grants` inside meta.rs (stale permissive doc on a security default)
- File: `crates/shamir-wasm-host/src/meta.rs:185-190` (`CreateFunctionOptions`: "empty `net_grants` = full DB-wide `net_allowlist`") vs `:79-92` (`FunctionMeta::net_grants`: "an EMPTY/absent `net_grants` means NO egress")
- Severity: medium
- Issue: The two docs in the same file state opposite semantics for the same field. The enforcement site (`shamir-db/src/shamir_db/shamir_db/core.rs:805-834`, `build_net_gateway`) implements the restrictive reading ("Task #609: empty `net_grants` now means NO egress"), so `CreateFunctionOptions`' doc — whose `Default` has `net_grants: Vec::new()` — is the stale one, falsely promising default-created functions inherit the DB-wide allowlist.
- Failure scenario: A future change authored against the stale doc "restores" the permissive default and silently re-opens egress for every default-created function; or a reviewer trusts the wrong half and mis-audits egress.
- Suggested fix: Rewrite the `CreateFunctionOptions` doc to match #609 (empty = no egress; non-empty narrows the DB-wide allowlist) and cross-link `build_net_gateway`.

### 6. `GlobalVars::seed_env` panics on any non-UTF-8 environment variable
- File: `crates/shamir-wasm-host/src/context.rs:210-216`
- Severity: medium
- Issue: `std::env::vars()` panics if any env var's name or value is not valid Unicode (documented std behavior). `seed_env` iterates it unconditionally.
- Failure scenario: One non-UTF-8 env var in the host process (set by any external software; routine on Unix) turns every seeding call into a process abort — violating the crate's own "avoid `panic!`" error-handling rule — and it fails deterministically at every startup until the var is removed.
- Suggested fix: Iterate `std::env::vars_os()` and skip (or lossy-convert) non-UTF-8 entries, e.g. `filter_map(|(k, v)| k.into_string().ok().zip(v.into_string().ok()))`.

### 7. Registry `replace`/`rename` are non-atomic remove-then-insert: transient `NotFound` and a silent name-theft race
- File: `crates/shamir-wasm-host/src/registry.rs:50-54` (`replace`), `:73-89` (`rename` recovery); same shape in `context.rs:50-54` (`BatchContext::put`) and `:164-168` (`GlobalVars::set`)
- Severity: low
- Issue: `replace` removes the key, then inserts. In the window a concurrent `invoke`/`get` returns `NotFound` despite the doc's "new invocations pick up the new artifact", and a racing `register` can claim the name — the subsequent `insert_sync` then fails and the new artifact is silently dropped (`let _ =`). `rename`'s race-recovery re-insert (`registry.rs:86`) can equally fail silently if the old name was re-registered in the interim, dropping `f` from the registry entirely. `put`/`set` briefly make an existing value invisible to readers.
- Failure scenario: Under concurrent `replace`/`register`/`invoke` a caller observes "function not found" or gets the racer's function instead of the replaced artifact; the loss is silent (no error surfaces).
- Suggested fix: Use scc's entry API (`entry_sync` Occupied → `get_mut`) or `scc::HashMap::reallocate`/compute-on-insert so replace is a single atomic operation; `BatchContext`'s own `update()` already demonstrates the entry pattern — reuse it.

### 8. `decode_http_request` silently defaults absent `method`/`url` to `""` and silently drops a non-`Bin` body
- File: `crates/shamir-wasm-host/src/wasm/host_http.rs:26-34` (`get_str` maps absent → `Ok(String::default())`), `:67-70` (body: any non-`Bin` → `Vec::new()`)
- Severity: low
- Issue: A guest request `{"body": "hello"}` (a `Str` — the natural first attempt) loses its payload with no error; a request missing `method`/`url` proceeds to the gateway with empty strings and fails later with a confusing allowlist/URL error instead of "missing field".
- Failure scenario: Guest authors lose request bodies or chase misleading gateway errors; data loss is invisible at the host-import boundary.
- Suggested fix: Treat absent `method`/`url` as `BadParam`-style errors (or at least trap with a named-field message), and accept `Str` bodies via UTF-8 bytes instead of dropping them.

### 9. Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits
- File: `crates/shamir-wasm-host/src/wasm_engine.rs:96-98,190` vs per-invocation `limits.max_memory_bytes` in `wasm_function.rs:457-459`
- Severity: low
- Issue: `pool.max_memory_size(WasmLimits::default().max_memory_bytes)` (64 MiB) is fixed at engine construction, while each `WasmFunction` may carry custom `WasmLimits` with a larger memory cap. Under the (default) pooling allocator, growth beyond 64 MiB fails at the pool ceiling regardless of the configured per-Store limit; the on-demand fallback (`SHAMIR_WASM_NO_POOL=1`) honors it — two allocators disagree on the same config.
- Failure scenario: A function configured with `max_memory_bytes = 256 MiB` works with pooling disabled and traps with "memory ... exceeded" when pooling is on — an env-var-dependent behavior change that looks like a guest bug.
- Suggested fix: Either clamp/document that pooling caps memory at the 64 MiB default, or derive the pool's `max_memory_size` from the maximum limit the engine is intended to serve (engine-level config, not `default()`).

### 10. Forbidden-macro scanner desyncs on the `'\''` char literal
- File: `crates/shamir-wasm-host/src/compile.rs:327-346` (`find_char_literal_end` returns the *escaped* quote's index for `'\''`, leaving the real closing quote unblanked), `:229-244`
- Severity: low
- Issue: For source containing `'\''`, the escape branch stops at the escaped quote instead of the closing one, blanking `'\'` and leaving a stray `'` in the cleaned text. The scanner and rustc then disagree on lexing by one quote; subsequent code can be swallowed into a phantom literal (false negative) or scanned as code that rustc treats as a literal. The module doc disclaims completeness ("defense-in-depth check, not a sandbox") and no working bypass was demonstrated (crafted desyncs generally fail cargo's parse), so this is a correctness wart in a control, not an open hole.
- Failure scenario: A guest source whose scanner-cleaned text differs lexically from what rustc compiles could in principle hide a forbidden invocation from the scan while remaining valid Rust.
- Suggested fix: In the escape branch, skip the escaped character and continue scanning for the closing quote (`if b[j+1] == b'\'' { return Some(j+1) }` after the escape char); add a `'\''`-containing test to `compile_tests.rs` asserting a following `env!` is still detected.

### 11. TDD-coverage gaps: security-bearing host imports and `Params` have no tests in this crate
- File: `crates/shamir-wasm-host/src/wasm/host_globals.rs` (env.* write-protection, grant-gated reads), `src/wasm/host_batch.rs`, `src/wasm/host_db.rs`, `src/wasm/host_http.rs`, `src/params.rs`; `Cargo.toml:46` (`doctest = false`)
- Severity: low
- Issue: The sanitizer/linker surface is excellently cross-checked, but no test anywhere in this crate *invokes* `batch_put`/`batch_get`, `global_set`/`global_get` (including the env.* write-protection at `host_globals.rs:46-50` and the fail-closed grant gate at `:79-83` — both security controls with zero Red/Green coverage here), `db_*`, or `http_fetch` (the `decode_http_request`/`encode_http_response` helpers of finding 8 are untested). `Params` (`bytes`/`str`/`u32`/`opt_u32` boundaries) has no tests file at all; `EnvPolicy`'s doc example is dead since doctests are disabled crate-wide. Compile happy-path tests silently `SKIP` on `ToolchainUnavailable`, so a CI host missing the wasm target loses the entire compile pipeline silently (the tiny-timeout test also accepts `Ok` as a documented skip).
- Failure scenario: A regression in env.* write-protection or grant gating (one-character change) passes the whole gate; boundary bugs in `Params::u32`/`opt_u32` are invisible.
- Suggested fix: Add WAT-based guest modules (the pattern already exists in `nested_actor_tests.rs`/`wasm_tests.rs`) that call `global_set("env.X", ...)`, `global_get` with/without grants, and `batch_put`/`batch_get`; add `params_tests.rs`; optionally assert `SANCTIONED`-style behavior for decode helpers.

### 12. Nits
- File: multiple
- Severity: nit
- Issue:
  - `host_call.rs:16-27` — the doc-comment block is duplicated verbatim.
  - `glob_matches` is duplicated in `env_policy.rs:75-106` and `net_gateway.rs:487-514` — drift risk for a matcher both security filters rely on (CLAUDE.md: one file = one primary export; prefer a shared helper).
  - `wasm_engine.rs:154-163` — a never-joined epoch-ticker OS thread is leaked per `WasmEngine::new()`; harmless in production (singleton) but each of the ~10 tests spawns one, and nothing stops repeated engine creation.
  - `compile.rs:542,544` — `.to_str().unwrap_or("")` silently passes empty `--manifest-path`/`--target-dir` args on non-UTF-8 temp paths, producing a confusing cargo failure; map to a `Compute` error instead.
  - `meta.rs:124-141` — `from_record` silently drops non-`Str` entries inside `secret_grants`/`net_grants` arrays (`filter_map`), which could quietly widen or narrow grants on malformed catalogue records rather than failing the load.

</details>
