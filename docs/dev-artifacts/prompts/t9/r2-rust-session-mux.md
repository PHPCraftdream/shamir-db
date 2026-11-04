# T9-1 / R2 - Rust client pre-refactor: session + mux modules (behaviour unchanged)

Base: master. Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` section 2.1 (read it first, then CLAUDE.md). **Order amendment:** R2 goes first (then R1 handshake, then R3 transport seams), because `crates/shamir-client/src/client/` already has exactly 7 entries (`api.rs connect.rs cursor_stream.rs mod.rs options.rs subscription/ transport.rs`) and R2 is the step that frees slots: `connect.rs` and `options.rs` leave, `session/` arrives.

## Goal

Pure move, no behaviour change. Create `client/session/` and move into it:

```
client/session/
  mod.rs       re-exports only
  client.rs    struct Client, SessionMeta, Client::assemble (from connect.rs)
  entry.rs     Client::connect, connect_host, connect_local, resume, resume_secure (from connect.rs) - bodies unchanged for now (SCRAM duplication stays; R1 removes it)
  options.rs   moved unchanged from client/options.rs
  mux/         lifecycle.rs (Lifecycle, LifecycleHooks, ClosedState, ShutdownGuard-related types), reader.rs (reader_task, demux, await_pending_response), hooks.rs (HookPoint) - moved out of client/transport.rs
```

`client/transport.rs` stays (it keeps `WriteSink`, `connect_tcp`, `connect_ipc`, `connect_with_timeout`, `Wire*` structs, frame decode); it re-exports what moved so that `crate::client::transport::{Wire*, HookPoint, Lifecycle, ClosedState, await_pending_response, connect_with_timeout}` keep resolving. `client/mod.rs` keeps every existing public and `pub(crate)` path (`crate::client::{reader_task, ShutdownGuard, ...}`). `client/` ends with 6 entries: `api.rs cursor_stream.rs mod.rs session/ subscription/ transport.rs`.

## Boundary (must hold)

- Public API byte for byte: `Client::connect/connect_host/connect_local/resume/resume_secure`, `ConnectOptions`, `ConnectLocalOptions`, `ResumeOptions`, `SecureResumeOptions` (public fields, no `non_exhaustive`, add no field), `bound_until`, `resolve_host`, `ClientError`, `CloseCause`, getters in `api.rs`, re-exports in `lib.rs`.
- Wire bytes, error texts, timeout phase names, Argon2 in `spawn_blocking`, `Zeroizing`, "ticket sent only after the resume proof" - unchanged. The `#[path = "../interner_cache/resume/security.rs"] mod resume_security` link in `connect.rs` must keep working after the move (fix the relative path if the module moves).
- Guard tests are NOT edited: everything under `src/tests/**`, `client/subscription/tests`, `tests/*.rs` and `tests/*/`, `examples/connect.rs`, `benches/roundtrip.rs`. If one of them cannot compile without an edit, stop and report instead of editing. Keep `pub(super)`/`pub(crate)` visibility as narrow as before; if a move forces a wider visibility, use the narrowest `pub(in crate::client)` and list every widening in your report (the CI `structure` gate flags widenings).
- One file = one primary export; `mod.rs` re-exports only; no inline `#[cfg(test)]` modules; no local `use` (all imports at the top).
- Layout: at most 7 entries per folder, at most 1000 lines per file (run `python -I scripts/ci/check_layout.py`).

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test`, no rustc, no npm. A child that runs them violates the policy. No `Cargo.toml`, lockfile or workflow edits; a new dependency means stop and report.
- After the move, do a compiler-style read of every touched file: every `use` path, privacy of each moved item, `cfg(test)` gating, unused imports, `#[path]` attributes, and grep `crates/` for every moved symbol (callers in `shamir-server` tests, `shamir-client-node`, `examples`, `benches`).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. To "delete" a moved file, empty-move it by creating the new file and replacing the old file's content only if it still has a role; when a file must disappear, report it - the orchestrator removes it. No machine-local absolute paths in any file.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the compiler-style read. Never leave a half-moved item: finish the move or revert by editing back and record it BLOCKED. Append a "T9-1 R2 report" section to the audit DESIGN.md (what moved, widenings, anything BLOCKED) as the last step.
