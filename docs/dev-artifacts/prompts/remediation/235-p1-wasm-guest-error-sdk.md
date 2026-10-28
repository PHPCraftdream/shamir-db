# 235 — p1-wasm-guest-error-sdk (P1 follow-up #1531, cards RV-SDK-6D2497CA / RV-SDK-MACROS-BB6378E2 / D7F6AEC5, stage 2 of 2: guest side)

Ruling (@oh, 2026-10-28): the guest reports deliberate errors through a **terminal host import** `shamir_host::raise_error(kind: i32, msg_ptr: i32, msg_len: i32)` instead of trapping. Stage 234 (separate branch, lands first) adds the host side: the import never returns, maps code 1 → `FunctionError::User(msg)`, 2/3/4 → `Compute("guest invalid argument|codec|host: msg")`, anything else → `Compute("unknown guest error kind …")`, caps messages at 1024 bytes and sanitises them. Old guests that only trap stay `Compute`. Under `panic=abort` the current panic text very likely never reaches the host, so `raise_error` also carries the message (D7F6AEC5). This stage changes the guest SDK and macros and adds an end-to-end test. Do NOT touch `crates/shamir-wasm-host` — reference the host contract above only.

Read first: `crates/shamir-sdk/src/error.rs` (`Error`, `ErrorKind`), `crates/shamir-sdk/src/runtime/host_imports.rs`, `crates/shamir-sdk/src/runtime/__rt.rs` (`trap`, `sanitize_trap_message`), `crates/shamir-sdk-macros/src/lib.rs` (all four `expand_*` functions, ~L302 `trap(&e.to_string())`), the existing tests of both crates, `crates/shamir-db/tests/functions/` and its `compile_or_skip` helper.

## Changes
1. `crates/shamir-sdk/src/error.rs`: `pub(crate) fn abi_code(self) -> i32` on `ErrorKind` (or `pub` with `#[doc(hidden)]` if macro-expanded code in user crates needs it — prefer keeping it crate-private by having `__rt::raise` call it): `User = 1`, `InvalidArgument = 2`, `Codec = 3`, `Host = 4`. Exhaustive `match`, no wildcard, so adding a kind breaks the build.
2. `crates/shamir-sdk/src/runtime/host_imports.rs`: declare `fn raise_error(kind: i32, ptr: i32, len: i32);` in the `shamir_host` import block (returns `()`, not `-> !`); the caller follows it with `core::arch::wasm32::unreachable()` in case a host ever returns.
3. `crates/shamir-sdk/src/runtime/__rt.rs`: `pub fn raise(e: &Error) -> !` — reuse `sanitize_trap_message` for the text; on `wasm32` call the import then `unreachable()`; on non-wasm targets `panic!("shamir function error: {kind:?}: {msg}")`. Keep `trap` for ABI violations (e.g. `invalid shamir_call pointer or length`).
4. `crates/shamir-sdk-macros/src/lib.rs`: in all four expansions every `trap(&e.to_string())` error arm becomes `shamir_sdk::__rt::raise(&e)`; the validator's missing-`record` case becomes `raise(&Error::codec(..))` (use the existing constructor name); delete the `TODO(slice 4)` and fix the doc lines that say deliberate errors trap → Compute.

## Tests (CI runs them — do not run locally)
- `crates/shamir-sdk/src/tests/runtime_tests.rs`: add `rv_sdk_6d2497ca_abi_codes_pinned` (asserts 1–4).
- New `crates/shamir-sdk-macros/src/tests/raise_path_tests.rs`, registered in its `tests/mod.rs`: `rv_sdk_macros_bb6378e2_err_arms_use_raise` — stringify each `expand_*` output and assert `__rt::raise` is present and `trap(&e.to_string())` is absent (follow how existing macro tests call the expanders).
- New `crates/shamir-db/tests/functions/guest_error_kind_e2e.rs`, registered in the folder's test root (`main.rs` or `mod.rs` — check): compile a guest from source via `compile_or_skip`; a function returning `Err(Error::user("nope"))` must yield an error whose message contains `user error: nope`; one returning `Error::codec(..)` must yield `compute error`. Check the folder stays ≤ 7 entries.

## Rules
- Layout: every folder ≤ 7 entries, every file ≤ 1000 lines; `mod.rs` re-exports only; tests in `tests/` dirs; imports at file top; one file = one primary export.
- No new `#[allow]`, no visibility widening beyond what's needed, no dependency or version changes.
- The guest import block may be `cfg(target_arch = "wasm32")`-gated; this host compiles only the native path — keep the wasm32 body simple and obviously correct.
- Comments laconic.

## Local gates (only these; one crate at a time; never --all-features)
- `cargo fmt -p shamir-sdk -p shamir-sdk-macros -p shamir-db -- --check`
- `cargo clippy -p shamir-sdk --all-targets -- -D warnings`
- `cargo clippy -p shamir-sdk-macros --all-targets -- -D warnings`
- `cargo clippy -p shamir-db --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW file path and the real exit codes.
