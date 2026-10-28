# 241 — p1-authok-hints-e2e (P1 follow-up #1530, cards F20C09BF / 2D0142DD, wave 3 / S6: real-server end-to-end)

Context (@oh ruling 2026-10-28). This branch stacks every earlier #1530 stage (not yet on master): S1 shamir-connect `verify_rotation_recovery` + `ServerHandshake::rotation_in_progress_payload`; S2 tolerant test mirrors; S4 the server emits auth_ok hints (8 fields without hints, 10 with: slot 8 rotation payload or nil, slot 9 `kdf_upgrade_required` or nil); S5 shamir-client returns `ClientError::IdentityRotationPending { previous_pin, new_pin, transition_until_ns }` when a supplied pin is the pre-rotation key and the rotation chain verifies (reconnect with `trusted_pin: Some(new_pin)` approves). Unit/fake-server coverage exists; this stage adds the end-to-end proof against a REAL launched `shamir-server`.

Read first: an existing shamir-server e2e test that launches a server and connects with `shamir_client` (look under `crates/shamir-server/tests/e2e/`, `tests/admin/`, `tests/common/`), the server boot config (`crates/shamir-server/src/boot/config.rs` `kdf_defaults: KdfConfig`), `crates/shamir-server/src/engine/server_meta.rs` (`ServerMetaStore::store_identity_after_rotate`) and its test `tests/misc/runtime/server_meta.rs`, and how the server loads its identity at boot.

## Tests (CI runs them — never locally)
New `crates/shamir-server/tests/<area>/auth_ok_hints_e2e.rs` (pick the area folder that already holds client-handshake e2e tests and register it the way its siblings are registered — a `[[test]]` target in `crates/shamir-server/Cargo.toml` or a `mod` line in the area's test root; keep every folder ≤ 7 entries, files ≤ 1000 lines):
1. `rv_client_f20c09bf_kdf_upgrade_hint_e2e`: create a user under weak (fast) KDF params, stop, relaunch the server on the same data dir with stronger `kdf_defaults`, log in with `shamir_client` → `client.kdf_upgrade_required()` is true; a fresh user created under the strong params sees false. Keep KDF params small enough for CI (look at how existing tests pick fast KDF).
2. `rv_client_f20c09bf_rotation_pending_e2e`: launch, connect once and record the pin (`sha256(server_pub)`), stop; between launches rotate the identity with `ServerMetaStore::store_identity_after_rotate` (overlap window inside the 7d+1h bound); relaunch; connecting with the OLD pin → `Err(ClientError::IdentityRotationPending { previous_pin == old pin, new_pin == sha256(new pub), .. })`; reconnecting with `trusted_pin: Some(new_pin)` → `Ok`; connecting with an unrelated pin → the existing server-identity-changed error.
If the server boot path cannot pick up a rotated identity without a production change, STOP and report exactly why (do not change production code in this stage).

## Rules
- Touch only `crates/shamir-server/tests/**` and, if needed, the `[[test]]` registration in `crates/shamir-server/Cargo.toml`.
- Unix-only transports must be `#[cfg(unix)]`-gated exactly like sibling tests; prefer TCP/whatever existing e2e tests use so the test also runs on Windows CI.
- No sleeps as synchronisation — use the readiness helpers sibling tests use. No new `#[allow]`; imports at file top.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-server -- --check`
- `cargo clippy -p shamir-server --all-targets --features shamir-server/test-util -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW file path and the real exit codes.
