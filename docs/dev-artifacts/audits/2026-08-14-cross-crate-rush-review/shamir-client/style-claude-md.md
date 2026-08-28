# shamir-client -- Style & CLAUDE.md structural conformance

## Summary

The crate is in strong conformance with CLAUDE.md's structural rules: `lib.rs` and
`src/tests/mod.rs` are re-export/manifest-only, there are zero inline
`#[cfg(test)] mod tests` blocks (all unit tests live in the `src/tests/` directory,
integration tests in the crate-root `tests/`), every file holds one primary export
or a genuinely closely-coupled group, and the builder-only query rule is respected
(no `serde_json` anywhere in the crate). The findings below are confined to four
mid-function `use` statements that violate the "Imports at the top" rule and one
stale, never-compiled doc example that has drifted out of sync with
`ConnectOptions` under the project-wide `doctest = false` ban.

## Findings

### 1. Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled
- **File:line:** `crates/shamir-client/src/lib.rs:10-17` (vs `crates/shamir-client/src/client.rs:54-89`, `crates/shamir-client/Cargo.toml:48-51`)
- **Severity:** low
- **Issue:** The `//!` illustration constructs `ConnectOptions` with six fields
  (`addr`, `server_name`, `username`, `password`, `accept_new_host`, `trusted_pin`),
  but the struct now has eight — `connect_timeout` (client.rs:81) and
  `request_timeout` (client.rs:88) were added and are absent from the example.
  `Cargo.toml` sets `[lib] doctest = false` ("Doctests are banned project-wide"),
  so the fenced `no_run` example is never type-checked and the drift is silent.
  The `Cargo.toml` comment explicitly sanctions keeping examples "as
  illustration", so the example itself is conforming — but an illustration that
  fails `E0063` if ever pasted defeats its purpose.
- **Failure scenario:** A user copies the documented connect snippet verbatim and
  gets a missing-field compile error; nobody on the maintenance side is ever
  alerted, because the banned-doctest setup guarantees the block is never built.
- **Suggested fix:** Add `connect_timeout: None, request_timeout: None` to the
  example's struct literal (or split the illustration so it only shows the stable
  core fields with a prose note that two optional timeout knobs exist). Keep the
  `no_run` fence — it stays uncompilable-by-design but should at least be
  field-accurate.

### 2. `use rand::RngCore;` inside the body of `Client::resume`
- **File:line:** `crates/shamir-client/src/client.rs:608-611`
- **Severity:** low
- **Issue:** CLAUDE.md "📦 Imports at the top" requires every `use` to live in the
  file header, with only three documented exceptions. None applies here: there is
  no `RngCore` name collision in scope (nothing else from `rand` is imported, and
  `rand` appears nowhere else in the file), there is no one-line comment stating a
  collision, and the block is not macro-generated or `cfg`-gated. The scoped
  `use` sits in an artificial `{}` block purely to limit trait scope.
- **Failure scenario:** None functional — pure style-conformance drift that the
  documented rule is meant to prevent (hidden mid-body dependency edges).
- **Suggested fix:** Hoist `use rand::RngCore;` to the import header next to the
  other external-crate imports and delete the enclosing braces.

### 3. Mid-body `use` statements in three `src/tests/` files
- **File:line:** `crates/shamir-client/src/tests/batch_has_refs_tests.rs:18` (`use shamir_query_types::read::ReadQuery;` inside helper `read_op`); `crates/shamir-client/src/tests/demux_tests.rs:408` (`use crate::subscription::SubscriptionHandle;` inside `subscription_handle_drop_removes_from_registry`); `crates/shamir-client/src/tests/wire_version_tests.rs:137` (`use std::sync::atomic::{AtomicU8, Ordering};` inside `atomic_u8_plumbing_stores_and_reads_correctly`)
- **Severity:** nit
- **Issue:** Same "Imports at the top" rule as finding 2. The `use super::*;`
  exception covers inline `#[cfg(test)] mod tests` blocks, not files in a
  `tests/` directory (which this crate correctly uses instead — so these files
  are ordinary modules and must keep imports in their headers). None of the three
  has a collision or a collision comment; all three hoist trivially
  (`batch_has_refs_tests.rs` already imports from `shamir_query_types::read`,
  so `ReadQuery` just joins that group).
- **Failure scenario:** None functional; each is a small, mechanical violation of
  the documented header-import rule in test code.
- **Suggested fix:** Move all three imports to the top of their files and delete
  the surrounding braces (`wire_version_tests.rs`'s import can merge into a
  header group with the other `std` imports).

### Non-findings (checked and conforming, for the record)
- `lib.rs` and `src/tests/mod.rs`: re-exports / `pub mod` manifest only — no
  logic, per "mod.rs files contain re-exports only" and test-organisation rule 3.
- `#[cfg(test)] mod tests;` wired only from the crate root (`lib.rs:38-39`);
  zero inline test modules in implementation files (rule 5).
- One-file-one-export: `client.rs` (`Client` + its constructor option structs and
  `pub(crate)` demux plumbing — one closely-coupled group), `wire_frames.rs`
  (five wire-mirror frames), `interner_cache.rs` (`FieldMap` + its registry),
  `subscription.rs`, `cursor_stream.rs`, `interner_cache_ops.rs` (an inherent
  `impl Client` extension with private helpers) — all fit the "closely-coupled
  group" allowance; no unrelated public types share a file.
- Builder-only query construction: zero `serde_json` / `json!` usage in the
  crate; every request is built via `shamir_query_builder` (no undocumented
  exception comments needed).
- Test coverage claims in module docs (e.g. `cursor_stream_tests.rs:1-11`
  justifying why it lives under `src/tests/` to reach `pub(crate)`
  `Client::roundtrip`) match the tests actually present (demux, timeouts, wire
  versioning, v2 passthrough, interner cache, ambient sync, `batch_has_refs`
  regression, resume wire roundtrip, cursor close/cancel, plus seven
  crate-root e2e files).
