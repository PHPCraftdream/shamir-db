# 237 — p1-server-authok-test-mirrors (P1 follow-up #1530, wave 1 / S2: shamir-server test decoders)

Context (@oh ruling, 2026-10-28): the server will soon (stage S4) send `auth_ok` as either the current 8-field positional msgpack array or, when a hint is present, 10 fields (slot 8 = rotation-in-progress payload or nil, slot 9 = kdf_upgrade_required flag or nil). The earlier attempt broke CI (`LengthMismatch(8)` in every e2e test, reverted in 71e19f85) because the test-side mirror structs in `crates/shamir-server/tests/**` decode exactly 8 fields. This stage makes every such mirror tolerant of 8 OR 10 fields, ahead of S4, without changing any assertion.

## Changes
Find every test-only struct in `crates/shamir-server/tests/**` that mirrors the auth_ok array (grep `AuthOk`; candidate files: `admin/identity/permission_e2e/common.rs`, `admin/ops/backup_restore_e2e/common.rs`, `admin/ops/backup_restore_e2e/restore_cli.rs`, `connection/resume_fast_path.rs`, `e2e/core/mvp_e2e.rs`, `e2e/core/mvp_ws_e2e.rs`, `e2e/core/persistence_e2e.rs`, `e2e/transport/ipc_e2e.rs`, `e2e/transport/ipc_resume_e2e.rs`, `e2e/transport/plain_tcp_e2e.rs`, `e2e/transport/rv_connect_0a42f42c_capped_login_reject_e2e.rs`, `repl/repl_pull_e2e.rs` — verify each; ~11 structs).

For each, append two trailing fields mirroring `crates/shamir-client/src/client/transport.rs` `WireAuthOk` slots 8/9 — read it and copy the exact derive form, types and serde attributes (e.g. `#[serde(default)]` Option fields). Unused fields: prefix with `_` (or use `serde::de::IgnoredAny`-compatible types) so no `dead_code` lint fires and no `#[allow]` is needed. Confirm the client's `WireAuthOk` derive form is what makes a shorter (8-element) array decode, and mirror exactly that form.

Do NOT touch `crates/shamir-transport-tcp/tests` (those decode their own in-test server). Do not change any test assertion, test name or production code.

## Rules
- Touch only `crates/shamir-server/tests/**`.
- No new `#[allow]`, imports at file top, files ≤ 1000 lines (do not grow an over-limit file beyond its baseline — check `scripts/ci/layout_baseline.txt` for each touched file).

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-server -- --check`
- `cargo clippy -p shamir-server --all-targets --features shamir-server/test-util -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every touched file and struct and the real exit codes.
