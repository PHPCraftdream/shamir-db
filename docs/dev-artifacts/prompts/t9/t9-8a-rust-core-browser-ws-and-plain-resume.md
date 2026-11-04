# T9-8a - Rust client: session and resume over the browser-profile WebSocket, resume over plain loopback TCP

Base: `master` (Rust client layers R1-R3, T9-2 native WebSocket and plain TCP session, T9-3 token REST and WebSocket, T9-4 password login). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` (matrix in section 4, decision 4 "Browser profiles for native clients: session x WS browser profile: yes", the T9-2 report), the user-facing table `docs/guide-docs/guide/client-matrix.md`, then CLAUDE.md. Crate: `crates/shamir-client`. The server and `crates/shamir-connect` do not change (if a public item you need is missing there, stop and report).

## Goal

Three matrix cells of the Rust column move from `нет (T9-8)` to `да`, each proven by an end-to-end test against a real server:

- **row 3** - session over the browser-profile WebSocket: listener `ws`, profile `tls_no_export`, path `/shamir/v1/browser`, `Origin` required by the server, binding mode `TlsNoExport` (0x02), no TLS exporter in the `auth_message`. Useful behind a TLS-terminating proxy that breaks the exporter;
- **row 6c** - resume over the same listener;
- **row 6d** - resume over plain loopback TCP (listener `tcp`, profile `plain`, binding mode 0x00). Today there is session over plain TCP (`Client::connect_plain`) but no resume; the T9-5 napi work was blocked by exactly this missing core API. The IPC resume path (`ipc_resume_e2e` in `crates/shamir-server/tests/e2e/`) shows the byte-level shape of resume with binding 0x00.

## What to build (deviations only with a written reason in the report)

1. **Public API, minimal and in the existing style** (`client/session/entry_ws.rs` holds the WebSocket entry points; keep files under 1000 lines and folders at 7 entries, add files instead of growing `entry_ws.rs` past its size):
   - `Client::connect_ws_browser(opts: BrowserWsOptions)`, `Client::resume_ws_browser(opts: BrowserResumeOptions)` (and a `_secure` variant only if the existing `resume_ws_secure` pattern makes it necessary: resume must verify the pinned server identity before the ticket is sent, as `resume_secure` / `resume_ws_secure` do). The options wrap the existing `ConnectOptions` / `ResumeOptions` plus the required `origin: String`; `#[non_exhaustive]` with constructors, secrets never in `Debug`.
   - `Client::resume_plain(opts: ResumeOptions)` with the same loopback-only refusal as `connect_plain` (before any socket is opened, `ClientError::Handshake`).
2. **Transport.** The browser profile cannot use the TLS exporter, so trust is the ordinary TLS certificate check against the options' roots (WebPKI roots by default, a PEM root when given, as the token WebSocket in T9-3b does; reuse that trust plumbing, do not invent a second one). `GET /shamir/v1/browser` upgrade with `Sec-WebSocket-Protocol: shamir-v1` and `Origin: <origin>`. Reuse `WsByteStream` and `ChannelBinding` (add `ChannelBinding::ws_browser` = (WebSocket, TlsNoExport, zeros)); `run_scram`, `resume_wire_exchange` and `Client::assemble` stay untouched.
3. **Handshake.** For binding 0x02 and 0x00 the exporter is NOT part of the `auth_message` and resume carries no TLS exporter; read the server side (`crates/shamir-connect`, `crates/shamir-server/src/engine/server/`) and the existing IPC resume client path before writing, and copy its behaviour (including what resume proves: the TS browser profile resumes without `resume_proof`; check what the Rust IPC resume does and keep the same).
4. **No behaviour change** for existing entry points and wire bytes.

## Tests (CI-only; they must exercise the real paths)

- Extend `crates/shamir-client/tests/matrix/` (read `common.rs`, `ws_session.rs`, `ws_resume.rs`, `plain_session.rs`): the fixture server gains a `ws` / `tls_no_export` / `/shamir/v1/browser` listener with an allowed origin, and reuses the existing `tcp` / `plain` listener for resume. New files, one topic each: `ws_browser_session.rs`, `ws_browser_resume.rs`, `plain_resume.rs`. Cases: session and a batch over the browser profile; the same session refused without `Origin` and with a wrong `Origin` (server error, no panic); resume over the browser profile after a browser-profile session; a ticket issued on the native `tcp` listener resumes over plain TCP and a plain-TCP ticket resumes over TCP (the matrix already moves tickets between TCP and WS for the native profile: mirror it); `resume_plain` refuses a non-loopback address before opening a socket; wrong ticket is refused.
- Unit tests next to the new code for the options types (no secret in `Debug`) and the binding constructors.

## Docs

- `docs/guide-docs/guide/client-matrix.md`: Rust rows 3, 6c, 6d become `да (тест: <path of the proving test file>)` ONLY for cells whose test you wrote; leave anything not proven as it is.
- `docs/guide-docs/KNOWN_LIMITATIONS.md` "Client coverage": drop the Rust gaps that no longer exist.
- Append a "T9-8a report" section to `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` as the last step (what was added, widenings, decisions, what is not done).

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test/metadata`, no rustc, no npm. Children (if any) only edit files and must not run commands. The orchestrator runs `cargo clippy -p shamir-client --all-targets -- -D warnings` itself after you finish, so read your Rust like a compiler (privacy, imports at the top of the file, `cfg(test)` gating, unused items under `-D warnings`, `clippy::absolute_paths` max 5 segments). Read real signatures from the repo and the cargo registry (`D:/system_artefact/cargo/registry/src/*/`: `tokio-tungstenite-*`, `tungstenite-*`, `tokio-rustls-*`) instead of writing from memory.
- Layout limits: at most 7 direct entries per folder, 1000 lines per file, one primary export per file; `mod.rs` re-exports only; no inline `#[cfg(test)]` modules; no local `use`.
- No `Cargo.toml`, `Cargo.lock`, `package.json` or workflow edits (everything needed is already a dependency); if something is missing, stop and report.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. No machine-local absolute paths in any file. Use forward slashes in all paths. Do not mention the continuous-integration setup of the orchestrator (which repository runs it) in any file; write "CI".
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the read-through. Never leave a half-edited item: finish it or record it BLOCKED.
