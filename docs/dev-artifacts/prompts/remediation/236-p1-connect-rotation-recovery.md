# 236 — p1-connect-rotation-recovery (P1 follow-up #1530, card F20C09BF, wave 1 / S1: shamir-connect)

Context (@oh ruling, 2026-10-28, against master f68b2749): the client side already decodes an 8- or 10-field `auth_ok` (`shamir-client/src/client/transport.rs` `WireAuthOk`, slots 8/9 `#[serde(default)]`) and exposes `Client::rotation_in_progress()` / `kdf_upgrade_required()`, but the server never emits hints (`shamir-connect/src/server/auth/handshake.rs` ~L253 leaves both `None`) and a client with an old pin stops at `ServerIdentityChanged` with no recovery path. Spec §6.5: re-pinning is NEVER automatic; recovery must keep strict signatures, time bounds and explicit approval, failing closed when nobody approves. This stage adds the two shamir-connect primitives later stages consume (S4 server emission, S5 client recovery). It changes no wire format.

All paths relative to `crates/shamir-connect/`. Read first: `src/client/handshake.rs`, the client rotation helpers (`verify_rotation_in_progress`, `build_rotation_in_progress_payload`, `build_identity_input`, `AuthMessage::build`), `src/server/auth/handshake.rs` (`ServerHandshake`, `AuthOkView`, `identity_keypair_for_verify`), `ServerIdentityState`, the existing rotation tests.

## Changes
1. New `src/client/rotation.rs` (check `src/client/` entry count first; if it would exceed 7 entries, put the struct next to the existing rotation code instead and say so): `pub struct RotationRecovery { pub previous_pin: [u8; 32], pub new_pin: [u8; 32], pub transition_until_ns: u64 }`, re-exported where the client handshake types are.
2. `src/client/handshake.rs`: `pub fn verify_rotation_recovery(&self, ok: &ServerAuthOk, keys: &DerivedKeys, auth_message: &AuthMessage, now_ns: u64) -> Result<RotationRecovery>` (use the crate's real type names). Checks in this order, each failure a typed error from the existing error enum (no panics, no `unwrap`):
   1. SCRAM `server_signature` matches;
   2. a pin is set and differs from `sha256(current_pub)`;
   3. a rotation payload is present;
   4. `identity_sig` verifies strictly against `current_pub`;
   5. `verify_rotation_in_progress(.., true)` passes (old-pin match, both previous-key signatures, window `(now, now + 7d + 1h]`).
   It only verifies and returns; it never mutates a pin.
3. `src/server/auth/handshake.rs`: `pub fn rotation_in_progress_payload(&self, view: &AuthOkView, state: &ServerIdentityState) -> Result<Option<RotationInProgressPayload>>` — `None` outside the overlap window, or when `state.current_pub() != view.server_pub_key` (the boot-time keypair copy may diverge); otherwise rebuild the private auth_message + identity_input and call `build_rotation_in_progress_payload`.

## Tests (CI runs them — never locally)
New `tests/auth/rv_client_f20c09bf_rotation_recovery.rs`, registered as a `[[test]]` target in `Cargo.toml` (`[[test]]` entry only — no dependency changes; follow how other `tests/auth/*` targets are registered). Cover every verify branch (bad server signature, no pin, pin equals current, missing payload, bad identity_sig, tampered `rotation_proof`, tampered `identity_sig_previous`, window beyond 7d+1h, window in the past, happy path returning `new_pin == sha256(current_pub)`), plus the server helper's `None` cases (outside window, pub-key mismatch) and its `Some` case verifying with `verify_rotation_in_progress`.

## Rules
- Touch only `crates/shamir-connect/` (other stages own shamir-client and shamir-server).
- Layout: folders ≤ 7 entries, files ≤ 1000 lines (do not grow an already-over-limit file; split along seams if needed); `mod.rs` re-exports only; imports at file top; one file = one primary export; no new `#[allow]`; secrets zeroized as the surrounding code does.
- Comments laconic.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-connect -- --check`
- `cargo clippy -p shamir-connect --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW file path, the public API added (exact signatures) and the real exit codes.
