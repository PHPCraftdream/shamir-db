<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-client — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All three style findings remain; they are documentation/style issues, not runtime High defects. Module registration and structural organization remain conforming.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 3 | 0 | 0 | 0 | 1 | 4 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled

Status: `confirmed-open`. Current risk: `low`.

The example omits both required timeout fields; disabled doctests prevent automatic checking.

Evidence: [crates/shamir-client/src/lib.rs:10](../../../../../crates/shamir-client/src/lib.rs#L10); [crates/shamir-client/src/client.rs:85](../../../../../crates/shamir-client/src/client.rs#L85); [crates/shamir-client/Cargo.toml:52](../../../../../crates/shamir-client/Cargo.toml#L52).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `use rand::RngCore;` inside the body of `Client::resume`

Status: `confirmed-open`. Current risk: `low`.

The import remains inside an ordinary block without a documented exception. This is source-style conformance only.

Evidence: [crates/shamir-client/src/client.rs:883](../../../../../crates/shamir-client/src/client.rs#L883); [CLAUDE.md](../../../../../CLAUDE.md).

Grouping/duplicate: `SUMMARY.md#7.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Mid-body `use` statements in three `src/tests/` files

Status: `confirmed-open`. Current risk: `nit`.

All three cited body-local imports remain; none needs collision/cfg handling.

Evidence: [crates/shamir-client/src/tests/batch_has_refs_tests.rs:18](../../../../../crates/shamir-client/src/tests/batch_has_refs_tests.rs#L18); [crates/shamir-client/src/tests/demux_tests.rs:408](../../../../../crates/shamir-client/src/tests/demux_tests.rs#L408); [crates/shamir-client/src/tests/wire_version_tests.rs:137](../../../../../crates/shamir-client/src/tests/wire_version_tests.rs#L137).

Grouping/duplicate: `SUMMARY.md#7.3`. This row is not another independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — `lib.rs` and `src/tests/mod.rs` are re-export/manifest-only

Status: `not-applicable`. Current risk: —.

The crate root contains module declarations/reexports, and tests/mod.rs only declares the nine test modules.

Evidence: [crates/shamir-client/src/lib.rs:30](../../../../../crates/shamir-client/src/lib.rs#L30); [crates/shamir-client/src/tests/mod.rs:1](../../../../../crates/shamir-client/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#NF.6`. This row is not another independent defect.

<a id="review-nf-2"></a>

### Claim NF.2 — Tests wired from crate root; zero inline implementation test modules

Status: `not-applicable`. Current risk: —.

Root cfg(test) loads tests/mod.rs; no inline implementation test blocks were found.

Evidence: [crates/shamir-client/src/lib.rs:38](../../../../../crates/shamir-client/src/lib.rs#L38); [crates/shamir-client/src/tests/mod.rs:1](../../../../../crates/shamir-client/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#NF.6`. This row is not another independent defect.

<a id="review-nf-3"></a>

### Claim NF.3 — One-file-one-export or closely-coupled group

Status: `not-applicable`. Current risk: —.

Client/options/demux, wire mirrors, cache types, and individual stream/subscription modules remain closely coupled groups.

Evidence: [crates/shamir-client/src/client.rs:58](../../../../../crates/shamir-client/src/client.rs#L58); [crates/shamir-client/src/wire_frames.rs:13](../../../../../crates/shamir-client/src/wire_frames.rs#L13); [crates/shamir-client/src/interner_cache.rs:39](../../../../../crates/shamir-client/src/interner_cache.rs#L39).

Grouping/duplicate: `SUMMARY.md#NF.6`. This row is not another independent defect.

<a id="review-nf-4"></a>

### Claim NF.4 — Builder-only query construction

Status: `not-applicable`. Current risk: —.

No serde_json/json construction exists; typed builders create query bodies and transport wrappers construct only their enclosing requests.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:24](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L24); [crates/shamir-client/src/cursor_stream.rs:41](../../../../../crates/shamir-client/src/cursor_stream.rs#L41).

Grouping/duplicate: `SUMMARY.md#NF.5`. This row is not another independent defect.

<a id="review-nf-5"></a>

### Claim NF.5 — Test coverage claims in module docs match tests present

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

Named modules and many behavioral tests exist, but broad coverage assurance is not justified: AtomicU8 is vacuous and refresh coverage is masked by ambient sync.

Evidence: [crates/shamir-client/src/tests/mod.rs:1](../../../../../crates/shamir-client/src/tests/mod.rs#L1); [crates/shamir-client/src/tests/wire_version_tests.rs:135](../../../../../crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/v2_passthrough_tests.rs:405](../../../../../crates/shamir-client/src/tests/v2_passthrough_tests.rs#L405).

Grouping/duplicate: `SUMMARY.md#NF.6`. This row is not another independent defect.

## Corrections and qualified non-findings

- Readability/style violations do not establish runtime severity.
- The crate now has an additional smoke_local integration test; the historical seven-file integration-test count is stale.
- The cursor close/cancel test registration and assertions are genuine; this does not validate every unrelated module-doc coverage claim.

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
