# P0 stage D3 — p1-ws-subprotocol

## Goal

Card `RV-TRANSPORT-WS-C0C43A18` (read the whole card in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md`): the spec (`docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18`) mandates the `shamir-v1` WebSocket subprotocol but neither accept callback negotiates it. Ruling by two rush reviewers + @oh (owner-approved): IMPLEMENT the spec.

Binding rules:
- Server (`crates/shamir-transport-ws/src/server.rs`, both the native and the browser acceptors): when the client offers `Sec-WebSocket-Protocol` containing `shamir-v1`, the handshake response MUST select it (`Sec-WebSocket-Protocol: shamir-v1`). If the header is absent, or it lists protocols but none is `shamir-v1`, reject with HTTP 400 (WHATWG §2.2: a browser that offered a subprotocol requires an acknowledgement; a client that offers nothing is a spec violation). Header parsing is a comma-separated token list, case-sensitive token compare, whitespace-trimmed, non-text header values rejected with 400. Origin/path rejection order from the existing code stays (path 404, origin 403/400 first, then subprotocol 400).
- Update every first-party client so it offers `shamir-v1`: the Rust test clients in `crates/shamir-transport-ws` and `crates/shamir-server` tests/e2e, and the TypeScript browser/node adapters under `crates/shamir-client-ts/src/platform/` (e.g. `browser.ts` ~:118). Do not change the wire framing.
- Add a CHANGELOG "Changed" entry: WS endpoints now require the `shamir-v1` subprotocol (alpha policy: no compatibility shim).
- Do NOT edit the spec text (it already requires this) except to fix a contradictory sentence if you find one.

Tests (names contain `rv_transport_ws_c0c43a18`): offered `shamir-v1` (alone and among others, e.g. `foo, shamir-v1`) succeeds and the response carries the selected header, for both acceptors; missing header -> 400; only incompatible offers -> 400; the TS adapters' unit tests assert the offered protocol (add or extend a vitest/jest unit test in the existing TS test layout; tests are not run locally). Each test must fail with only its own fix reverted.

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
