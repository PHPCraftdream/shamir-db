# 238 — p1-client-scratch-zeroize-test (P1 follow-up #1530, H3 review P3a, wave 1 / S3: shamir-client)

The existing scratch-zeroization test (`crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs` ~L147-167 — verify the path) cannot tell `zeroize()` from `clear()`: `clear()` only sets len = 0, leaving password bytes in the buffer's spare capacity. Add a test that fails if the post-send wipe of the request scratch buffer were downgraded to `clear()`.

## Changes
1. `crates/shamir-client/src/client/api.rs` (or wherever the request scratch buffer lives — find the buffer that holds the serialized secret-bearing request and the `zeroize()` call after send): add `#[cfg(test)] pub(crate) fn` helpers (items, not an inline test module; no production behaviour change):
   - one that fills the scratch buffer's full capacity (4096 or its real capacity) with `0xA5`, then restores the length the production code expects;
   - one that asserts capacity is unchanged and returns all capacity bytes. Reading spare capacity needs one `unsafe { std::slice::from_raw_parts(ptr, cap) }` with a `// SAFETY:` comment: sound because the fill helper initialised every byte of the allocation and capacity did not change.
2. `resume_and_scratch_tests.rs`: add `rv_client_150f9230_request_scratch_capacity_wiped_after_secret_send` — fill, drive a secret-bearing send through the same path the existing test uses, then assert every capacity byte is zero and the password byte substring is absent. Keep the existing test unchanged.

## Rules
- Touch only `crates/shamir-client/src/client/api.rs` (test helpers only) and the test file.
- No new `#[allow]`; check the crate's lints for an `unsafe_code` policy — if the crate forbids unsafe code, stop and report instead of working around it. Files ≤ 1000 lines; imports at top.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-client -- --check`
- `cargo clippy -p shamir-client --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report the real exit codes.
