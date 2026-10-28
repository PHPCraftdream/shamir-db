# 231 — p1-engine-drain-barrier-loom (F14d, card 898ECB5E)

Split from F14. The ordering-pin gate (RG1, `rust-guard orderings`, `.rust-guard/atomic_orderings.toml`) is now live on master and pins the PRODUCTION orderings of `WriterDrainBarrier`. This stage adds the loom proof only.

All paths are relative to `crates/shamir-engine/src`.

## 898ECB5E — writer drain barrier: loom proof + ordering pin gate (OWNER DECISION 2026-10-27)

**Owner decision:** do NOT change production orderings or add a production fence.

### Production barrier
In `table/support/guards/writer_drain_barrier.rs`:
- Put the barrier's orderings behind cfg so that:
  - `#[cfg(not(loom))]` keeps the CURRENT SeqCst accesses exactly. No extra mfence on the hot path.
  - `#[cfg(loom)]` uses the fence variant:
    - `enter_writer`: `fetch_add(1, Relaxed)` then `fence(SeqCst)`
    - guard drop: `fetch_sub(1, Release)`
    - `begin_drain()`: `fence(SeqCst)`
    - `is_drained()`: `load(Acquire) == 0`
- `drain()` = `begin_drain()` then a `while !is_drained()` loop around `yield_now`. Keep the existing public API.
- Use cfg-gated imports at the TOP of the file:
  - `#[cfg(loom)] use loom::sync::{Arc, atomic::{AtomicUsize, Ordering, fence}};`
  - the std equivalent under `#[cfg(not(loom))]`.
- Doc comment:
  - The proof by the SC-fence rule: if fence X ≺ fence Y in S, then B sees A or later, so either the drainer sees counter ≥ 1 or the writer sees the flag. That covers the loom variant.
  - The argument that the production SeqCst accesses participate in the same single total order S, so the guarantee carries over.
  - A pointer to the ordering-pin gate below.

### Loom model
- Move the inline loom module into `table/support/guards/tests/loom_model.rs`, wired as `#[cfg(all(loom, test))] mod tests;` in `guards/mod.rs` (the folder goes from 4 to 5 entries).
- The existing CI job is `loom-model` with filter `test(loom_model)`. Its test names must contain `loom_model`; check the discovery filter.
- `rv_engine_898ecb5e_production_barrier_drain_sees_inflight_writer`:
  - Use the real `WriterDrainBarrier`, a Relaxed flag and a Relaxed `wrote`.
  - Writer: `enter_writer`, read the flag, write, drop the guard.
  - Drainer: set the flag, `begin_drain`, spin on `is_drained`, snapshot.
  - Assert `took_fast ⇒ snapshot`.
  - It catches: removing either fence, `Release`→`Relaxed` on drop, and `Acquire`→`Relaxed` on the drain load.
- Control `rv_engine_898ecb5e_oracle_detects_unfenced_sb`, `#[should_panic]`: a local copy WITHOUT fences must fail. This proves the oracle can detect the bug.

### Ordering pin gate (already on master — do not edit it)
- Read `.rust-guard/atomic_orderings.toml` and `tools/rust-guard` (orderings subcommand) first. The production (`#[cfg(not(loom))]`) accesses must stay literally `Ordering::SeqCst`, written inline at each call (no consts, wrappers or helper fns), in the same items the pin file names, so the gate keeps passing unchanged. If your cfg split changes an item name or moves a pinned call into a different item, STOP and report it instead of editing the pin file.
- `#[cfg(loom)]` items/statements are excluded from the gate by its cfg evaluation; do not add pins for them.

### Notes
- The current inline module is `#[cfg(all(loom, test))] mod loom_model` at the end of `table/support/guards/writer_drain_barrier.rs` (~line 470). Move it (and its existing F-56/F-84 explanatory comment block, which documents why loom needs explicit SeqCst fences) into `table/support/guards/tests/loom_model.rs`; `table/support/guards/tests/mod.rs` is a manifest only. Keep the existing loom tests unless the new production-barrier test strictly subsumes them; never lose a test.
- The loom CI job discovers tests with `cargo nextest list -E 'test(loom_model)'` and fails on zero tests: every loom test path must contain `loom_model` (the module name gives that).
- `#[cfg(loom)]` imports go at the top of the file next to their `#[cfg(not(loom))]` std equivalents (no local `use`).
- `rust-guard structure` rules apply to changed files: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no visibility widening.
- shamir-engine has `#![deny(clippy::unwrap_used, clippy::expect_used)]` for non-test code.
- Layout: <=7 entries per folder, <=1000 lines per file.

### Local gates (only these; one crate at a time; never --all-features)
- `cargo fmt -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`
- `cargo clippy -p shamir-engine --no-default-features --all-targets -- -D warnings`
- `cargo check -p shamir-engine --features loom --all-targets` — type-check only (the `loom` feature turns on `cfg(loom)` via build.rs); confirms the loom-gated code compiles. Do not run the loom tests.
- `python -I scripts/ci/check_layout.py` (counts tracked files only: report every NEW file path so the orchestrator can `git add` it).
Loom itself runs only in CI.
