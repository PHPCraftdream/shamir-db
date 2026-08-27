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
