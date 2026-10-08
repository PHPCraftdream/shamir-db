<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The import nit and structural counterexamples remain. Sixteen coupled schema types are permitted, and none of these style observations establishes runtime impact.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 1 | 0 | 0 | 4 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `use shamir_query_types::hmac as canon;` repeated 4x mid-function instead of hoisted to file top

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The same unconditional local import appears four times, with no inspected cfg or trait-collision exception. Hoisting preserves behavior and addresses only style debt.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L119); [crates/shamir-server/src/db_handler/admin.rs:294](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L294); [crates/shamir-server/src/db_handler/admin.rs:375](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L375); [crates/shamir-server/src/db_handler/admin.rs:642](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L642); [CLAUDE.md:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L617).

<a id="review-2"></a>

### Claim 2 — `config.rs` bundles 16 public types in one file

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The inventory is accurate, but the types comprise the nested configuration schema and its validation error. The rule permits closely coupled groups and supplies no numerical cutoff.

Evidence: [crates/shamir-server/src/config.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L72); [crates/shamir-server/src/config.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L219); [crates/shamir-server/src/config.rs:615](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L615); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

<a id="review-summary-mod-rs"></a>

### Claim Summary/mod.rs — Every mod.rs is re-export-only with no logic

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Production manifests inspected are declarative, but tests/common/mod.rs defines helper implementations. The unqualified every-file assertion is false.

Evidence: [crates/shamir-server/tests/common/mod.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/common/mod.rs#L36); [crates/shamir-server/tests/common/mod.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/common/mod.rs#L51).

<a id="review-summary-test-layout"></a>

### Claim Summary/test layout — Topic-split tests-directory layout is followed everywhere; no inline test bodies

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Nested topic manifests are registered and no inline test-module bodies were found. However, service uses a single service/tests.rs file, contradicting the strict directory/manifest inventory.

Evidence: [crates/shamir-server/src/service.rs:622](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/service.rs#L622); [crates/shamir-server/src/service/tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/service/tests.rs#L1); [crates/shamir-server/src/db_handler/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tests/mod.rs#L1).

<a id="review-no-other-findings-imports-elsewhere"></a>

### Claim No other findings/imports elsewhere — All remaining mid-body imports satisfy documented exceptions

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The unconditional bridge-local hash-trait import has no demonstrated collision. Helper locality is not a documented general exception; decode-cache hash_one also has a helper-local trait import.

Evidence: [crates/shamir-server/src/subscriptions/bridge.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/bridge.rs#L116); [crates/shamir-server/src/subscriptions/decode_cache.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/decode_cache.rs#L66); [CLAUDE.md:622](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L622).

## Evidence and recipe corrections

- Do not promote structural preferences into runtime-severity findings.
- The closely-coupled-group allowance applies without a numerical maximum.
- The single-helper import exception in the historical report is invented; any eventual cleanup should remain separate from behavioral fixes.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- Style & CLAUDE.md structural conformance

## Summary

The crate is largely disciplined: every `mod.rs` (crate root `lib.rs`,
`connection/`, `db_handler/`, `replication/`, `server/`, `subscriptions/`, and
every `tests/mod.rs` manifest) is re-export-only with no logic, and the
`tests/` directory layout (one dir, topic-split files, `mod.rs` as manifest,
wired via `#[cfg(test)] mod tests;`) is followed correctly everywhere,
including nested cases (`logging/tests/`, `db_handler/tests/`,
`replication/tests/`, `subscriptions/tests/`). No inline
`#[cfg(test)] mod tests { ... }` bodies exist anywhere in the crate. The two
real gaps are a repeated non-cfg-gated mid-function `use` in
`db_handler/admin.rs` (imports-at-top) and `config.rs` bundling 16 public
types into a single file (one-file-one-export), both narrow enough to fix
without touching behavior.

## Findings

### 1. `use shamir_query_types::hmac as canon;` repeated 4x mid-function instead of hoisted to file top

- File: `crates/shamir-server/src/db_handler/admin.rs:119,294,375,642`
- Severity: medium
- Issue: The same import (`use shamir_query_types::hmac as canon;`) appears
  inside four separate function bodies (`create_scram_user`-style handler,
  `set_superuser`, a third admin op, and `check_destructive_hmacs`-adjacent
  helper at line 642). None of these are `cfg`-gated, and none collide with
  another `hmac`-named trait already imported at the top of the file (the
  file's top-of-file `use` block, lines 1-20, has no conflicting `hmac`
  import) — so this doesn't fit either of the two substantive documented
  exceptions ("cfg-gated bodies" or "trait collision") in CLAUDE.md's
  "Imports at the top" section. It is plain avoidable duplication that
  belongs at the file header.
- Failure scenario: Not a runtime bug — a maintainability/consistency issue.
  A future editor adding a 5th HMAC-gated admin op in this same file has
  three prior instances to copy from mid-body, reinforcing the drift instead
  of correcting it.
- Suggested fix: Move `use shamir_query_types::hmac as canon;` to the file's
  top-level `use` block (next to the existing `use shamir_query_types::auth::SecretString;`
  at line 16) and delete the four local copies.

### 2. `config.rs` bundles 16 public types in one file

- File: `crates/shamir-server/src/config.rs` (850 lines)
- Severity: low
- Issue: CLAUDE.md's "one file = one primary export" rule allows "a struct,
  enum, trait, or closely-coupled group" per file, but `config.rs` defines
  16 distinct `pub` items: `Config`, `ReplicationConfig`,
  `ObservabilityConfig`, `AuditConfig`, `SecurityConfig`, `TxLimitsConfig`,
  `CursorLimitsConfig`, `QueryLimitsConfig`, `ConnectionSecurity`,
  `LoggingConfig`, `KdfConfig`, `ListenerConfig`, `ListenerKind`,
  `ProfileKind`, `TlsConfig`, `ConfigError` (lines 72, 121, 157, 190, 219,
  276, 305, 355, 437, 486, 532, 545, 569, 579, 596, 605). These do form a
  single nested Ktav schema tree (each sub-struct is a field of `Config` or
  a field of another sub-struct), which is the strongest argument for
  "closely-coupled group" — but at 16 top-level public types the file
  stretches that allowance further than any other file in the crate (the
  next-largest multi-type file, `user_directory.rs`, has only 3).
  `git blame` on this file mixes unrelated concerns (e.g. a TLS profile
  rename touches the same file as a cursor-limits default change).
- Failure scenario: N/A (structural/maintainability, not a runtime defect).
- Suggested fix: If this file grows further, consider splitting along
  existing substructure boundaries already implied by the doc comment's
  "Schema" section (e.g. `config/listener.rs` for
  `ListenerConfig`/`ListenerKind`/`TlsConfig`/`ProfileKind`, `config/limits.rs`
  for `TxLimitsConfig`/`CursorLimitsConfig`/`QueryLimitsConfig`), keeping
  `config.rs` itself as the top-level `Config` + `ConfigError` +
  `from_file`/`validate`. Given the current size is stable and the module
  doc already documents the schema clearly, this is a nit-to-low priority
  cleanup, not urgent.

No other findings for this theme — `mod.rs` re-export discipline, imports-at-top elsewhere (the remaining mid-body `use` occurrences in `bootstrap.rs`, `framer.rs`, `main.rs`, `runtime.rs`, `service.rs`, `tls.rs`, `server_launcher.rs:115/127`, `tx_registry.rs`, `subscriptions/bridge.rs`, `subscriptions/payload.rs`, `subscriptions/decode_cache.rs`, `subscriptions/deliver_cache.rs`, and the Windows-only test in `tests/restore_tests.rs:540` are all genuinely `cfg`-gated or scoped to a single non-reusable helper block, matching CLAUDE.md's documented exception), test-directory layout, and comment discipline are all consistent with CLAUDE.md.

</details>
