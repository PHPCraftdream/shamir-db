<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk-macros — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Validator deep copies and retained ownership are source-proven. Cross-call leakage is not a current production defect, and numerical cost/latency claims are unmeasured.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 3 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`

Status: `confirmed-open`. Current risk: `medium`.

Validator still clones owned recursive Value data from borrowed getters and keeps Params in the enclosing scope through block_on. There is no take/move-out API or early drop. Additional ownership and allocations are structural; a universal 3x byte multiplier is not proven.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:126](../../../../../crates/shamir-sdk-macros/src/lib.rs#L126); [crates/shamir-sdk-macros/src/lib.rs:130](../../../../../crates/shamir-sdk-macros/src/lib.rs#L130); [crates/shamir-sdk-macros/src/lib.rs:137](../../../../../crates/shamir-sdk-macros/src/lib.rs#L137); [crates/shamir-sdk-macros/src/lib.rs:144](../../../../../crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-sdk/src/params.rs:26](../../../../../crates/shamir-sdk/src/params.rs#L26); [crates/shamir-sdk/src/value.rs:26](../../../../../crates/shamir-sdk/src/value.rs#L26).

<a id="review-2"></a>

### Claim 2 — `shamir_alloc` leaks every allocation; the "module is short-lived" contract is documented only here and enforced nowhere in this crate

Status: `not-applicable`. Current risk: —.

Intentional within-invocation leaks remain, but every production call creates a fresh Store and instance, reclaiming guest memory on return. SDK host-import documentation also explicitly names Store destruction. Future instance reuse is a conditional design risk, not a present cross-call leak.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:112](../../../../../crates/shamir-sdk-macros/src/lib.rs#L112); [crates/shamir-sdk/src/__rt.rs:28](../../../../../crates/shamir-sdk/src/__rt.rs#L28); [crates/shamir-sdk/src/host_imports.rs:60](../../../../../crates/shamir-sdk/src/host_imports.rs#L60); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:474](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L474); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:498](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L498); [crates/shamir-wasm-host/src/wasm/host_call.rs:149](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L149).

<a id="review-3"></a>

### Claim 3 — Generated `shamir_call` busy-spins at 100% CPU if the author's future ever yields `Pending`

Status: `confirmed-open`. Current risk: `medium`.

A persistently-Pending guest future is repeatedly polled without yielding or a guest poll limit. Finite Pending does not imply exhausting the host budget. Async host imports already use fiber suspension; the claimed unavoidable I/O-induced spin is false.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](../../../../../crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:477](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L477); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:487](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L487).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `shamir_alloc` zero-fills O(len) bytes the host immediately overwrites, and does not reject negative `len`

Status: `confirmed-open`. Current risk: `nit`.

Allocator semantics still request initialized len bytes, and the host overwrites that range. The negative-length guard is absent. Whether allocation emits a separate linear memset and whether eliminating it helps latency are unmeasured.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](../../../../../crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:540](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L540).

Grouping/duplicate: `security-crypto.md#2`. This row is not another independent defect.

## Corrections and qualified non-findings

- Drop exact 3x/1x memory claims. Encoded bytes, decoded owned structures, and clones have different sizes, and the clone phase temporarily overlaps even with a later drop(params).
- Pooling reuses allocation slots, not this call's live guest Store/heap. Fresh Store creation is the positive lifetime mechanism.
- The lifetime contract is not documented only in the macro: crates/shamir-sdk/src/host_imports.rs:60 names Store destruction.
- Guest-local spin can occupy a runtime worker until fuel/epoch interruption, but no measured CPU percentage or latency regression is available.
- No macro-implementation runtime hot path exists. Parsing/string operations do scale with authored token input, so the literal claim of no unbounded-input work is too strong; no practical compile-time performance defect was established.
- An uninitialized-buffer optimization must not create initialized u8 slices before initialization; benefit and safe representation need validation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros -- Performance & O(x->0)

## Summary

The macro-implementation code itself runs once per annotated function at compile
time (no hot path, no findings there), so this review judges the **generated
guest ABI code** (`shamir_alloc` / `shamir_call`), which executes once per
record validation / function call. The one substantive issue is in the
`#[validator]`-generated `shamir_call`: it deep-copies `record` and
`old_record` out of the decoded `Params` map and then retains the entire
decoded map across the whole `block_on(...)` await, putting ~3x the payload
simultaneously live on the per-record hot path. The per-call leak design of
`shamir_alloc`/`leak_result` is documented here and *currently* honored by the
host (fresh `instantiate_async` per call + wasmtime pooling, verified in
`shamir-wasm-host/src/wasm/wasm_function.rs:495-500`), but nothing in this
crate enforces that contract. Test coverage gap relevant to this theme: this
crate has no `tests/` directory, and no in-repo test executes the
validator/function-generated `shamir_call` at all (host tests use hand-written
WAT; `shamir-sdk/tests/{scalar,procedure}_compile_pass.rs` are compile-pass
only, 2 of 4 macros) -- so none of the behavior below is pinned by a test.

## Findings

### 1. `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`
- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:126-144` (generated code; same pattern per macro, but only `validator` does the extraction)
- **Severity:** medium
- **Issue:** The generated `shamir_call` does `params.get("record")` ->
  `v.clone()` and `params.get("old_record")` -> `Some(v.clone())`
  (`lib.rs:129-139`). `Params::get` returns `&Value`
  (`shamir-sdk/src/params.rs:26`), so each `.clone()` is a full deep copy of
  the value, while the original stays inside `params`. Because `params` is an
  owned binding and Rust drops owned values at scope end (not at last use),
  the entire decoded payload (`Params { map: Vec<(String, Value)> }`,
  decoded by `__rt::decode_params`) stays live across
  `__rt::block_on(#inner_name(record, old_record, ctx))` -- the whole author
  validation. Peak live memory per invocation is: raw input bytes (leaked in
  guest linear memory) + full decoded `params` map + the two deep clones.
  `#[validator]` never passes `params` to the author, so the retained map is
  pure dead weight; this is exactly pillar-3's "hidden allocation/retention in
  the hot path" (validators run once per record insert/update).
- **Failure scenario:** Large user records (the payload size is
  attacker/user-controlled) x concurrent validations multiply guest linear
  memory ~3x per call; with a high validation fan-out this wastes memory.grow
  bandwidth and pool capacity and can trip the host's `StoreLimits` memory cap
  earlier than necessary.
- **Suggested fix:** In the generated code, `drop(params)` immediately after
  the `old_record` extraction (safe: `validator` doesn't forward `params`), and
  avoid the deep copies entirely by moving out of the map -- e.g. add a
  `Params::take(&mut self, key) -> Option<Value>` in `shamir-sdk`
  (`Vec::swap_remove` by position) and have the macro emit
  `params.take("record")` / `params.take("old_record")`. That makes peak
  footprint ~1x payload (the decoded map) instead of ~3x.

### 2. `shamir_alloc` leaks every allocation; the "module is short-lived" contract is documented only here and enforced nowhere in this crate
- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:105-114` (and duplicated at `235-244`, `366-375`, `533-542`), output-buffer counterpart `shamir-sdk/src/__rt.rs:25-30`
- **Severity:** low
- **Issue:** Every `shamir_alloc(len)` call `mem::forget`s its `Vec<u8>`; there
  is no `shamir_free` in the ABI. The doc comment justifies this with "the WASM
  module is short-lived" -- an assumption about the *host*, which this crate
  cannot enforce. I verified it currently holds: the host instantiates a fresh
  instance per call (`wasm_function.rs:495-500`) under wasmtime's pooling
  strategy, so leaked bytes die with the instance. But the contract is one
  host-side refactor away from breaking: any future path that keeps a guest
  instance warm across calls makes growth O(total bytes ever allocated) --
  unbounded per pillar 3. Note also the within-call case: the host's
  nested-call path (`shamir-wasm-host/src/wasm/host_call.rs:142`) allocates a
  result buffer via `shamir_alloc` mid-guest-call, so one guest call that fans
  out N nested host calls leaks N buffers even under fresh-instance-per-call
  (bounded only by the store's memory limit).
- **Failure scenario:** Host adopts instance reuse/pooling-at-the-Rust-level
  for throughput; validators called once per record then grow linear memory
  without bound until `memory.grow` fails and every call traps.
- **Suggested fix:** Either emit a `shamir_free(ptr: i32, len: i32)` export and
  document it as required for long-lived guests, or add a module doc /
  debug-build high-water assertion that names the host contract explicitly so a
  host-side change to instance lifetime forces a revisit. At minimum, carry the
  "fresh instance per call is load-bearing" note into `shamir-wasm-host`'s call
  path docs (cross-crate invariant).

### 3. Generated `shamir_call` busy-spins at 100% CPU if the author's future ever yields `Pending`
- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:144, 264, 391, 556` (generated `__rt::block_on` calls); mechanism in `shamir-sdk/src/__rt.rs:50-59`
- **Severity:** low
- **Issue:** All four macros drive the author's `async fn` via
  `__rt::block_on`, whose `Poll::Pending` branch is a `core::hint::spin_loop()`
  tight loop (the `__rt` comment acknowledges this: "For now, a tight loop is
  correct"). Today all SDK futures are Ready-on-first-poll, so it never
  triggers -- but the moment slice 4 adds genuinely async host imports
  (procedures take `Ctx` with db access; the `#[procedure]` doc example itself
  calls `ctx.db().table("users").query(None)`), any yielding future turns
  `shamir_call` into a hot spin inside a tokio worker thread, starving every
  other task on that worker. The host's fuel + epoch deadline + wall-clock
  timeout bounds the burn to a trap (verified:
  `wasm_function.rs:480-487`, `549-559`), so this degrades to
  worst-case-latency-plus-trap rather than an infinite hang -- but it burns the
  entire budget every time.
- **Failure scenario:** A procedure awaits a real async host import; the guest
  spins, pinning the executor thread at 100% until the epoch deadline traps it;
  latency collapses to the deadline and unrelated tasks on the same worker
  starve in the interim.
- **Suggested fix:** When async host imports land, replace the spin with a
  proper waker that re-polls only after the host import resolves (or route the
  pending case to `unreachable!` with a clear message so the failure is loud).
  Until then, the macro's doc should state the "futures must not yield
  Pending" constraint that the generated code silently imposes on authors.

### 4. `shamir_alloc` zero-fills O(len) bytes the host immediately overwrites, and does not reject negative `len`
- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:109-114` (and `239-244`, `370-375`, `537-542`)
- **Severity:** nit
- **Issue:** `vec![0u8; len as usize]` performs an O(len) memset per call; the
  host writes the full input buffer right after allocating
  (`wasm_function.rs:540` `copy_from_slice`), so the zeroing is redundant work
  on the per-record path (only matters for large payloads). Additionally,
  `len as usize` on a negative `i32` wraps to ~4 GiB, so a malformed length
  drives a near-4-GiB allocation attempt instead of a clean error (the host's
  `StoreLimits` memory cap converts it to a trap; the host is trusted
  in-repo, so this is robustness polish, not a security hole).
- **Failure scenario:** Large payload per record -> measurable wasted memset
  bandwidth per validation; a buggy/compromised host path passing negative
  `len` -> opaque allocation-failure trap rather than a diagnosable error.
- **Suggested fix:** Guard `len` (`debug_assert!(len >= 0)` or return -1 on
  negative) and, if payload sizes justify it, allocate uninitialized via
  `alloc` + `from_raw_parts_mut` with the "host overwrites the full buffer"
  contract documented at the ABI boundary; otherwise leave as-is and accept
  the memset.

No findings for this theme in the macro-implementation code itself: it runs
once per annotated function per build (compile-time), has no loops over
unbounded input (argument iteration is capped at the fixed 1-3-arity
validation), and its string `replace`/`to_string` work in the return-type
checks is negligible at that call frequency.

</details>
