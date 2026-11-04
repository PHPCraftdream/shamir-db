# T9-1 / R1 - Rust client pre-refactor: handshake module, one SCRAM for every transport (behaviour unchanged)

Base: master (R2 landed: `client/session/{client,entry,options}.rs`, `session/mux/`). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` sections 2.0 and 2.1 (read them first, then CLAUDE.md). Crate: `crates/shamir-client`.

## Goal

The SCRAM exchange is written twice in `client/session/entry.rs` (`Client::handshake` for TCP+TLS and `Client::connect_local` for IPC) and the wire structs `WireAuthInit .. WireResumeProofOk` live in `client/transport.rs`. Create `client/handshake/` (axis 2: frames + channel binding only, never opens a socket) and make both entry points call ONE implementation.

```
client/handshake/
  mod.rs       re-exports only
  wire.rs      Wire* structs moved from transport.rs (transport.rs re-exports them: crate::client::transport::Wire* must keep resolving)
  auth_ok.rs   VerifiedAuthOk + verify_auth_ok (from session/entry.rs)
  scram.rs     run_scram<R, W>(...): the single SCRAM client exchange (auth_init -> challenge -> KDF in spawn_blocking -> proof -> auth_ok), generic over R: AsyncRead + Unpin and W: AsyncWrite + Unpin
  resume.rs    ResumedHandshake, resume_wire_exchange, verify_resume_identity_proof (from session/entry.rs)
```

`run_scram` takes the channel binding as an explicit input (a small `pub(crate)` struct or three parameters: `TransportKind`, `BindingMode`, exporter `[u8; 32]`; TCP passes `(Tcp, TlsExporter, <exporter>)`, IPC passes `(Unix, None, zeros)`), plus username, optional trusted pin / accept-new-host, the `phase: Option<&AtomicU8>` the TCP path updates (`PHASE_KDF` etc. keep their current meaning; IPC passes `None` and keeps having no phase), and whatever the two call sites need to feed `Client::assemble`. Both call sites become: dial (unchanged) -> `run_scram` -> `assemble`. `session/entry.rs` keeps the public entry points, option handling, timeouts and dialing; it loses the SCRAM bodies. `client/` ends with exactly 7 entries: `api.rs cursor_stream.rs handshake/ mod.rs session/ subscription/ transport.rs` (do not add an eighth).

## Boundary (must hold)

- Public API byte for byte (see the R2 brief): `Client::connect/connect_host/connect_local/resume/resume_secure`, option structs, `ClientError`, `CloseCause`, getters, `lib.rs` re-exports. No new public item.
- Wire bytes and error texts identical (`"send auth_init"`, `"read challenge"`, `"read auth_ok"` ... keep every string), the same frame limits (`MAX_PRE_AUTH_FRAME`), same order of operations, Argon2 in `spawn_blocking`, `Zeroizing` for the password, derived keys and the resumption ticket, the resumption ticket handed out only after the resume proof (see `resume_wire_exchange`). `connect_local` keeps having no `handshake_timeout`; the TCP path keeps its `PHASE_*` updates in the same places. A difference between the two old copies (comments mention some on purpose) must be preserved as a parameter, not erased: diff them first and list every difference in your report.
- Guard tests are NOT edited: all of `src/tests/**` except the one new file below (plus its line in `src/tests/wire/mod.rs`), `client/subscription/tests`, `tests/*`, `examples/connect.rs`, `benches/roundtrip.rs`. Old import paths used by tests (`crate::client::transport::{Wire*, ...}`, `crate::client::...`) keep resolving through re-exports. If a guard test cannot compile without an edit, stop and report.
- Visibility: narrowest possible; `pub(in crate::client)` or `pub(crate)` only where a sibling needs it; list every widening against the base in your report (the CI `structure` gate flags widenings).
- One file = one primary export; `mod.rs` re-exports only; no inline `#[cfg(test)]` modules; no local `use`; layout limits (7 entries per folder, 1000 lines per file); `python -I scripts/ci/check_layout.py`.

## New test

`src/tests/wire/scram_binding_tests.rs` (add `pub mod scram_binding_tests;` to `src/tests/wire/mod.rs`; the folder then has 6 entries): drive `run_scram` over `tokio::io::duplex` against an emulated server (build the server side with `shamir_connect` helpers the way existing tests in `src/tests/auth/` do, e.g. `kdf_rejection_tests.rs` and `auth_identity_tests.rs`; read them first) for two bindings: `(Tcp, TlsExporter, a fixed non-zero exporter)` and `(Unix, None, zeros)`. Assert the `binding_mode` byte in `auth_init` and that the exchange completes for each. The test must not be `cfg(unix)`; JSON-free; multi-line formatting for literals.

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test`, no rustc, no npm. A child that runs them violates the policy (children have already run stray no-op commands; say so in every child prompt). No `Cargo.toml`, lockfile or workflow edits; a new dependency means stop and report.
- After the change, a compiler-style read of every touched file: `use` paths, generic bounds on `run_scram` (`R: AsyncRead + Unpin`, `W: AsyncWrite + Unpin`, futures `Send` where `tokio::spawn`/`spawn_blocking` need it), moved items' privacy, `cfg(test)` gating, unused imports under `-D warnings`, serde attributes that name paths (`deserialize_with`), and a grep over `crates/` for every moved symbol (callers in `shamir-server` tests, `shamir-client-node`, examples, benches).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. A file that must disappear is reported; if a moved-from file still has a role keep it and trim it. No machine-local absolute paths in any file.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the compiler-style read. Never leave a half-moved item: finish it or revert by editing back and record it BLOCKED. Append a "T9-1 R1 report" section to the audit DESIGN.md as the last step (what moved, differences found between the two SCRAM copies, widenings, anything BLOCKED).
