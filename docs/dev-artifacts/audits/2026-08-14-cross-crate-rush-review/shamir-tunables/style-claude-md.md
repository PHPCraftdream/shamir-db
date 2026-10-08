<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tunables — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Inline namespace definitions and two documentation nits remain unchanged. The namespace split is advisory consistency work, not a hard mod.rs violation or runtime defect; root test organization is compliant.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `lib.rs` embeds two definition modules inline instead of the workspace's manifest-style `lib.rs`

Status: `confirmed-open`. Current risk: `low`.

Both constant namespaces remain inline, unlike the cited sibling roots. This confirms the optional consistency observation only: the literal restriction names mod.rs, and CLAUDE permits closely coupled export groups.

Evidence: [crates/shamir-tunables/src/lib.rs:17](../../../../../crates/shamir-tunables/src/lib.rs#L17); [crates/shamir-tunables/src/lib.rs:31](../../../../../crates/shamir-tunables/src/lib.rs#L31); [CLAUDE.md:503](../../../../../CLAUDE.md#L503); [CLAUDE.md:505](../../../../../CLAUDE.md#L505); [crates/shamir-query-types/src/lib.rs:36](../../../../../crates/shamir-query-types/src/lib.rs#L36).

<a id="review-2"></a>

### Claim 2 — `RuntimeTunables` struct doc duplicates the module doc nearly verbatim

Status: `confirmed-open`. Current risk: `nit`.

The module and struct documentation still repeat atomic-read and constant-initialization semantics. No deduplication occurred.

Evidence: [crates/shamir-tunables/src/runtime.rs:1](../../../../../crates/shamir-tunables/src/runtime.rs#L1); [crates/shamir-tunables/src/runtime.rs:13](../../../../../crates/shamir-tunables/src/runtime.rs#L13).

<a id="review-3"></a>

### Claim 3 — Crate-level doc is stale relative to the shipped `runtime` module

Status: `confirmed-open`. Current risk: `nit`.

The header paragraph and package description still emphasize build-time-only/future promotion despite the existing runtime foundation. The opening line already acknowledges runtime knobs, so the omission is partial rather than total.

Evidence: [crates/shamir-tunables/src/lib.rs:1](../../../../../crates/shamir-tunables/src/lib.rs#L1); [crates/shamir-tunables/src/lib.rs:4](../../../../../crates/shamir-tunables/src/lib.rs#L4); [crates/shamir-tunables/Cargo.toml:6](../../../../../crates/shamir-tunables/Cargo.toml#L6).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

## Corrections and qualified non-findings

- Retain the structural positives: root tests are registered, tests/mod.rs contains only the topic declaration, no inline test block exists, and imports are at file/module headers (crates/shamir-tunables/src/lib.rs:11,14,32; crates/shamir-tunables/src/tests/mod.rs:1).
- Replace 'entire public API covered/no coverage gap' with 'ordinary runtime defaults and set/get calls covered'; boundary, repeated-update, cross-thread, and consumer-effect coverage remain absent.
- Splitting the two namespaces is optional style work. The current closely related constant group does not establish a hard violation of CLAUDE.md:505, and no runtime severity follows from the layout.
- Fix stale documentation by accurately describing the unwired foundation, not by claiming the existing runtime fields already form a live cascade.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tunables -- Style & CLAUDE.md structural conformance

## Summary

The crate is small and largely exemplary against CLAUDE.md's structural rules: `src/tests/mod.rs` is a manifest-only re-export wired via `#[cfg(test)] mod tests;`, tests are topic-split with no inline `#[cfg(test)] mod tests { ... }` blocks, every `use` sits at a file/module header, and the test suite covers the crate's entire public API. The one structural deviation is `lib.rs` itself: unlike the sampled sibling crates (manifest-style `mod`/`pub use` only), it carries two full definition modules inline. Two minor comment-discipline nits round out the list (a duplicated doc block, and a crate-level doc that predates the `runtime` module it sits above).

## Findings

### 1. `lib.rs` embeds two definition modules inline instead of the workspace's manifest-style `lib.rs`
- **File:line:** `crates/shamir-tunables/src/lib.rs:17-160`
- **Severity:** low
- **Issue:** CLAUDE.md's discipline rules state "`mod.rs` files contain re-exports only. Types and logic live in sibling files" and "One file = one primary export ... This keeps diffs atomic and `git blame` meaningful." `lib.rs` plays the crate-root `mod.rs` role, yet instead of declaring `pub mod store_defaults;` / `pub mod instance_defaults;` it defines both modules inline (~140 lines, 17 consts). Sampled sibling crates follow the sibling-file pattern: `shamir-numa/src/lib.rs` and `shamir-query-types/src/lib.rs` are pure `mod` + `pub use` manifests with all definitions in sibling files. The rule's letter names `mod.rs` (not `lib.rs`) and the two namespaces are a closely-coupled group, so this is not a hard violation -- but this is the only crate root sampled that carries definitions, and it is the documented growth surface ("a later phase promotes selected knobs to a runtime cascade"), so it will keep accreting.
- **Failure scenario:** As tunables are added, `lib.rs` diffs mix unrelated knob families, eroding the atomic-diff/blame rationale behind the rule, and the crate becomes the off-pattern template copied by future crates.
- **Suggested fix:** Split verbatim into `src/instance_defaults.rs` and `src/store_defaults.rs`, leaving `lib.rs` as `pub mod runtime;` + the two module declarations + `#[cfg(test)] mod tests;`, matching `shamir-numa`/`shamir-query-types`. Land it as a standalone `style:`/`chore:` commit per the code-quality rules (style-only sweeps live in their own commits).

### 2. `RuntimeTunables` struct doc duplicates the module doc nearly verbatim
- **File:line:** `crates/shamir-tunables/src/runtime.rs:13-16` (vs. module doc `runtime.rs:1-7`)
- **Severity:** nit
- **Issue:** The struct doc repeats the `//!` module doc's three sentences ("Reads are a single atomic load (instant, cached, lock-free ...); overrides store a new value. Initialized from the compiled `instance_defaults` consts ...") almost word-for-word. Redundant copies drift independently.
- **Failure scenario:** A future change to override semantics (ordering, visibility, invalidation) updated in one copy but not the other leaves contradictory docs that rustdoc renders on the same page.
- **Suggested fix:** Keep the semantics in one place (module doc) and reduce the struct doc to a single line, e.g. "Instance-level runtime-overridable tunables; see module docs."

### 3. Crate-level doc is stale relative to the shipped `runtime` module
- **File:line:** `crates/shamir-tunables/src/lib.rs:1-7`
- **Severity:** nit
- **Issue:** The `//!` crate doc says "Today these are plain `const`s ... a later phase promotes selected knobs to a runtime cascade" and never mentions `pub mod runtime;` declared directly below it (line 9). `runtime::RuntimeTunables` already is that promotion for three instance-level knobs, so the crate doc understates the crate's contents. The `Cargo.toml` `description` ("build-time knobs") carries the same framing.
- **Failure scenario:** A consumer reading only the crate docs concludes runtime overrides don't exist yet and hard-codes a redundant const-copy workaround; the "(future)" framing misleads contributors about the module's status.
- **Suggested fix:** Add one sentence to the crate doc: `runtime` provides runtime-overridable instance-level knobs seeded from `instance_defaults`; the remaining consts are build-time only. Optionally refresh the Cargo.toml description.

## Conformance notes (checked, no finding)

- `src/tests/mod.rs` is a manifest-only re-export (`pub mod runtime_tests;`) -- matches the `tests/mod.rs` rule and mirrors the established `shamir-numa/src/tests/` pattern.
- Tests are split by topic (`runtime_tests.rs`), wired via `#[cfg(test)] mod tests;` in the parent, and no implementation file contains an inline `#[cfg(test)] mod tests { ... }`.
- All `use` statements live at file/module headers, including `use super::Duration;` at the top of the `instance_defaults` module body (explicitly allowed as "the enclosing module's header"). No mid-function imports anywhere.
- Test coverage matches the crate surface: `defaults_equal_consts` pins all three runtime defaults to their `instance_defaults` consts (the invariant claimed in `runtime.rs`'s docs), each setter has a round-trip test, and `reads_are_shared_ref` covers `&self`/`Arc` shareability. No coverage gap found.

</details>
