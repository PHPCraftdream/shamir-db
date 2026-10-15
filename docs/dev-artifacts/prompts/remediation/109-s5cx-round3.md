# Round 3 — S5c-X repository write-domain repairs

Implement the finite Round3 stage1 source corrections in the current worktree. Read `CLAUDE.md` and `108-s5cx-repo-write-domain.md` before editing (done); this document preserves the delegated Round3 brief without committing it.

## Restrictions

- No git writes or commits; orchestrator commits.
- No `git reset`, `checkout`, `clean`, `stash`, `restore`, `rm`, or any git command that mutates worktree/index.
- No local builds/tests/clippy/rustc. `cargo fmt -p shamir-engine` is permitted only as the final mutating operation.
- Keep changes finite, source-verified, and avoid unrelated edits/comments. Do not close public raw store/index accessors in this stage.
- Layout ratchet against origin/master: >1000 line files or >7 entry folders cannot grow; p997 <=1155 lines, server db_handler <=1174. Prefer existing subfolders.

## Round3 final verification evidence (2026-10-15)

- Recovery admission test is in existing `crates/shamir-engine/src/db_instance/tests/write_domain_membership_tests.rs` (test starts at line 18 before formatting), not the >1000-line `tx/tests/recovery_tests.rs`. The `tx/tests` source had 1,726 lines at HEAD and worktree, so no growth there. Destination had 115 lines at HEAD and 189 after adding 74 lines; it remains below 1,000. `db_instance/tests` has four file entries (was already wired in `mod.rs`); `tx/tests` has 59 file entries. Existing `db_instance/tests/write_domain_membership_tests.rs` was extended, no new direct entry.
- The test uses explicit `RecordKey::from_slice` for `MetaKey::LastCommittedVersion`; it captures exact persisted marker bytes and inflight WAL frames before direct poll, checks blocked replay leaves record unavailable and marker/WAL unchanged, then releases admission and checks checkpoint `Some(7)`, retained WAL equality, replayed value and idempotent repeat. Imports `Future`, `Context`, and `Poll` were removed from recovery_tests.rs after confirming no remaining references there.
- Source signatures checked: `RepoInstance::add_table(&self, config: TableConfig, permit: &RepoWritePermit)` and `remove_table(&self, table_name: &str, permit: &RepoWritePermit)` in `repo/changelog_store/repo_membership_impl.rs`; admitted wrappers call them with permit. DB create/drop and seed installation use admitted wrappers; rename passes its permit. No other raw table membership method callers were found in engine source; similarly named `RepoConfig::add_table` is a builder, not this membership mutation.
- Read-only admission call inventory extraction: `rg -n '\\.admit(_pair)?\\s*\\(' crates/shamir-engine/src` returned direct permit acquisition sites in DB create/drop, `tx/recovery.rs`, repo-membership initialization, repo-instance mutations/recovery, seed install, durability policy, record counter, table-manager modules/barrier/doctor, plus focused permit tests in db_instance/meta/repo/table. This is an exact callsite listing in command output; enclosing-function attribution remains to be completed with source inspection. `_admitted` membership methods are defined in `repo/changelog_store/write_domain_membership.rs` and called by `db_instance/db_instance.rs` and `repo/seed_install/writer.rs`.
- Explicit remaining obligations: P1/schema, retention, and migration proof incomplete; full caller-level proof and enclosing-function inventory not complete; public raw store/index accessor audit incomplete and deferred. No full proof is claimed.

## Work / verification limits

1. Source corrections and caller migration are represented in the actual changed tree; inspect current definitions and every reference before claiming completion.
2. No tests, builds, clippy, rustc, commits, or git writes were run. Formatting is the sole allowed Cargo operation and must be the final mutation.
3. Complete read-only call-site/caller inventory and retain explicit unresolved P1/schema/retention/migration and public-accessor audit gaps above.

## Completion report

Report changed absolute paths, actual signatures checked, remaining issues, formatter exit code, and verification limits. Do not claim compile/test success.
