# 239 — p1-client-rotation-pending (P1 follow-up #1530, card F20C09BF, wave 2 / S5: shamir-client)

Context (@oh ruling, 2026-10-28): stage S1 (brief 236, PR #375 — this branch is stacked on it) added `ClientHandshake::verify_rotation_recovery(&self, ok: &ServerAuthOk, keys: &DerivedKeys, auth_message: &AuthMessage, now_ns: u64) -> Result<RotationRecovery>` (verify-only; `RotationRecovery { previous_pin, new_pin, transition_until_ns }`, re-exported from `shamir_connect::client`) and `ServerHandshake::rotation_in_progress_payload(&self, &AuthOkView, &ServerIdentityState) -> Result<Option<RotationInProgressPayload>>`. Today `shamir-client` stops at `ServerIdentityChanged` when the server identity differs from the pin, with no recovery path. Spec §6.5: re-pinning is NEVER automatic; with nobody to approve, fail closed. Approval model chosen: **reconnect-to-approve** — the client surfaces a verified pending rotation as an error; the caller persists `new_pin` out-of-band and reconnects with `trusted_pin: Some(new_pin)`. No new option fields: `ConnectOptions` / `ConnectLocalOptions` are public struct literals with no `Default`, so a new field would break every literal in the workspace.

All paths relative to `crates/shamir-client/`. Read first: `src/client/connect.rs` (both auth_ok paths ~L402-459 and ~L587-636), `src/client/transport.rs` `WireAuthOk` (8|10 fields), `src/client/api.rs` accessors `rotation_in_progress()` / `kdf_upgrade_required()`, `src/error.rs` (`ClientError`), `src/tests/auth/` and its `mod.rs`, S1's API in `crates/shamir-connect/src/client/handshake.rs`.

## Changes
1. `src/error.rs`: new variant `ClientError::IdentityRotationPending { previous_pin: [u8; 32], new_pin: [u8; 32], transition_until_ns: u64 }` with a clear `Display` (hex pins are fine; no secrets). Update any exhaustive matches / error-code mapping in the crate (and in workspace crates that match `ClientError` exhaustively — grep; if one exists outside shamir-client, update it minimally and report it).
2. `src/client/connect.rs`: the 8-field-decode → `process_auth_ok` → pin step is duplicated in the two connect paths — move it into ONE private function returning `SessionMeta` (or the crate's equivalent). In it: when `process_auth_ok` returns `ServerIdentityChanged` AND a pin was supplied AND the auth_ok carries a rotation payload, call `verify_rotation_recovery(.., now_ns)`; on `Ok(r)` drop the connection and return `IdentityRotationPending { .. }` from `r`; on verification failure return the crate's `Handshake` error (never the pending variant). Without a payload or without a pin the behaviour stays exactly as today (`server_identity_changed`). This must shrink `connect.rs` (874 lines; must not grow). `src/client/` and `src/` are at the 7-entry limit — add no new file there.
3. Update the field doc at `connect.rs` ~L113 that says the client "does not currently expose" rotation recovery.

## Tests (CI runs them — never locally)
New `src/tests/auth/auth_ok_hints_tests.rs`, registered in `src/tests/auth/mod.rs`: a fake server over `IpcListener` (Unix transport, no channel binding — gate the file or tests with `#[cfg(unix)]` if the IPC transport requires it; check how existing client tests drive a fake server) running the real `ServerHandshake` SCRAM and driving the public `Client::connect_local`. Build the rotation payload with `AuthMessage::build` + `build_identity_input` + `build_rotation_in_progress_payload` (all on master) or S1's `rotation_in_progress_payload`. Cases:
- first-use connect (no pin) with both hints: both accessors set, pin is `sha256(current)`;
- 8-field reply: `None` / `false`;
- old pin + valid payload: `IdentityRotationPending` with `new_pin == sha256(current)`;
- tampered `rotation_proof`, tampered `identity_sig_previous`, window beyond 7d+1h: `Handshake`, never pending;
- unrelated pin: `server_identity_changed`;
- reconnect with `new_pin`: `Ok`.

## Rules
- Touch only `crates/shamir-client/` (plus a minimal exhaustive-match fix elsewhere if the compiler forces it — report it).
- Layout: folders ≤ 7 entries, files ≤ 1000 lines (do not grow a baselined file), `mod.rs` re-exports only, imports at file top, one file = one primary export, no new `#[allow]`, secrets zeroized as surrounding code does. Comments laconic.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-client -- --check`
- `cargo clippy -p shamir-client --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Note: Unix-only test code is not type-checked on this Windows host — keep it simple and mirror existing Unix-gated client tests exactly.
Report every NEW file path and the real exit codes.
