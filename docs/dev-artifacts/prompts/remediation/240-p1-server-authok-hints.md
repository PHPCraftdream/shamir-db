# 240 — p1-server-authok-hints (P1 follow-up #1530, cards F20C09BF / 2D0142DD, wave 2 / S4: shamir-server emission)

Context (@oh ruling, 2026-10-28). The server's `auth_ok` is a positional msgpack array of 8 fields (`crates/shamir-server/src/sync/connection/session/wire.rs` `AuthOk`). The client (`shamir-client` `WireAuthOk`) already decodes 8 or 10 fields (slots 8/9 `#[serde(default)]`): slot 8 = rotation-in-progress payload, slot 9 = `kdf_upgrade_required`. This branch already contains (stacked, not yet on master):
- S1 (PR #375): `ServerHandshake::rotation_in_progress_payload(&self, view: &AuthOkView, state: &ServerIdentityState) -> Result<Option<RotationInProgressPayload>>` in `crates/shamir-connect/src/server/auth/handshake.rs` (None outside the overlap window or on pub-key mismatch).
- S2 (PR #377): every test-side `WireAuthOk` mirror in `crates/shamir-server/tests/**` tolerates 8|10 fields.

Wire rule (chosen, do not change): **no hints → exactly today's 8 fields, byte for byte**; if either hint is present → 10 fields: slot 8 = rotation payload or `nil`, slot 9 = `true` or `nil`. Rejected alternatives (do not use): map encoding (TS client and positional decoders reject it), `auth_init.version` bump, extra `auth_init` field, post-login fetch. Plain `skip_serializing_if` is wrong: with rotation absent and the KDF flag present the flag would land in slot 8.

Read first: `session/wire.rs` (`AuthOk`, how it is serialized and sent), the auth_ok build site in `crates/shamir-server/src/sync/connection/session/handshake.rs` (~946 lines — keep the call-site edit small), `ServerIdentityState` access from the session, `needs_kdf_upgrade` and `hs.effective_kdf_params()` (find them; if names differ, use the real equivalents and report), `docs/**/AUTH_PROTOCOL.md` §2.4.

## Changes
1. `session/wire.rs`: hand-written `impl Serialize for AuthOk` using `serializer.serialize_struct("AuthOk", if hints { 10 } else { 8 })` (rmp_serde encodes structs as arrays in this codebase — verify the serializer config used for auth_ok; byte compatibility with the current derive must hold for the 8-field case), `serde_bytes::Bytes` for byte fields exactly as the derive did (check current attributes), then `rotation_in_progress` / `kdf_upgrade_required` as `Option` (None → nil) only in the 10-field case. Keep `#[derive(Deserialize)]` with the two new fields `#[serde(default)]`.
2. New `session/auth_ok_hints.rs` (the folder goes to 7 entries — verify it does not exceed 7; otherwise report and place it next to the nearest owner): pure helper computing `(Option<RotationInProgressPayload>, Option<bool>)` from the handshake: `kdf_upgrade_required = needs_kdf_upgrade(hs.effective_kdf_params(), kdf).then_some(true)`; rotation payload via S1's `rotation_in_progress_payload`. Errors from the payload builder must not fail login — log at debug/warn without secrets and emit no rotation hint (fail closed = no hint).
3. Call it from the auth_ok build site; minimal edit.
4. `AUTH_PROTOCOL.md` §2.4: short note documenting the 8|10 rule, slot meanings and that pre-#364 strict 8-field Rust decoders cannot decode hint-bearing replies (owner decision: accepted, `shamir-client` is `publish = false` alpha).

## Tests (CI runs them — never locally)
New `crates/shamir-server/src/tests/engine/auth_ok_hint_tests.rs` registered in `src/tests/engine/mod.rs` (verify path/convention):
- no hints: encoded array length 8 (count via `Vec<serde::de::IgnoredAny>`) and bytes identical to the encoding of an equivalent 8-field derive-mirror struct defined in the test;
- KDF flag only: length 10, slot 8 nil, slot 9 true;
- rotation only: length 10, slot 9 nil; decodes into `shamir_client`'s shape if accessible, otherwise into a local 10-field mirror;
- helper: KDF flag only when stored params are weaker than effective; rotation payload present during overlap and verifies with `shamir_connect`'s `verify_rotation_in_progress`.

## Rules
- Touch only `crates/shamir-server/src/**` and the protocol doc (S1/S2 already landed their parts on this branch; do not edit them).
- Layout: folders ≤ 7 entries, files ≤ 1000 lines (do not grow baselined files — `handshake.rs` must not grow past its baseline; move code out if needed), `mod.rs` re-exports only, imports at file top, one file = one primary export, no new `#[allow]`, no `unwrap`/`expect` in production code, no secrets in logs. Comments laconic.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-server -- --check`
- `cargo clippy -p shamir-server --all-targets --features shamir-server/test-util -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW file path and the real exit codes.
