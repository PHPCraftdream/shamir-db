# T9-1 / R3 - Rust client pre-refactor: transport module with an explicit channel binding (behaviour unchanged)

Base: branch `feat/t9-r1-handshake` (R2 landed in master; R1 is green in CI and carries `client/handshake/` with `run_scram(ScramRequest) -> ScramOutcome`). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` sections 2.0 and 2.1 (read them, the R1 and R2 reports at the end, then CLAUDE.md). Crate: `crates/shamir-client`.

## Goal

Axis 1 (transport) becomes an explicit module, and the link between the axes becomes one named type. Today `client/transport.rs` mixes the write sink, the dial helpers and re-exports, and `run_scram` takes the channel binding as three loose fields (`transport_kind`, `binding_mode`, `tls_exporter`). Do exactly this, nothing more:

```
client/transport/            (replaces client/transport.rs; the path crate::client::transport keeps working)
  mod.rs        re-exports only: everything transport.rs exports today (Wire*, HookPoint, Lifecycle, ClosedState, LifecycleHooks, ShutdownGuard, await_pending_response, reader_task, WriteSink, connect_tcp, connect_ipc, connect_with_timeout) plus ChannelBinding, Dialed
  binding.rs    pub(crate) struct ChannelBinding { transport: TransportKind, mode: BindingMode, exporter: [u8; 32] }  (Copy, Clone, Debug); constructors tcp_tls_exporter(exporter) -> (Tcp, TlsExporter, exporter) and ipc() -> (Unix, None, [0u8; 32])
  write_sink.rs WriteSink enum + its AsyncWrite impl, moved verbatim
  dial.rs       connect_tcp, connect_ipc, connect_with_timeout moved verbatim, plus two NEW thin helpers that return the transport side of a connection:
                  dial_tcp_tls(addr, server_name, connect_timeout, phase: Option<&AtomicU8>) -> Result<Dialed<ReadHalf<TlsStream<TcpStream>>>, ClientError>
                  dial_ipc(addr, connect_timeout) -> Result<Dialed<ReadHalf<IpcClientStream>>, ClientError>
                where Dialed<R> { read: R, write: WriteSink, binding: ChannelBinding }.
                dial_tcp_tls contains exactly the code that today sits in the TCP part of Client::handshake before SCRAM (client config, ServerName, TCP connect, PHASE_TCP / PHASE_TLS stores, TLS connect, exporter extraction, split) and dial_ipc the IPC part of Client::connect_local; error values and texts unchanged.
```

`ScramRequest` (in `handshake/scram.rs`) replaces its three binding fields by one `binding: ChannelBinding`; `run_scram` reads `binding.transport/mode/exporter` where it read the old fields. `session/entry.rs` then reads: dial (`dial_tcp_tls` / `dial_ipc`) -> `run_scram` -> `assemble`. The phase constants stay where they are (only widen their visibility if `dial.rs` needs them, narrowest scope).

`client/` keeps exactly 7 entries (`transport.rs` becomes the `transport/` folder). The WriteSink keeps its `impl AsyncWrite` (do NOT change the write path in `api.rs`; introducing `write_frame` methods or a `FrameRead` trait belongs to the WebSocket step later, not here).

## Boundary (must hold)

- Public API byte for byte (see the R2 and R1 briefs): `Client::connect/connect_host/connect_local/resume/resume_secure`, option structs, `ClientError`, `CloseCause`, getters, `lib.rs` re-exports. No new public item.
- Wire bytes, error texts, timeout phase stores and their order (`PHASE_TCP`, `PHASE_TLS`, then `PHASE_SCRAM`/`PHASE_KDF` in `run_scram`), `connect_timeout` handling, the absence of `handshake_timeout` for `connect_local`, `resume` paths (they also dial TCP+TLS: reuse `dial_tcp_tls` only if the code is identical, otherwise leave `resume` alone and report it) - all unchanged.
- Guard tests are NOT edited. The one test file that builds a `ScramRequest` (`src/tests/wire/scram_binding_tests.rs`, added by R1) must be updated for the new field; that is the only allowed test edit. `crate::client::transport::{...}` imports used by tests keep resolving.
- Visibility: narrowest possible; list every widening against the base in your report.
- One file = one primary export; `mod.rs` re-exports only; no inline `#[cfg(test)]` modules; no local `use`; layout limits (7 entries per folder, 1000 lines per file): run `python -I scripts/ci/check_layout.py`.

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test`, no rustc, no npm. Children must not run commands. No `Cargo.toml`, lockfile or workflow edits; a new dependency means stop and report.
- Compiler-style read of every touched file at the end: `use` paths, privacy, generic bounds (`Dialed<R>` and the `AsyncRead` bound of the read half), `cfg(test)` gating of re-exports (an unused `cfg(test)` re-export fails `-D warnings`; verify each is used by a test through that path), unused imports, `#[path]`/serde attribute paths, and a grep over `crates/` for every moved symbol.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. Moving `transport.rs` into `transport/`: create the new files; the orchestrator removes the old `transport.rs` (report it; do not leave both with the same content). No machine-local absolute paths.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the read. Never leave a half-moved item: finish it or revert by editing back and record it BLOCKED. Append a "T9-1 R3 report" section to the audit DESIGN.md as the last step (what moved, widenings, anything BLOCKED, the resume decision).
