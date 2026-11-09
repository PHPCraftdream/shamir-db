# Brief: oracle test, guest `BatchRequest` against the native query builder

## Why

The `shamir` language (`crates/wasm/shamir-lang`) lets a procedure build a batch
(`batch`, `batch_id`, `batch_read`, `batch_insert`, `request`, `db_execute`). The
guest encodes the `BatchRequest` DTO itself, in WASM, with the shared guest
encoder. The design (`docs/dev-artifacts/design/shamir-lang-design.md`, sections 4
and 9) requires that this request equals what the native
`shamir_query_builder::batch::Batch` produces for the same logical batch, so the
server never sees two dialects of one request.

Today `crates/wasm/shamir-wasm-host/src/tests/runtime/lang/batch_tests.rs`
compares the guest request with a hand-written `QueryValue` expectation
(`plan`, `map_of`, `select_all`, `default_limits`). That is a second copy of the
format, not an oracle: if the native builder changes, the test still passes. The
guest key order of `queries` is the insertion order since PR "keep the insertion
order of the batch queries" (the native `Batch` keeps insertion order too).

## What

Add oracle tests that build the same logical batch twice, once through a compiled
`shamir` procedure (the request recorded by the existing `RecordingDb` gateway) and
once through `shamir_query_builder::batch::Batch` (`Batch::new`, `id`, the typed
read/insert helpers, `to_request_via_msgpack`), and assert equality:

1. Primary assertion: the two requests are identical after decoding to
   `QueryValue` (field by field, including `limits`, `result_encoding`,
   `transactional`, `return_all`, key order of `queries`).
2. Where the native side can produce raw bytes (find the msgpack writer the
   server uses for `BatchRequest`, for example the one behind
   `to_request_via_msgpack` or `shamir_types` `QueryValue::to_bytes`), also
   assert the bytes are equal. If bytes legitimately differ while the decoded
   values are equal (for example an integer width), do not weaken (1): state the
   difference in a comment and in your final message.
3. Cover: `batch_read` with a record filter (two fields), a scalar key filter, a
   null filter; `batch_insert`; `batch_id`; several aliases in one batch (order
   check); an alias that sorts before an earlier one.
4. Keep the old hand-written expectations only where they test something the
   oracle does not; otherwise replace them. No test may be lost silently: list
   removed/renamed tests in the final message.

Where the tests live: `shamir-wasm-host` needs `shamir-query-builder` as a
`[dev-dependencies]` entry (path dependency, no new external crate, check that no
dependency cycle appears: the builder must not depend on `shamir-wasm-host`). If a
cycle or a lockfile problem makes that impossible, put the oracle in
`crates/engine/shamir-db/tests/functions/gateway/` instead, with a recording
gateway wrapper there, and say why in the final message.

## Findings are the product

If the guest and the native request differ in a way that is not a test artefact,
do NOT change the guest runtime or the builder in this task. Write the minimal
failing assertion, keep it (it must be an honest red test only if you cannot
express the difference as a documented, asserted deviation), and report the exact
difference (field, bytes) in the final message. The orchestrator decides the
fix. Prefer asserting a documented deviation with a clear comment over leaving a
red test.

## Observable acceptance

1. The oracle tests exist and compare against `Batch` output (no hand-written
   expectation for the compared shapes).
2. `cargo fmt -p shamir-wasm-host -- --check`, `cargo check -p shamir-wasm-host
   --all-targets`, `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`
   are clean (and the same for `shamir-db` when it is touched).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder,
   at most 1000 lines per file; split into a new test file if needed, wire it in
   the folder's `mod.rs`).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no
   local `use`, no new `#[allow]`, no visibility widening of existing items.

## Non-goals

No change to `shamir-lang`, the guest runtime, the builder or the server. No new
language features. No version change.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git. Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --`. No `cargo build/test/run/bench/doc`, no
nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
