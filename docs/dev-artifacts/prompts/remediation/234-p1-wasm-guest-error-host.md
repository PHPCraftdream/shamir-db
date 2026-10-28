# 234 — p1-wasm-guest-error-host (P1 follow-up #1531, card RV-SDK-6D2497CA, stage 1 of 2: host side)

Guest-side `shamir_sdk::ErrorKind` categories exist, but every deliberate guest error reaches the host as a trap, and the host maps every trap to `FunctionError::Compute`. Ruling (@oh, 2026-10-28): carry the category through a **terminal host import** `shamir_host::raise_error(kind: i32, msg_ptr: i32, msg_len: i32)`. The host function never returns `Ok` — it always returns `Err(wasmtime::Error::new(FunctionError::…))`, which unwinds the guest (same shape `host_call` already uses: `invoked.map_err(wasmtime::Error::new)`; `map_wasm_error` already downcasts). The result buffer is never inspected for errors, so success cannot be misread, and "error set, then success returned" is impossible. Old guests (no `raise_error` import) are untouched: trap → `Compute` exactly as today. This stage is host-only (`crates/shamir-wasm-host`); the guest SDK/macro stage (235) lands after it.

All paths relative to `crates/shamir-wasm-host/` unless stated. Read the current code first (`src/wasm/wasm_function.rs`, `src/wasm/wasm_sanitizer.rs`, `src/wasm/bindings/`, the `FunctionError` type, existing WAT-based tests and `nested_actor_tests`).

## Changes
1. **New `src/wasm/bindings/raise.rs`:**
   - `pub(crate) const MAX_GUEST_ERROR_MESSAGE_BYTES: usize = 1024;`
   - pure `pub(crate) fn decode_guest_error(kind: i32, bytes: &[u8], declared_len: usize) -> FunctionError`:
     - mapping (exhaustive, no wildcard over known codes):
       | code | result |
       |---|---|
       | 1 (User) | `FunctionError::User(msg)` |
       | 2 (InvalidArgument) | `Compute(format!("guest invalid argument: {msg}"))` |
       | 3 (Codec) | `Compute(format!("guest codec: {msg}"))` |
       | 4 (Host) | `Compute(format!("guest host: {msg}"))` |
       | 0, negative, any other | `Compute(format!("unknown guest error kind {kind}: {msg}"))` |
     - `msg`: `String::from_utf8_lossy(bytes)`, every control char replaced by U+FFFD, cut at a char boundary ≤ 1024 bytes; append ` [truncated]` when `declared_len > MAX_GUEST_ERROR_MESSAGE_BYTES`.
   - `host_raise_error(caller, kind, ptr, len) -> wasmtime::Result<()>` (always `Err`):
     - negative `ptr` or `len` → `Compute("raise_error: negative pointer or length")`;
     - read only `min(len, 1024)` bytes from guest memory with checked arithmetic; out of bounds or missing memory → `Compute(...)` (guest kind dropped);
     - no `unwrap`/`expect`, no allocation larger than the cap driven by the guest.
2. **Layout:** `src/wasm/bindings/` is already at 7 entries (not baselined). Move `src/wasm/bindings/tests/http_decode_tests.rs` to `src/tests/wasm/bindings/http_decode_tests.rs` (create the folder chain with `mod.rs` manifests if missing; check what exists under `src/tests/` first), drop the bindings `tests/` folder and its `mod tests;` line, then add `pub(super) mod raise;` (match the sibling visibility style). It uses `crate::` paths and `pub(crate)` items, so nothing needs widening — verify. The moved test must keep its exact test names (CI's test-discovery gate fails on lost tests).
3. **`src/wasm/wasm_function.rs`:** register `linker.func_wrap("shamir_host", "raise_error", …)` next to the existing host imports; add a `map_wasm_error` downcast arm `FunctionError::User(m) => FunctionError::User(m.clone())` so a nested callee's `User` error propagates intact; document the import in the Guest ABI doc comment. Keep the file under 1000 lines (target < 800).
4. **`src/wasm/wasm_sanitizer.rs`:** add `("shamir_host", "raise_error")` to `SANCTIONED_HOST_IMPORTS`. The existing sanitizer↔linker sync test must stay green.

## Tests (CI runs them — do not run locally)
New `src/tests/wasm/bindings/raise_error_tests.rs`, registered in that folder's `mod.rs`, using hand-written WAT guests (pattern of the existing WAT tests; no guest toolchain):
- `rv_sdk_6d2497ca_raise_user_maps_to_user`
- `rv_sdk_6d2497ca_codec_host_invalid_arg_map_to_compute_with_prefix`
- `rv_sdk_6d2497ca_unknown_kind_compute` (kinds 0, 99, −1)
- `rv_sdk_6d2497ca_old_guest_unreachable_stays_compute` (no import)
- `rv_sdk_6d2497ca_import_unused_success_untouched` (imports `raise_error`, returns a valid result)
- `rv_sdk_6d2497ca_raise_then_success_code_never_runs` (`call raise_error` then an `i64.const` packed result; expect `Err(User)`)
- `rv_sdk_6d2497ca_oversized_len_truncated_to_cap`
- `rv_sdk_6d2497ca_prefix_oob_compute`
- `rv_sdk_6d2497ca_negative_ptr_len_compute`
- `rv_sdk_6d2497ca_invalid_utf8_and_control_chars_replaced`
- `rv_sdk_6d2497ca_nested_child_user_propagates` (pattern from `nested_actor_tests`)
Plus unit tests of `decode_guest_error` (cap boundary at exactly 1024 / 1025 bytes, multi-byte char straddling the cap). Existing tests that expect `shamir_call trap` / `Compute` must pass unchanged — do not edit their expectations.

## Rules
- Do not touch `crates/shamir-sdk*` or `crates/shamir-db` (stage 235 owns them).
- Layout: every folder ≤ 7 entries, every file ≤ 1000 lines; `mod.rs` re-exports only; tests in `tests/` dirs, `tests/mod.rs` manifest only; imports at file top; one file = one primary export.
- No new `#[allow]`, no visibility widening beyond what the new items need, no dependency or version changes.
- Comments laconic.

## Local gates (only these; one crate at a time; never --all-features)
- `cargo fmt -p shamir-wasm-host -- --check`
- `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW and MOVED file path and the real exit codes.
