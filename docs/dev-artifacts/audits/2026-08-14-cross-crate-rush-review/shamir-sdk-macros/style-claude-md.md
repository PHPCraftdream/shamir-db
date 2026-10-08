<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk-macros — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Related proc-macro entrypoints do not prove a file-layout rule violation. The functional return-check gap and missing tests remain; exhaustive inline-comment wording and emitter extraction are optional.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 2 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Single lib.rs holds four public exports + two helpers -- "one file = one primary export" violated

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

CLAUDE allows closely coupled groups, and these four macros implement one shared ABI family. Duplication suggests a possible refactor, not an unmet semantic or mandatory file-count guarantee.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-sdk-macros/src/lib.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L43); [crates/shamir-sdk-macros/src/lib.rs:464](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L464).

<a id="review-2"></a>

### Claim 2 — Zero tests in the crate -- TDD protocol and tests/ layout unfulfilled; purity check untested anywhere

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Neither private helper has direct registered coverage and no local expansion/UI tests exist. Existing downstream function coverage prevents an absolute zero-workspace-coverage conclusion; historical TDD ordering remains unknown.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-sdk-macros/src/lib.rs:411](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L411); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Divergent duplicated return-type validation -- `#[function]` bypasses the shared helper

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The independent literal set has a demonstrated acceptance difference. That API behavior, not duplication alone, justifies remediation.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L330); [crates/shamir-sdk-macros/src/lib.rs:499](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L499).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Inline comment under-documents the normalisation chain

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The comment accurately names two replacements without claiming they are the only ones, and adjacent documentation mentions core::result::. Listing every replacement is optional comment style; the genuinely overbroad qualification guarantee is tracked under return acceptance.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L408); [crates/shamir-sdk-macros/src/lib.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L413); [crates/shamir-sdk-macros/src/lib.rs:418](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L418).

## Evidence and recipe corrections

- The current compliant-path assurance is inaccurate: generated Vec and Option are bare, and core/shamir_sdk paths are relative.
- A re-export-only proc-macro lib.rs is not the prescribed safe extraction shape. Keep annotated entry functions at the crate root and delegate to private helpers; the Rust Reference requires root definitions: https://doc.rust-lang.org/reference/procedural-macros.html.
- The four ABI blocks share scaffolding but are not verbatim identical.
- Presence of Safety comments is style evidence, not a full proof of safe public raw-pointer boundaries.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros -- Style & CLAUDE.md structural conformance

Reviewed against `CLAUDE.md` (workspace root), sections "Discipline rules"
(`mod.rs` re-exports only; one file = one primary export), "Test
organisation", "Imports at the top", and comment-discipline rules. Scope:
`crates/shamir-sdk-macros/` (Cargo.toml + src/lib.rs; no other files exist).

## Summary

The crate is a single 572-line `src/lib.rs` carrying four public proc-macro
exports plus two private helpers, which conflicts with CLAUDE.md's
"one file = one primary export" pillar; the shared guest-ABI `quote!` blocks
are duplicated verbatim four times and validation logic has already drifted
between macros. There is no `mod.rs` anywhere (so the re-export-only rule is
vacuously met) and imports are fully compliant with the imports-at-top rule.
The crate contains zero test code -- no `src/**/tests/`, no crate-root
`tests/` -- leaving the macro-internal helpers (`is_result_value_return`,
`type_contains_ctx`) and two of the four expansions untested anywhere in the
workspace.

## Findings

### 1. Single lib.rs holds four public exports + two helpers -- "one file = one primary export" violated
- File: `crates/shamir-sdk-macros/src/lib.rs:43-571`
- Severity: medium
- Issue: CLAUDE.md:505-509 mandates each non-`mod.rs` file own one primary
  export (or a closely-coupled group) and split unrelated public types into
  separate files. `lib.rs` exports four `#[proc_macro_attribute]` publics --
  `validator` (:43), `function` (:176), `procedure` (:307), `scalar` (:464) --
  plus private helpers `is_result_value_return` (:411) and
  `type_contains_ctx` (:425). The "closely-coupled group" reading is strained:
  the guest-ABI `shamir_alloc`/`shamir_call` `quote!` blocks are copy-pasted
  verbatim four times (lines 105-149, 235-275, 366-402, 533-567), and the
  file's size/divergence (see finding 3) shows it has outgrown the exemption.
  The CLAUDE.md-conformant shape is `lib.rs` as a re-export-only manifest
  (`mod` decls + `pub use`) over siblings `validator.rs`, `function.rs`,
  `procedure.rs`, `scalar.rs`, and one shared `abi.rs` emitting alloc+call.
- Failure scenario: none at runtime -- the cost is maintainability: 4x
  duplicated ~40-line ABI blocks must be edited in lockstep, `git blame`
  granularity is coarse, and each new macro family re-copies the ABI.
- Suggested fix: split per the documented layout in a dedicated
  `refactor:` commit (CLAUDE.md bans riding-along refactors); factor the
  alloc/call emission into the single shared `abi.rs` helper.

### 2. Zero tests in the crate -- TDD protocol and tests/ layout unfulfilled; purity check untested anywhere
- File: `crates/shamir-sdk-macros/` (entire crate; no `src/**/tests/`, no
  crate-root `tests/`, no `#[cfg(test)]` anywhere)
- Severity: medium
- Issue: CLAUDE.md's "Protocol of development (TDD)" is MANDATORY (red test
  first) and "Test organisation" prescribes the `tests/` layout; this crate
  has no test code at all. Coverage reality across the workspace: downstream
  `shamir-sdk/tests/` has compile-pass tests only for `procedure` and
  `scalar` (`procedure_compile_pass.rs`, `scalar_compile_pass.rs`);
  `validator` and `function` expansions are tested nowhere. The pure helpers
  `is_result_value_return` (lib.rs:411) and `type_contains_ctx` (lib.rs:425)
  -- including the `#[scalar]` Ctx-purity rejection -- have zero coverage
  anywhere. (The proc-macro entry fns themselves are not unit-testable
  in-process, but the two helpers are plain `fn(&Type) -> bool`, testable via
  `syn::parse_str`; syn's default `parsing` feature is enabled.)
- Failure scenario: a tweak to `type_contains_ctx`'s segment splitting or to
  `is_result_value_return`'s normalisation chain silently breaks the
  `#[scalar]` purity guarantee or return-type validation with nothing to
  catch it (CI would stay green).
- Suggested fix: add `src/tests/` per CLAUDE.md layout -- `tests/mod.rs`
  manifest + `return_type_tests.rs` / `ctx_detection_tests.rs` unit-testing
  the helpers through `syn::parse_str::<syn::Type>`, wired via
  `#[cfg(test)] mod tests;` in `lib.rs`; add `validator`/`function`
  compile-pass tests beside the existing ones in `shamir-sdk/tests/`.

### 3. Divergent duplicated return-type validation -- `#[function]` bypasses the shared helper
- File: `crates/shamir-sdk-macros/src/lib.rs:193-202` (vs `:411-420`)
- Severity: low
- Issue: `function` hand-rolls
  `type_str == "Result<Value>" || type_str == "core::result::Result<Value,Error>"`
  while `procedure` (:330) and `scalar` (:499) call the shared
  `is_result_value_return` helper, which additionally normalises
  `shamir_sdk::`/`crate::` prefixes. Same concept, two implementations in one
  file -- precisely the duplication the one-file-one-export rule exists to
  prevent. (The resulting behavioural gap -- `-> shamir_sdk::Result<Value>`
  accepted by `#[procedure]`/`#[scalar]` but rejected by `#[function]` -- is
  for the correctness theme to weigh.)
- Failure scenario: future edits to one copy (e.g. widening
  normalisation) leave the other stale, as already happened.
- Suggested fix: have `function` call `is_result_value_return(ty)`; the
  finding-1 split into a shared validation module makes this the only copy.

### 4. Inline comment under-documents the normalisation chain
- File: `crates/shamir-sdk-macros/src/lib.rs:413-418`
- Severity: nit
- Issue: the comment says "Strip any `shamir_sdk::` or `crate::` prefixes",
  but the code also strips `core::result::` (:418). The doc comment above
  (:408-410) does mention `core::result::...`, so the inline comment and code
  disagree on scope.
- Failure scenario: none -- reader of the inline comment alone may
  misjudge which qualifications are normalised.
- Suggested fix: extend the inline comment to list all three stripped
  prefixes.

### Compliant areas (no findings)
- **`mod.rs` re-export-only**: no `mod.rs` exists in the crate, so the rule
  is vacuously satisfied; the `lib.rs`-carries-all-logic aspect is covered by
  finding 1.
- **Imports at top**: all `use` statements sit in the file header
  (lib.rs:8-10); no mid-function imports; generated code correctly uses
  fully-qualified `shamir_sdk::`/`core::` paths instead of emitting `use`.
- **Comment discipline otherwise**: every `unsafe` in generated code carries
  a `// Safety:` justification (lib.rs:121, 254, 382, 548); the one TODO
  carries a slice tag (lib.rs:251); doc examples are ` ```ignore `-fenced,
  consistent with `doctest = false` in Cargo.toml (whose comment documents
  the project-wide doctest ban).

</details>
