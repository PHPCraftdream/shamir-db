<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk-macros — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Guest-side busy polling remains open, but the claimed incompatibility with async host imports is refuted by the existing fiber bridge. No macro-owned locking defect was found.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Generated guest ABI drives author `async fn` on a spin-on-`Pending` executor -- latent busy-wait/livelock, undocumented and unguarded

Status: `confirmed-open`. Current risk: `medium`.

All four generated entrypoints still use unbounded no-op-waker block_on. Permanently-Pending guest futures busy-poll until host metering interrupts them. Current async host imports already suspend the Wasmtime fiber while appearing synchronous to the SDK; they do not establish the alleged deterministic Pending failure.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:144](../../../../../crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-sdk-macros/src/lib.rs:556](../../../../../crates/shamir-sdk-macros/src/lib.rs#L556); [crates/shamir-sdk/src/__rt.rs:36](../../../../../crates/shamir-sdk/src/__rt.rs#L36); [crates/shamir-sdk/src/host_imports.rs:29](../../../../../crates/shamir-sdk/src/host_imports.rs#L29); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:449](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L449).

<a id="review-2"></a>

### Claim 2 — The concurrency-critical "Ready on first poll" contract of the emitted ABI has zero test coverage in this crate

Status: `confirmed-open`. Current risk: `low`.

No local expansion or Pending-path tests exist. There is an immediate-Ready helper test and a wired generated-function runtime test, but neither checks bounded rejection of a permanently-Pending future.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](../../../../../crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-sdk/src/tests/value_tests.rs:414](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L414); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

## Corrections and qualified non-findings

- The executor tolerates finite Pending sequences by repolling; Ready-on-first-poll is an intended restriction, not a mechanically enforced necessity.
- The proposed once-Pending-then-Ready test would already complete and would not detect unbounded polling. A bounded-poll or never-ready regression oracle is needed.
- The blanket statement that nothing exercises any generated ABI is false: crates/shamir-wasm-host/src/tests/compile_tests.rs:35 invokes #[function] output.
- Host imports are async in the host linker already: crates/shamir-wasm-host/src/wasm/wasm_function.rs:195. Guest SDK extern calls remain synchronous.
- Confirmed non-finding: macro implementation and generated wrapper infrastructure introduce no lock primitives or guards across their generated await. Author bodies are outside that guarantee.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros -- Concurrency & lock-free invariants

## Summary

The macro crate itself is trivially clean against the 5 pillars: it holds no `Mutex`/`RwLock`/`parking_lot`, no atomics, no `scc`/`dashmap`/`ArcSwap`, and its only dependencies are `syn`/`quote`/`proc-macro2` (compile-time, single-threaded execution). Nothing it emits holds a guard across an `.await` (the emitted ABI is sync; the single `.await` is the unguarded inner call). The one real theme-relevant surface is inherited by construction: all four macros lower the author's `async fn` through `shamir_sdk::__rt::block_on`, which is a no-op-waker executor that busy-spins on `Poll::Pending` -- a latent livelock baked into every generated guest module (latent only because all current host imports are synchronous).

## Findings

### 1. Generated guest ABI drives author `async fn` on a spin-on-`Pending` executor -- latent busy-wait/livelock, undocumented and unguarded

- **File:line:** `crates/shamir-sdk-macros/src/lib.rs:144` (`#[validator]`), `:264` (`#[function]`), `:391` (`#[procedure]`), `:556` (`#[scalar]`); evidence in sibling crate `crates/shamir-sdk/src/__rt.rs:36-61`
- **Severity:** medium
- **Issue:** Every macro expansion emits `shamir_sdk::__rt::block_on(#inner_name(...))` as the sole bridge from the sync `extern "C"` entrypoint to the author's `async fn`. `__rt::block_on` polls with a no-op waker and, on `Poll::Pending`, enters `core::hint::spin_loop()` forever (`__rt.rs:50-60`; its own comment concedes "If a future genuinely needs async I/O (slice 4 host imports), this will spin"). The macro crate originates this pattern yet never discloses the "must resolve on first poll" contract in any of the four macro docs, and applies no static or runtime guard against suspension. This is in tension with CLAUDE.md pillar 2 (async as the sanctioned I/O discipline) and the workspace's explicit zero-tolerance stance on hangs ("Hangs and test-locks are BUGS -- hunt and fix them, never tolerate"; a spin here is a guest-side livelock by construction). The macro's own `#[procedure]` doc example (`lib.rs:291-294`) advertises `ctx.db().table("users").query(None)` -- i.e. genuinely I/O-shaped work -- under an executor that cannot tolerate a single `Pending` poll.
- **Failure scenario:** Today the hazard is latent: all host imports (`shamir-sdk/src/host_imports.rs:139-282`) are synchronous, so SDK-provided awaits resolve on first poll. It becomes live the moment (a) a guest author awaits anything that pends (own channel/timer/pending future -- the guest then burns WASM fuel at spin-loop rate until the host's fuel/epoch meter traps it, or hangs forever without metering), or (b) the planned slice-4 async host imports land, at which point the first real I/O-bound `#[procedure]` traps or hangs deterministically. Note the spin is not even a correct wait: with a no-op waker, wake notifications are impossible, so progress is only observable by re-polling guest-local memory at 100% CPU.
- **Suggested fix:** Where the fix lands is split. In this crate: (1) state the Ready-first contract explicitly in each macro's doc ("the generated entrypoint resolves the future synchronously; a `Pending` poll spins the guest -- use only immediately-ready awaits until slice-4 host imports"); (2) when slice 4 lands, replace the emitted `__rt::block_on` lowering with a host-suspend waker (or gate it behind the then-current sanctioned primitive) rather than carrying the spin into the I/O era. In the sibling crate (out of this review's scope, but the actual guard): make `__rt::block_on` trap/panic after N consecutive `Pending` polls instead of spinning, so the failure mode is a named trap (`FunctionError::Compute`-class), never a silent burn.

### 2. The concurrency-critical "Ready on first poll" contract of the emitted ABI has zero test coverage in this crate

- **File:line:** whole crate (`crates/shamir-sdk-macros/` contains only `Cargo.toml` and `src/lib.rs`; no `tests/` directory exists)
- **Severity:** low
- **Issue:** The only concurrency invariant this crate establishes -- that the generated `shamir_call` synchronously resolves the author's future and must never encounter `Pending` (finding 1's contract) -- is asserted nowhere at the macro level. The nearest coverage is indirect and Ready-only: `shamir-sdk/src/tests/value_tests.rs::block_on_resolves_immediately` (`:414-415`). Nothing exercises any macro-emitted ABI (`validator`/`function`/`procedure`/`scalar`) or the Pending/`spin_loop` branch, so a regression that makes even the pure path pend (or that silently swaps the executor) would not be caught by any test owned by this crate. CLAUDE.md's test-organisation rules exist so per-module behavior is pinned where it is defined.
- **Failure scenario:** A future change to the emitted `quote!` body (e.g. the `TODO(slice 4)` Result-envelope rework already flagged at `lib.rs:251`) alters the async lowering; no crate-local test notices, and the first signal is a guest fuel-exhaustion trap or hang in an integration run far from the change.
- **Suggested fix:** Add a `src/tests/` directory per CLAUDE.md layout with at least: (1) a compile-and-expand test per macro asserting the emitted tokens contain exactly one `__rt::block_on` and no lock primitives; (2) a guest-side test (or a shared fixture with `shamir-sdk`) that a scalar/function whose future pends once then resolves still completes, and that a never-resolving future trips the (post-fix) trap instead of spinning unbounded.

</details>
