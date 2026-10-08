<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-tunables — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Root tests and imports follow the stated conventions. Namespace splitting and documentation deduplication are optional preferences; the existing crate introduction already distinguishes runtime knobs from a future full cascade.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 | 0 | 0 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `lib.rs` embeds two definition modules inline instead of the workspace's manifest-style `lib.rs`

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The layout observation is true, but the literal mod.rs rule and closely coupled export allowance do not require this root to match sampled siblings. No functional or mandatory style defect is proved.

Evidence: [crates/shamir-tunables/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/lib.rs#L17); [crates/shamir-tunables/src/lib.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/lib.rs#L31); [CLAUDE.md:503](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L503); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

<a id="review-2"></a>

### Claim 2 — `RuntimeTunables` struct doc duplicates the module doc nearly verbatim

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Consistent semantics are repeated at module and struct entry points. No contrary rule or existing drift establishes a defect; deduplication is optional.

Evidence: [crates/shamir-tunables/src/runtime.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L1); [crates/shamir-tunables/src/runtime.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L13).

<a id="review-3"></a>

### Claim 3 — Crate-level doc is stale relative to the shipped `runtime` module

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The first line acknowledges runtime-overridable knobs; the following paragraph accurately describes build-time constants and future cascade promotion. The existing foundation is not the completed cascade, so the future framing is not stale.

Evidence: [crates/shamir-tunables/src/lib.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/lib.rs#L1); [crates/shamir-tunables/src/lib.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/lib.rs#L3); [docs/dev-artifacts/roadmap/TUNABLES.md:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/TUNABLES.md#L180).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

## Evidence and recipe corrections

- Do not classify optional consistency observations as confirmed-open runtime risks.
- Retain the root-test compliance decision: the manifest reaches five ordinary unit tests, without inline implementation test blocks.
- The historical entire-API/no-gap assertion is false: direct Default, interval boundaries, repeated updates and cross-thread sharing are not all independently exercised.
- Any optional clarification should distinguish an in-memory foundation from live server consumption and a full scope cascade.

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
