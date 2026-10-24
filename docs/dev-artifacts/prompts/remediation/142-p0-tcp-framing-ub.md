# P0 stage D5 — p0-tcp-framing-ub

## Goal

Owner ruling D5 (two reviewers agreed): `crates/shamir-transport-tcp/src/framing.rs:117-128` does `clear` → `reserve(len)` → `unsafe set_len(len)` → `read_exact(buf)`, handing uninitialized memory to an `AsyncRead` as an initialized `&mut [u8]` (undefined behaviour regardless of the reader; the SAFETY comments at `:93-97` and `:118-123` are wrong). A dropped (cancelled) read also skips the cleanup at `:134`; the client reuses its buffer (`crates/shamir-client/src/client/transport.rs:201`).

Acceptance:
- Replace `set_len` with `buf.resize(len, 0)` (preferred, smallest) or `take(len)` + `read_buf` into spare capacity. No uninitialized byte may enter the vector's live length.
- Add `#![forbid(unsafe_code)]` to `crates/shamir-transport-tcp/src/lib.rs` (these are the crate's only `unsafe` uses — verify with grep); delete the stale SAFETY comments.
- Regression tests (names contain `rv_client_d4d604ca`): short/error read returns an error and leaves the buffer in a valid state; a read cancelled after partial progress followed by reuse of the same buffer yields the correct next frame. Use an in-memory duplex / custom `AsyncRead` that delivers bytes in controlled pieces, no sleeps.
- The card is `### RV-CLIENT-D4D604CA` in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read it).

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
