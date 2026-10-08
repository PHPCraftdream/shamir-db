<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-client — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The stale example and four body-local imports remain. Structural layout conforms. The broad module-doc coverage assurance has concrete counter-evidence rather than merely insufficient proof.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 3 | 0 | 0 | 1 | 0 | 4 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

ConnectOptions requires eight fields but the disabled-doctest illustration contains six. Adding explicit None fields repairs this mismatch without permissive defaults.

Evidence: [crates/shamir-client/src/lib.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/lib.rs#L10); [crates/shamir-client/src/client.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L85); [crates/shamir-client/Cargo.toml:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/Cargo.toml#L52).

Grouping/duplicate: [SUMMARY.md#7.1](SUMMARY.md#review-7-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — `use rand::RngCore;` inside the body of `Client::resume`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

An ordinary block-local import lacks any collision/cfg exception. This is source-style drift with no runtime defect.

Evidence: [crates/shamir-client/src/client.rs:883](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L883).

Grouping/duplicate: [SUMMARY.md#7.2](SUMMARY.md#review-7-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Mid-body `use` statements in three `src/tests/` files

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All three remain in ordinary test/helper bodies; no documented exception applies.

Evidence: [crates/shamir-client/src/tests/batch_has_refs_tests.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/batch_has_refs_tests.rs#L18); [crates/shamir-client/src/tests/demux_tests.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L408); [crates/shamir-client/src/tests/wire_version_tests.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L137).

Grouping/duplicate: [SUMMARY.md#7.3](SUMMARY.md#review-7-3). This is not an additional independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — `lib.rs` and `src/tests/mod.rs` are re-export/manifest-only

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The root declares modules and reexports; tests/mod.rs declares the nine topic modules. Neither embeds implementation logic.

Evidence: [crates/shamir-client/src/lib.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/lib.rs#L30); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

Grouping/duplicate: [SUMMARY.md#NF.6](SUMMARY.md#review-nf-6). This is not an additional independent defect.

<a id="review-nf-2"></a>

### Claim NF.2 — Tests wired from crate root; zero inline implementation test modules

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Root cfg(test) registers the test manifest; no inline implementation test block exists. Crate-root integration files are separate auto-discovered targets.

Evidence: [crates/shamir-client/src/lib.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/lib.rs#L38); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

Grouping/duplicate: [SUMMARY.md#NF.6](SUMMARY.md#review-nf-6). This is not an additional independent defect.

<a id="review-nf-3"></a>

### Claim NF.3 — One-file-one-export or closely-coupled group

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Client/options and cache/registry groups are closely related, while stream/subscription/error modules each have a primary purpose. No unrelated public-type group was found.

Evidence: [crates/shamir-client/src/client.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L58); [crates/shamir-client/src/wire_frames.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L13); [crates/shamir-client/src/interner_cache.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L39).

Grouping/duplicate: [SUMMARY.md#NF.6](SUMMARY.md#review-nf-6). This is not an additional independent defect.

<a id="review-nf-4"></a>

### Claim NF.4 — Builder-only query construction

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Queries use typed builders, with no serde_json/json macros in the crate. Enclosing lifecycle DbRequest construction is not raw query-body assembly.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L24); [crates/shamir-client/src/cursor_stream.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L41).

Grouping/duplicate: [SUMMARY.md#NF.5](SUMMARY.md#review-nf-5). This is not an additional independent defect.

<a id="review-nf-5"></a>

### Claim NF.5 — Test coverage claims in module docs match tests present

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Explicit advertised seams are not covered: AtomicU8 never invokes Client, warm dump/same-client refresh can pass without the named operation, and ambient deltas preempt unknown-ID refresh. The src/tests interner header also incorrectly requires --full.

Evidence: [crates/shamir-client/src/tests/wire_version_tests.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/interner_cache_tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L1); [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194); [crates/shamir-client/src/tests/v2_passthrough_tests.rs:405](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/v2_passthrough_tests.rs#L405).

Grouping/duplicate: [SUMMARY.md#NF.6](SUMMARY.md#review-nf-6). This is not an additional independent defect.

## Evidence and recipe corrections

- There are eight crate-root integration files, including smoke_local, and nine library test modules.
- Tests under src/tests run with default --lib selection even when their harness boots a server; their location/registration, not their descriptive label, controls selection.
- Module presence and a green historical label do not prove advertised production seam coverage.
- Style-only imports should be rated nit; the stale copyable example is a concrete low-severity documentation defect.
- Doctest-disabled means unchecked by that runner, not uncompilable by design.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
