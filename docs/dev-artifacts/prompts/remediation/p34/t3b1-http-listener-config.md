# TOKEN-T3b-1 - shamir-server: `kind: http` listener configuration (slice 1 of 3 of T3b)

The full stage T3b is described in `docs/dev-artifacts/prompts/remediation/p34/t3b-http-listener.md`; read it in full first for context and sources. A first attempt at the whole stage failed (the executor ran out of time and reverted everything), so T3b is now three slices: T3b-1 (this brief: configuration only), T3b-2 (token gate, REST executor, ScopedHandler, audit/metrics; a parallel worktree), T3b-3 (launcher wiring and integration tests; after 1 and 2). Do ONLY your slice.

## Scope of this slice

- `ListenerKind::Http` in `crates/shamir-server/src/boot/config.rs` and its parsing: bind address, TLS settings (reuse the existing TLS configuration pieces; TLS 1.3 only for this kind), `allow_plaintext_remote` / `allow_plaintext_write` rails exactly as TOKEN_API section 11 says, limits that map onto `shamir_transport_http::HttpLimits` / `HttpServerConfig` (read `crates/shamir-transport-http/src/config.rs`), `trusted_proxies` only if TOKEN_API section 11 requires it for REST. Defaults for every field; `browser_origin_allowlist` is NOT used for REST.
- Startup validation with clear messages (plaintext on a non-loopback bind without the explicit flag; the read-only/write rails; zero or contradictory limits). Put the new code in the existing folder `crates/shamir-server/src/boot/config/` (it has 2 entries: add at most `http_listener.rs` for the settings type and `http_validation.rs`; `boot/config.rs` gets only the enum variant, the field and the call into the validation). One primary type per file.
- A pure function `HttpListenerSettings -> HttpServerConfig` (name it for what it does) that T3b-3 will call; no listener is started in this slice, nothing is wired into `launch.rs`/`listeners.rs`.
- Every `match` on `ListenerKind` in the whole workspace (src, tests, benches, examples of shamir-server and any other crate): add the `Http` arm with the correct behaviour (for example, listeners the launcher does not start yet must not be silently ignored: return a clear "not wired yet" startup error in the launcher match, which T3b-3 will replace). Every struct literal of `ListenerConfig` and of any config struct you extend, across the WHOLE workspace (the previous server stages needed several CI rounds for missed literals).
- A commented example for the new kind in `deploy/` only where the other listener kinds are documented the same way.
- Tests (you cannot run them): config parsing of a full and a minimal `kind: http` block, every validation error, defaults, the settings -> `HttpServerConfig` mapping, a configuration with an HTTP listener next to the existing kinds. Tests under `tests/` folders, one topic per file; `boot/config` and its tests folders must stay within 7 entries.

## Rules

- NO LOCAL COMPILATION: the only cargo commands you may run are `cargo fmt -p shamir-server` and `cargo fmt -p shamir-server -- --check` (and the same for another crate you touch) and `python -I scripts/ci/check_layout.py`. No check/clippy/build/test, no rustc, no npm. `Cargo.toml` and `Cargo.lock` already contain the `shamir-transport-http` dependency; do not touch either file (if cargo fmt modifies `Cargo.lock`, report it). CI compiles and lints after the orchestrator pushes (`-D warnings`), so do a lint-aware read-through of every file you touch: imports, visibility, exhaustive matches, moved values, `?` conversions, `#![warn(missing_docs)]` on new pub items, clippy.toml bans std HashMap/HashSet/RandomState, `absolute_paths` max 5 segments, `too_many_arguments` max 7, no `#[allow]`, no inline `#[cfg(test)]`, no local `use`, at most 7 entries per folder and 1000 lines per file.
- Dependencies: none new.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. You cannot move or delete files: list such needs.
- DEADLINE RULE: when the time limit comes, STOP EDITING AND REPORT. Never revert, restore, delete or empty files to "clean up" unfinished work: partial code stays exactly where it is and is listed in the report as incomplete. Reverting your own work is the worst possible outcome.
- No machine-local absolute paths in any file you write (repo-relative only).
- Crash resilience: keep `docs/dev-artifacts/audits/2026-11-01-token-t3b1-config/DESIGN.md` (decisions, new public items, every `match` and literal you updated); append one line per finished item; write the final report into it under `## Final report`.

## Final report (mandatory, also in the note)

Per item DONE / PARTIAL / DISPUTED with file:line evidence; tests by name; files read end to end; every new public item; every `match` on `ListenerKind` and every struct literal updated; what needs CI.
