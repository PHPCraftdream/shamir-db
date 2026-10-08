<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The SDK itself contains no synchronization registries. Unresolved-future polling is real, but it is neither caused by host async imports nor inevitably nonterminating after one Pending.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 1 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `__rt::block_on` busy-spins on `Poll::Pending` with a no-op waker; the "pure functions only" guard is a stale comment, not an enforced invariant

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

All four wrappers use the same continuous polling loop. Always-Pending exhausts the WASM execution budget; Pending-then-Ready finishes. Direct native helper use has no internal bound.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-sdk-macros/src/lib.rs:144](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:477](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L477).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — O(N) linear scans for keyed lookups (`Params::get`, `HttpResponse::from_value`) -- pillar-3 adjacent, bounded and documented; acked for the audit trail

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Scans are present and input counts are not enforced, but the documented Vec choice and owned accessors are deliberate. No measured hashing advantage establishes a repair obligation.

Evidence: [crates/shamir-sdk/src/params.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L26); [crates/shamir-sdk/src/http.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L130); [crates/shamir-sdk/src/value.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L13).

Grouping/duplicate: [SUMMARY.md#4.4](SUMMARY.md#review-4-4). This is not an additional independent defect.

<a id="review-summary"></a>

### Claim Summary — Lock-free/O(x-&gt;0)/Fx-hash/scc-dashmap pillars: trivially compliant concurrency surface

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Own-source inspection found no locks, atomic/shared registries, or hash-map fields. This does not prove universal O(1) helpers or properties of the optional builder/host.

Evidence: [crates/shamir-sdk/src/params.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L11); [crates/shamir-sdk/src/value.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L35); [crates/shamir-sdk/Cargo.toml:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/Cargo.toml#L18).

## Evidence and recipe corrections

- Published wasmtime-46.0.2.crate src/runtime/func.rs describes host async suspension through stack switching while guest calls remain synchronous.
- A poll-once rejection policy changes currently successful Pending-then-Ready behavior and needs an explicit authoring contract.
- Native park-based execution requires real wake synchronization; it cannot be justified solely by replacing spin_loop.
- The no-lock non-finding is narrower than the report's original blanket O(x-&gt;0) compliance assurance.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk -- Concurrency & lock-free invariants

## Summary

This crate is a guest-authoring SDK (WASM UDF ABI + plain-data DTOs) and its concurrency surface is tiny and, on the lock-free/O(x->0)/Fx-hash/scc-dashmap pillars, trivially compliant: a full grep of `crates/shamir-sdk/` shows zero `std::sync::Mutex`/`RwLock`, zero `parking_lot`, zero atomics, zero `scc`/`dashmap`/`ArcSwap`, and no hash-keyed structures at all (`Params`/`Value::Map` are deliberately `Vec`-based, documented at `src/value.rs:13-15`), so there are no locks across `.await`, no hot-path lock justifications missing, and no `scc::*::len()` calls lacking an O(N) ack. The single genuine concurrency-invariant concern is `__rt::block_on` (`src/__rt.rs:36-61`): a no-op-waker **busy-spin** executor that drives every macro-generated guest UDF, whose "pure functions never yield Pending" correctness argument is a stale comment -- not an enforced invariant -- and contradicted by the crate's own shipped non-pure API (`Ctx::http_fetch`/`db`/`call`), with the `Pending` branch entirely untested. Everything else checked out clean.

## Findings

### 1. `__rt::block_on` busy-spins on `Poll::Pending` with a no-op waker; the "pure functions only" guard is a stale comment, not an enforced invariant

- **File:line:** `crates/shamir-sdk/src/__rt.rs:36-61` (no-op waker vtable `:38-46`; spin branch `:53-58`); consumed by all four macro expansions in `shamir-sdk-macros/src/lib.rs` (`:144`, `:264`, `:391`, `:556`).
- **Severity:** high
- **Issue:** `block_on` drives a future with a `Waker` whose `wake`/`wake_by_ref` are no-ops and, on `Poll::Pending`, enters `core::hint::spin_loop()` forever. Its doc comment claims correctness because "pure functions (the only kind this slice supports) are `Ready` on the first poll," while the `Pending` branch concedes "If a future genuinely needs async I/O (slice 4 host imports), this will spin. For now, a tight loop is correct." That deferral has aged out: this same crate now ships non-pure capability reachable from `#[function]`/`#[procedure]` bodies that all run under this loop -- `Ctx::http_fetch`/`http_get`/`http_post` (`src/context.rs:116-129`), `Ctx::db` (`src/context.rs:99`), `Ctx::call` (`src/context.rs:86`). All current host imports are synchronous FFI, so nothing in-crate yields *today*, but the SDK explicitly advertises "plain async Rust" authoring, and nothing but the (now stale) comment prevents a guest-authored future from genuinely yielding. Because the waker can never schedule anything, a yielding future has **no progress mechanism whatsoever** -- this is an unbounded 100%-CPU spin, not even a benign park. Per CLAUDE.md's "Hangs and test-locks are BUGS -- hunt and fix them, never tolerate" doctrine, an unbounded-spin wedge is exactly the bug class the repo bans (and one that nextest can only surface as a 180 s `TIMEOUT`, after the damage).
- **Failure scenario:** A guest author writes `#[function]` whose body awaits a genuinely-pending future (e.g. `futures::future::pending::<()>().await`, a channel receive, or -- once slice-4 async host imports exist -- any awaited host I/O). The host calls `shamir_call`; the guest polls `Pending` and spins; the host worker thread that invoked the call is wedged at 100% CPU until killed. In tests this is a 180 s `TIMEOUT`; in production it is a permanent per-call worker wedge, repeatable by any authored function. The `Pending` branch has zero test coverage (`src/tests/value_tests.rs:414-417` exercises only the Ready path), so CI would not catch the regression path either.
- **Suggested fix:** Enforce the invariant instead of commenting it: poll once, and on `Poll::Pending` call `__rt::trap("yielding futures are not supported by the guest runtime")` so the misuse fails fast as a catchable `Compute` error rather than hanging the host (a no-op waker can never make progress, so fail-fast is the honest WASM primitive; a real park only makes sense on the host-testing target, e.g. a `thread::park`-backed waker there). At minimum, replace the stale "pure functions (the only kind this slice supports)" justification with an explicit deferred-debt ack naming the new non-pure API surface, and add a test asserting the chosen `Pending` behavior. Revisit with a schedulable waker when async host imports land.

### 2. O(N) linear scans for keyed lookups (`Params::get`, `HttpResponse::from_value`) -- pillar-3 adjacent, bounded and documented; acked for the audit trail

- **File:line:** `crates/shamir-sdk/src/params.rs:26-32` (every typed getter is a full `iter().find` scan; reading P params from an N-param set costs O(P*N)); `crates/shamir-sdk/src/http.rs:130-153` (three sequential `map.iter().find` passes per parsed response).
- **Severity:** nit
- **Issue:** CLAUDE.md pillar 3 ("Avoid hidden O(N)/O(N²) in helpers -- full scans, repeated lookups") flags this pattern; however, the workspace's *banned* form is specifically `scc::*::len()` on concurrent maps, and this crate contains no concurrent structures at all. Here N is the host-supplied parameter/header count (single digits in practice), the Vec-over-IndexMap choice is documented (`src/value.rs:13-15`, dependency avoidance for the guest binary), and hashing short keys would likely be slower than the linear scan at these sizes.
- **Failure scenario:** None realistic; only degrades if some future builder-generated procedure passes hundreds of params or a response carries hundreds of headers.
- **Suggested fix:** None required. If param counts ever grow, consider `shamir_collections::TMap` (IndexMap + Fx) or a one-shot position cache on `Params` rather than per-get scans. Listed here so the pillar-3 audit trail shows the pattern was checked and consciously accepted, not missed.

</details>
