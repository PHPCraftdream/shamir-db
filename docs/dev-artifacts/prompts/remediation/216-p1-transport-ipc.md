# P1 stage G8 — p1-transport-ipc

## Goal

IPC: stale socket recovery, logical endpoints, Drop ownership. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G8): A stale socket left by an unclean exit is recovered safely; logical endpoint names behave the same on every OS (or the spec is changed); Drop never unlinks an endpoint it does not own; public signatures agree across platforms; typed errors; Windows DACL/exclusivity tests.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TRANSPORT-IPC-E0B387B3 — No recovery from a crash-left stale socket path (unclean exit bricks the listener until manual rm)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ipc/src/unix.rs:62; crates/shamir-transport-ipc/src/unix.rs:88; changed by: none
- R0 note (includes the fix site): OBSERVED: bind still directly delegates to Tokio without stale-path recovery, and its Drop documentation explicitly records the unclean-exit limitation; the new cleanup covers only failed chmod. Fix site: crates/shamir-transport-ipc/src/unix.rs, preserving live endpoints and non-socket files during any reclamation.

### RV-TRANSPORT-IPC-BD4EBEF7 — Spec §9 logical endpoint names are unimplemented — and fail *differently* per OS

- R0 verdict: `confirmed-open`, risk `medium`; evidence: docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:131; crates/shamir-client/src/client/transport.rs:139; changed by: none
- R0 note (includes the fix site): OBSERVED: the specification still promises logical-name mapping, but production forwards the address unchanged to platform connect functions, which use the raw socket path or pipe name. Fix site: crates/shamir-client/src/client/transport.rs for production endpoint resolution and consistent platform mapping.

### RV-TRANSPORT-IPC-DD5D7B60 — Unix listener Drop can unlink a replacement endpoint it does not own

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ipc/src/unix.rs:25; crates/shamir-transport-ipc/src/unix.rs:94; changed by: none
- R0 note (includes the fix site): OBSERVED: listener ownership still records only the pathname and Drop removes it without endpoint-identity validation; chmod-failure cleanup does not guard replacement after successful bind. INFERRED: an authorized pathname replacement remains vulnerable to the old listener's Drop. Fix site: crates/shamir-transport-ipc/src/unix.rs.

### RV-TRANSPORT-IPC-8ED1B450 — Windows test gaps: DACL content and first-instance exclusivity are untested (spec §11 checklist items)

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-transport-ipc/src/tests/windows_tests.rs:209; crates/shamir-transport-ipc/src/windows.rs:151; changed by: 990177e6 (#291)
- R0 note (includes the fix site): OBSERVED: the new lifecycle oracle attempts a second live public bind and requires failure, then verifies rebind and communication after drop; effective pipe DACL content and denial to another principal remain untested. Fix site: crates/shamir-transport-ipc/src/tests/windows_tests.rs for the remaining effective-DACL oracle.

### RV-TRANSPORT-IPC-ACD301ED — Platform-divergent public signatures: `connect`/`bind`/`path()` differ across `cfg`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-ipc/src/unix.rs:39; crates/shamir-transport-ipc/src/windows.rs:83; changed by: none
- R0 note (includes the fix site): OBSERVED: Unix connect/bind still accept AsRef<Path> and path returns &Path; Windows uses &str/Into<String> and returns &str. Current string callers do not remove the public portability gap. Fix sites: crates/shamir-transport-ipc/src/unix.rs and crates/shamir-transport-ipc/src/windows.rs.

### RV-TRANSPORT-IPC-2072FE7B — Declared-but-unused `thiserror` dependency — the typed error this crate needs was never written

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-transport-ipc/Cargo.toml:17; crates/shamir-transport-ipc/src/windows.rs:102; changed by: none
- R0 note (includes the fix site): OBSERVED: thiserror remains declared without any Rust source use; io::Result remains appropriate and does not itself require a custom enum. Fix site: crates/shamir-transport-ipc/Cargo.toml, removing the unused dependency rather than inventing an error abstraction.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU3 — IPC endpoint names (RV-TRANSPORT-IPC-BD4EBEF7)

Decided by 2x rush reviewer + @oh (unanimous, 3/3): **option B — an IPC endpoint is a raw platform address; the spec's logical names are dropped; one validator enforces it.**

Facts: only the spec promises logical names (docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md ~lines 133-140); no code contains `shamir+unix`; every production path forwards the raw string (server launch, `crates/shamir-client/src/client/connect.rs` and `transport.rs`, `crates/shamir-transport-ipc/src/unix.rs` and `windows.rs`); the only validation is non-empty addr + plain profile (`crates/shamir-server/src/boot/config.rs` ~lines 894-907); existing users, tests and the TS client already pass raw addresses. The Node binding has no IPC surface.

Acceptance:
1. New file in `shamir-transport-ipc`, re-exported from `lib.rs`: `pub fn validate_endpoint(addr: &str) -> io::Result<()>` returning `io::ErrorKind::InvalidInput` with a message naming the violated rule; no OS calls.
2. Unix rules: `Path::is_absolute`; no `Component::ParentDir`; no NUL; byte length below `SUN_PATH_LEN` (108 on linux/android, 104 on macos/ios/*bsd/dragonfly, matching std's `sockaddr_un` bound).
3. Windows rules: starts with `\\.\pipe\` (ASCII case-insensitive); the rest is non-empty with no `\`; no NUL; total at most 256 UTF-16 units; rejects `\\server\pipe\x`, `\\?\pipe\x`, `//./pipe/x` and a bare `shamir-db`.
4. The first statement of `connect` and of `IpcListener::bind` on both platforms is `validate_endpoint`; a rejected address creates no filesystem entry and no pipe instance.
5. `validate_listener` (config.rs) checks profile=plain first, then calls `validate_endpoint`, mapping the error to `ConfigError::Validation` with `listeners[{idx}]` in the message; `rejects_unix_listener_with_non_plain_profile` stays platform-neutral.
6. Unit tests in `crates/shamir-transport-ipc/src/tests/` for each rule and the boundaries (Unix `SUN_PATH_LEN-1` accepted and `SUN_PATH_LEN` rejected; Windows 256 accepted and 257 rejected).
7. Unix entry-point tests: `connect("rel.sock")` is `InvalidInput` (not `NotFound`); `bind("rel.sock")` is `InvalidInput` and creates no file; `bind("/tmp/x/../y.sock")` is `InvalidInput`.
8. Windows entry-point tests: `connect("shamir-db")` and `bind("shamir-db")` are `InvalidInput`; `connect(r"\\server\pipe\x")` is `InvalidInput`.
9. Config tests: a relative Unix path (on Unix) and a non-prefixed name (on Windows) are rejected by `validate()`; the 5-listener fixture uses a cfg-selected address so it validates on both CI operating systems.
10. TS: `openIpcSocket` applies the same rules for `process.platform` before `net.connect` and rejects with a message naming the rule; unit test: a relative path (POSIX) and a non-prefixed name (win32) are rejected with that message, not ENOENT.
11. TRANSPORT_UNIX.md: section 9 states the raw-address rules and that no logical names, resolution or prefixing exist; an address valid on one OS is invalid on the other; `shamir+unix://user@<addr>` is notation only; section 5.1 adds that the parent directory must already exist and is the operator's to restrict (the server neither creates nor chmods it); section 11 lists the validation tests.
12. Correct the false "same string works on both OSes" doc comments in `crates/shamir-client/src/client/options.rs` (~120-123) and `crates/shamir-client-ts/src/core/types/connection.ts` (~86-88).

Compat: no wire/disk change. Deliberate breaking change: server configs with a relative Unix path, a `..` component or a non-`\\.\pipe\` Windows name now fail at boot with a validation error; clients fail fast with `InvalidInput`. Out of scope (other cards of G8): Drop ownership (DD5D7B60) and stale-socket recovery (E0B387B3).


## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
