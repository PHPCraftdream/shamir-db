<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The SDK's own source remains free of synchronization and hash-map structures. Unresolved futures still busy-poll, but the claimed inevitable/permanent no-progress behavior is overstated.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 1 | 0 | 0 | 0 | 0 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `__rt::block_on` busy-spins on `Poll::Pending` with a no-op waker; the "pure functions only" guard is a stale comment, not an enforced invariant

Status: `confirmed-open`. Current risk: `medium`.

All four macros use the unchanged no-op-waker polling loop. Unresolved guest-local futures consume CPU without scheduling support. Repeated polls can nevertheless reach Ready; production WASM has fuel and epoch/deadline bounds, unlike direct native helper use.

Evidence: [crates/shamir-sdk/src/__rt.rs:36](../../../../../crates/shamir-sdk/src/__rt.rs#L36); [crates/shamir-sdk/src/__rt.rs:50](../../../../../crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-sdk-macros/src/lib.rs:144](../../../../../crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-sdk-macros/src/lib.rs:556](../../../../../crates/shamir-sdk-macros/src/lib.rs#L556); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:475](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L475); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:487](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L487).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — O(N) linear scans for keyed lookups (`Params::get`, `HttpResponse::from_value`) -- pillar-3 adjacent, bounded and documented; acked for the audit trail

Status: `not-applicable`. Current risk: —.

The scans remain O(N), but this report explicitly accepts the Vec trade-off and requires no fix. Small input counts are an expectation, not an enforced bound; hashing superiority or inferiority was not measured.

Evidence: [crates/shamir-sdk/src/params.rs:26](../../../../../crates/shamir-sdk/src/params.rs#L26); [crates/shamir-sdk/src/http.rs:130](../../../../../crates/shamir-sdk/src/http.rs#L130); [crates/shamir-sdk/src/value.rs:13](../../../../../crates/shamir-sdk/src/value.rs#L13).

Grouping/duplicate: `SUMMARY.md#4.4`. This row is not another independent defect.

<a id="review-summary"></a>

### Claim Summary — Lock-free/O(x->0)/Fx-hash/scc-dashmap pillars: trivially compliant concurrency surface

Status: `not-applicable`. Current risk: —.

Non-finding supported for the SDK's own source: no mutexes, atomics, concurrent registries, or hash-map fields were found. This does not extend to optional query-builder dependencies or host-runtime implementation.

Evidence: [crates/shamir-sdk/src/params.rs:11](../../../../../crates/shamir-sdk/src/params.rs#L11); [crates/shamir-sdk/src/value.rs:35](../../../../../crates/shamir-sdk/src/value.rs#L35); [crates/shamir-sdk/Cargo.toml:18](../../../../../crates/shamir-sdk/Cargo.toml#L18).

## Corrections and qualified non-findings

- Current host imports are synchronous from the guest's perspective; host async suspension does not itself cause guest Poll::Pending.
- Continuous repolling disproves the statement that the no-op waker provides no possible progress mechanism.
- Default production limits include fuel, epoch interruption, a 30-second deadline, and 64 MiB memory; permanent host-worker wedge is not established.
- The accepted linear-scan observation is not an unresolved runtime defect requiring replacement with a hash map.

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
