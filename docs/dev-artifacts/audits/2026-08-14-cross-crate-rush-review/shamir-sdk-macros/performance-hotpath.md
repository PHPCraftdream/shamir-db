<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk-macros — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Validator cloning and retained ownership are structural costs. Fresh Store teardown prevents current cross-call leakage. Zero-initialization optimization and numerical resource claims remain unmeasured.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 3 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Borrowed getters clone recursive owned Values while the original Params remains in scope through validation. Large strings/lists/maps therefore incur redundant ownership and allocation. Neither exact peak RSS nor a universal threefold multiplier follows from source.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L126); [crates/shamir-sdk-macros/src/lib.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L130); [crates/shamir-sdk-macros/src/lib.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L137); [crates/shamir-sdk/src/params.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L26); [crates/shamir-sdk/src/value.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L26).

<a id="review-2"></a>

### Claim 2 — `shamir_alloc` leaks every allocation; the "module is short-lived" contract is documented only here and enforced nowhere in this crate

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Fresh Store/instance creation and destruction reclaim invocation memory. Published wasmtime 46.0.2 src/runtime/store.rs:2565/2578 confirms instance deallocation: https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/store.rs. Pooling slots are not live guest reuse. Within-call allocations remain intentionally memory-limited.

Evidence: [crates/shamir-sdk/src/host_imports.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L55); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:474](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L474); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:498](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L498); [Cargo.lock:4740](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4740).

<a id="review-3"></a>

### Claim 3 — Generated `shamir_call` busy-spins at 100% CPU if the author's future ever yields `Pending`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Persistent guest-local Pending repeatedly polls without parking. Finite Pending and host-fiber suspension are counter-evidence to the blanket every-Pending/entire-budget scenario; no exact CPU percentage was measured.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `shamir_alloc` zero-fills O(len) bytes the host immediately overwrites, and does not reject negative `len`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Initialized allocation and host overwrite are present. Negative-length defense is absent and is the actionable residual; a separately emitted memset and profitable uninitialized replacement are not established.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L540).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

## Evidence and recipe corrections

- drop(params) after cloning reduces retention during validation, not the preceding peak where originals and clones coexist.
- A take API must preserve current first-match lookup behavior and absent/null semantics. A generic swap_remove recipe can reorder remaining Params entries, observable through raw(); scope it or preserve order.
- Do not form an initialized u8 slice over uninitialized allocation. Retaining zero-fill is a valid current choice.
- A guest free export would require an ownership/capacity protocol; ptr and len alone are not a universal Vec reconstruction proof.

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
