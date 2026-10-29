# 254 — audit-fix-rg-ufcs (P1 audit follow-up: RG1 UFCS atomic calls unchecked)

Source: post-merge audit of PR #345 (RG1, rust-guard `orderings` gate), verified on master. `tools/rust-guard/src/orderings_cmd/scan.rs`: the method-call visitor checks atomic operations, but the FUNCTION-call visitor recognises ONLY `fence` (~lines 436, 461). Atomic calls written in associated-function (UFCS) form - e.g. `AtomicUsize::load(&self.active, Ordering::Relaxed)` in the `WriterDrainBarrier` drain loop (`crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs` ~378-393) - are never checked, so weakening a pinned ordering via UFCS passes the gate (false negative); the existing SeqCst method-level diagnostic loads still satisfy the `load` pin.

## Change
1. Extend the function-call visitor: a call whose callee path ends in `<AtomicType>::<method>` (any `std::sync::atomic::Atomic*`, via full path, `atomic::` path or an imported name) with a known atomic method (`load/store/swap/compare_exchange*/fetch_*/fetch_update/...` - reuse the method table the method-call visitor uses) must be treated exactly like the corresponding method call: the first argument is the receiver, `Ordering` arguments are matched against the pins, CAS failure ordering included. Receiver identity for pins: derive it from the first argument expression the same way the method visitor derives it from the receiver (strip `&`/`&mut`/`self.`).
2. Tests in `tools/rust-guard/tests/orderings_tests.rs` + fixtures under `tools/rust-guard/tests/fixtures/orderings/`: a mutation fixture where the pinned production load is rewritten into UFCS with `Relaxed` must be REPORTED; the same UFCS call with the pinned ordering must pass; a UFCS call on an unpinned site is not flagged. Keep every existing fixture passing (read them, do not weaken any).
3. Update the orderings section of `tools/rust-guard/README.md` by one sentence.

## Rules
Only `tools/rust-guard`. No new dependencies; existing code style; layout limits; do not touch `.rust-guard/atomic_orderings.toml` pins.

## Local gates (only these)
`cargo fmt --manifest-path tools/rust-guard/Cargo.toml -- --check`; `cargo clippy --manifest-path tools/rust-guard/Cargo.toml --all-targets -- -D warnings`; `python -I scripts/ci/check_layout.py`. (The tool's own tests run in CI `rust-guard self-test` only.) Real exit codes.
