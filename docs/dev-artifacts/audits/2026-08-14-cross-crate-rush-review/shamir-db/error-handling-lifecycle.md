<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-db — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Warn-only catalogue persistence, remove-first artifact renames and ignored cascade errors remain. The report incorrectly claims no SystemStore injection seam: existing ACL tests already replace system tables through the engine test-util seam, although lifecycle write-failure assertions remain missing.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 12 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Catalogue-persistence failures are swallowed (`warn!` + continue) across the DB/repo/table lifecycle, so multi-step mutations can return `Ok(())` half-migrated

Status: `confirmed-open`. Current risk: `high`.

Create/drop and database/repository/table rename persistence still logs failures and continues. Rename save failure does not stop removal of the old key. Several prerequisite reads do propagate, so not every failure is swallowed.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:184](../../../../../crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L184); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:195](../../../../../crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L195); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:534](../../../../../crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L534); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:323](../../../../../crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L323); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:337](../../../../../crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L337).

<a id="review-2"></a>

### Claim 2 — `rename_function_as` / `rename_validator_as` / `rename_function_folder_as` destroy the durable record *before* writing the new one (remove-before-write)

Status: `confirmed-open`. Current risk: `high`.

Function/validator old-row removal still precedes save. Folder rename deletes every old row before its save loop and still claims no partial state. Reordering alone preserves a copy but does not make multi-row rename atomic.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:337](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L337); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:375](../../../../../crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L375); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:471](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L471); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:567](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L567).

<a id="review-3"></a>

### Claim 3 — No error-path test injects a system-store failure into the lifecycle paths above

Status: `confirmed-open`. Current risk: `medium`.

Relevant tests still lack catalogue-write/rename failure assertions. But a system-table injection mechanism already exists: access_meta_tests installs custom databases/groups TableManagers into system_repo. Extend that seam rather than claiming injection is impossible.

Evidence: [crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs:741](../../../../../crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs#L741); [crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs:759](../../../../../crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs#L759); [crates/shamir-db/Cargo.toml:93](../../../../../crates/shamir-db/Cargo.toml#L93); [crates/shamir-db/src/shamir_db/tests/mod.rs:1](../../../../../crates/shamir-db/src/shamir_db/tests/mod.rs#L1).

<a id="review-4"></a>

### Claim 4 — `SystemStore::add_group_member` / `remove_group_member` silently fabricate a phantom group when the id does not exist

Status: `confirmed-open`. Current risk: `medium`.

Both store methods still default absent records and call save_group. Public facade methods reach these under a per-group lock, which prevents lost-update interleavings but does not enforce existence.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:713](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L713); [crates/shamir-db/src/shamir_db/system_store.rs:743](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L743); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:660](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L660).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Cascade-drop paths discard per-table drop errors with `let _ =` — not even a log line

Status: `confirmed-open`. Current risk: `medium`.

Database/repository cascades still discard table-cleanup Results; index cascades discard their results too. Stale rows/orphaned data are possible. Table rows alone cannot resurrect a deleted database/repository because boot attaches tables only under existing parent rows.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:130](../../../../../crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L130); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:377](../../../../../crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L377); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:252](../../../../../crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L252); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:219](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L219).

<a id="review-6"></a>

### Claim 6 — Wire error-code classification by substring on the stringified `PortError`

Status: `confirmed-open`. Current risk: `low`.

Grant/revoke still classify the English substring; create/drop map errors to query. Current directory errors contain the expected text, so current classification works but has no typed/stable port contract.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:148](../../../../../crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L148); [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:187](../../../../../crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L187); [crates/shamir-db/src/shamir_db/ports.rs:32](../../../../../crates/shamir-db/src/shamir_db/ports.rs#L32); [crates/shamir-server/src/user_directory.rs:710](../../../../../crates/shamir-server/src/user_directory.rs#L710).

<a id="review-7"></a>

### Claim 7 — Boot path: repo re-attach failure is `warn!` + `continue`, and repo rows for unknown databases are skipped without any log

Status: `confirmed-open`. Current risk: `low`.

Attach errors still permit degraded boot with a warning; missing-parent rows have no diagnostic. Successful attachment followed by failed recovery aborts boot. No structured failed-attach diagnostic reconciles these policies.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:219](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L219); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:243](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L243); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:263](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L263).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Ambient interner delta attach: errors silently skipped, contradicting the module's own doc

Status: `confirmed-open`. Current risk: `low`.

Both interner error branches still continue without reporting; the helper never returns those errors despite its soft-BatchError promise. Epoch synchronization can lag, but permanent mis-resolution until a full dump is not proven: a later delta request can recover.

Evidence: [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:20](../../../../../crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L20); [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:40](../../../../../crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L40); [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:44](../../../../../crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L44); [crates/shamir-db/src/shamir_db/execute/db_execute.rs:69](../../../../../crates/shamir-db/src/shamir_db/execute/db_execute.rs#L69).

<a id="review-9"></a>

### Claim 9 — `admin_result_with_op_id` panics on wall-clock regression while every other call site defaults

Status: `confirmed-open`. Current risk: `low`.

duration_since(UNIX_EPOCH) is still unwrapped. A pre-epoch time is an environmental failure; ordinary backward clock movement that remains after the epoch does not trigger this panic.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:59](../../../../../crates/shamir-db/src/shamir_db/execute/helpers.rs#L59).

Grouping/duplicate: `correctness-tdd.md#10`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Stringly-typed error collapsing loses error identity on internal mappings

Status: `confirmed-open`. Current risk: `low`.

Implicit transaction BatchErrors and status-resolution DbErrors are still string-wrapped as Internal; gateway errors use Debug Strings. Typed identity is lost, though the gateway Debug text can retain the code text.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:156](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L156); [crates/shamir-db/src/shamir_db/system_store.rs:178](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L178); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:772](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L772); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:88](../../../../../crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L88).

<a id="review-11"></a>

### Claim 11 — `resolve_in_group` silently converts group-lookup errors into `false` without a log

Status: `confirmed-open`. Current risk: `low`.

The silent unwrap_or(false) remains. It removes group-class grants but is not universally fail-closed: permits selects Other when membership is false, and Other permissions can be broader than Group permissions.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:916](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L916); [crates/shamir-types/src/access.rs:682](../../../../../crates/shamir-types/src/access.rs#L682); [crates/shamir-types/src/access.rs:709](../../../../../crates/shamir-types/src/access.rs#L709).

<a id="review-12"></a>

### Claim 12 — Minor nits (grouped)

Status: `confirmed-open`. Current risk: `nit`.

The guarded itype unwraps and silent response-header file-read fallback remain. Present non_btree guards establish Some, so those unwraps are not a current panic defect; the read error still becomes empty headers.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:459](../../../../../crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L459); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:514](../../../../../crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L514); [crates/shamir-db/src/shamir_db/curl_gateway.rs:228](../../../../../crates/shamir-db/src/shamir_db/curl_gateway.rs#L228).

## Corrections and qualified non-findings

- Existing system_repo/install_table_for_test use positively refutes 'SystemStore has no equivalent seam' and 'unverified by construction'. The missing lifecycle write-failure assertions remain real.
- Catalogue save failures after the WAL commit point may be committed-but-not-materialized, not aborted. Tests/fixes must discriminate failure timing and outcome instead of assuming every injected storage fault makes set_via_implicit_tx return Err.
- Write-new-before-remove-old is preservation ordering, not complete multi-row rename atomicity. Old rows are not inert merely because their live registration moved; boot reconstructs registrations from persisted rows.
- Cascade table rows alone cannot recreate absent database/repository parents; qualify resurrection as requiring stale surviving parent rows.
- Substring classification currently matches the directory implementation; the concern is fragility, not demonstrated present misclassification.
- resolve_in_group false is not an unconditional deny. Retain the missing-log finding and correct the stronger fail-closed guarantee.
- Library source contains the schema expect and guarded unwraps; the blanket 'no expect'/'sole unwrap' descriptions are inaccurate. No facade anyhow/unsafe use was found.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-db -- Error handling & resource lifecycle

## Summary

The crate is broadly faithful to CLAUDE.md's error-handling rules: `Result<T, E>` with the shared `thiserror`-style `DbError` throughout (no `anyhow` anywhere in `src/`, no raw `panic!`/`expect` on production paths outside guarded invariants), genuinely exemplary fail-closed ACL error handling (`resource_meta`/`authorize_access`), compensating-write rollback in `admin_schema.rs` and `compile_table_schema`, and a documented `Box<dyn Error>` boundary only where the dependency direction forbids naming the impl's error type. The systemic weakness is the opposite corner: the DB/repo/table/function/validator *catalogue lifecycle* layer treats storage-write failures as `log::warn!`-and-continue, so multi-step catalogue mutations (renames especially) can return `Ok(())` half-migrated, and two rename paths use remove-before-write — the exact inverse of the crate's own write-new-before-remove-old crash-safety convention. Error-path tests are strong for validation/rollback, but no test injects a system-store write failure into these swallowed paths.

## Findings

### 1. Catalogue-persistence failures are swallowed (`warn!` + continue) across the DB/repo/table lifecycle, so multi-step mutations can return `Ok(())` half-migrated
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:184-201` (`rename_db_as` — save-new then remove-old, both `if let Err(e) => log::warn!`), also `db_management.rs:58-64` (`create_db_as`), `77-85` (`remove_db`), `232-251` + `298-319` (rename_db repo/table re-key loops), `396-426` (`add_repo_as`), `435-446` (`remove_repo`), `534-553` + `571-598` (`rename_repo_as`); `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:56-74` (`add_table_as`), `95-107` (`drop_table`), `323-350` (`rename_table_as`).
- **Severity:** high
- **Issue:** Every one of these writes the in-memory/live registration first, then persists catalogue changes with a `write-new-before-remove-old` design whose comments promise crash-safety ("a crash between the two writes leaves the new row resolvable"). But an *error* (not a crash) in either catalogue write is only logged: the function never propagates, and remove-old proceeds even when write-new failed. `flush_all` (`db_management.rs:632-656`) demonstrates the crate's own better pattern — log each failure, then return the first error.
- **Failure scenario:** `rename_db_as("a","b")`: in-memory key moves to `b`; `save_database("b", …)` fails on a transient fjall I/O error (warn); `remove_database("a")` succeeds → catalogue now holds **no** record for the database while the caller got `Ok(())`. On restart, `core.rs:194-201` registers no `DbInstance` and `core.rs:219` silently skips every repo row whose db is missing (`if let Some(db)` — not even a warn) → all repos/tables of the renamed DB are unavailable after reboot with zero operator-visible signal beyond earlier warns. Mirror case for `rename_table_as`: save-new fails + remove-old succeeds → the table is gone from the catalogue but the physical stores were already renamed → orphaned data, table unresolvable after restart.
- **Suggested fix:** Propagate the first catalogue error (after attempting the compensating step), mirroring `flush_all`; minimally, do not run remove-old when the preceding write-new failed, and return `Err` if any step of the re-key sequence failed. Add an `else { log::warn!(…) }` (or skip-list accessor, cf. `unresolved_native_artifacts`) for repo rows whose db is missing at boot in `core.rs:219`.

### 2. `rename_function_as` / `rename_validator_as` / `rename_function_folder_as` destroy the durable record *before* writing the new one (remove-before-write)
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:335-347` (`remove_function(from).await?` at 337 before `save_function(to, …)` at 344); `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:373-385` (same shape, remove at 375, save at 382); `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:565-574` (folder rename removes **all** old keys in one loop, then saves all new keys in a second loop).
- **Severity:** high
- **Issue:** These are the only rename paths in the crate that remove the old durable row before the new one is durably written — the inverse of the convention `rename_db_as`/`rename_repo_as`/`rename_table_as` document and follow. `rename_function_folder_as`'s doc comment explicitly claims "no partial state is left if a write fails mid-way" — the implementation does not satisfy that claim: every old key is already gone when the first save fails.
- **Failure scenario:** `rename_function_as("f","g")`: live registry renamed; `remove_function("f")` succeeds; `save_function("g", …)` fails (fsync/I/O error) → `Err` returned, but the function's durable record no longer exists under *either* name → after restart the function is silently gone (boot only loads from the catalogue). Same for validators, and for an entire renamed folder subtree.
- **Suggested fix:** Reorder to persist-new-first, then remove-old (both `?`-propagated), matching the db/repo/table renames; fix the folder-rename doc or implement it (save-new loop first, remove-old after). Optionally roll back the live-registry rename when the catalogue step fails, so registry and catalogue never diverge on the error path.

### 3. No error-path test injects a system-store failure into the lifecycle paths above
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/tests/schema_rollback_tests.rs` (covers `compile_table_schema` rollback), `crates/shamir-db/src/shamir_db/tests/p1065_ddl_status_contract_tests.rs:264` (fault-injected `DbError::Storage("injected set failure")` — but only for the index DDL op-status contract), plus ~100 `is_err()`/`unwrap_err` assertions across `src/shamir_db/tests/` and `tests/` covering validation/not-found/denied paths.
- **Severity:** medium
- **Issue:** Validation-error coverage is excellent, but none of the swallowed catalogue-write paths (findings 1, 2, 4) has a test, because nothing can currently fail `SystemStore::save_*`/`remove_*` in a test. The engine side already has the seam (`shamir-engine` `test-util` fault-injecting Store, used by `p1065`); the facade's `SystemStore` has no equivalent, so the `Ok(())`-with-half-migrated-catalogue behaviour in `rename_db_as`/`rename_function_as` etc. is unverified by construction.
- **Failure scenario:** A refactor reorders remove-old before write-new in `rename_db_as` (as `rename_function_as` already did) and every test stays green.
- **Suggested fix:** Add a test-only fault-injecting seam for `SystemStore` writes (cfg(test) wrapper or `test-util` feature), then test: (a) rename_db/repo/table with write-new failing → must return `Err` and leave old rows intact; (b) rename_function/validator with save failing → old record must survive; (c) boot with a repo row whose db is missing → warn/diagnostic surfaced.

### 4. `SystemStore::add_group_member` / `remove_group_member` silently fabricate a phantom group when the id does not exist
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/system_store.rs:712-734` and `742-762` (`load_group` → `Ok(None)` tolerated; write-back with `name = ""`, `owner = Actor::System` default). Contrast `set_group_owner` at `768-772`, which correctly returns `DbError::NotFound`.
- **Severity:** medium
- **Issue:** The wire dispatcher defends against this (`admin_access.rs:381-395` even names the hazard: "would let `system_store::add_group_member` silently fabricate a phantom group record"), but the store-level API — and the public facade `ShamirDb::add_group_member`/`add_group_member_as`/`remove_group_member`/`remove_group_member_as` (`access_control.rs:634-694`) which calls it directly — still fabricates. The CLAUDE.md-endorsed "self-defending method" pattern used for `create_group_as`/`authorize_group_manage_or_root` argues the check belongs in the store/`*_as` layer, not only at one dispatcher.
- **Failure scenario:** Any non-wire caller (CLI tooling, a future internal path, an embedder using the public facade) calls `add_group_member(999_999, 42)` → a durable group record with an empty name and `owner = System` appears; it then participates in `resource_meta(Group)` resolution and `rename_group_as` uniqueness scans.
- **Suggested fix:** Return `DbError::NotFound` from `SystemStore::add_group_member`/`remove_group_member` when `load_group` yields `None` (same as `set_group_owner`), and delete the now-redundant dispatcher-side `group_id_exists` pre-check or keep it for its nicer wire code.

### 5. Cascade-drop paths discard per-table drop errors with `let _ =` — not even a log line
- **File:line:** `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:130-137` (`handle_drop_db` cascade over tables), `377-384` (`handle_drop_repo` cascade); `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:252, 261-264, 274, 279-282` (index-cascade `let _ =`).
- **Severity:** medium
- **Issue:** `drop_table_cleaning_validators` returns `DbResult<bool>`; its catalogue-removal half is best-effort with a `warn!`, but the cascade call sites throw the whole `Result` away. A failing table drop during `DROP DATABASE … CASCADE` is invisible, the outer op reports success, and stale `(db, repo, table)` catalogue rows survive — which `boot` will happily re-create as tables under a dropped db/repo on next start.
- **Failure scenario:** System-store blip during cascade: `DROP DATABASE` answers `{"dropped": true}`; on restart, the previously dropped database partially resurrects from surviving table-catalogue rows (or, at minimum, orphaned rows linger with no diagnostic).
- **Suggested fix:** At minimum `log::warn!` on each failed cascade step; better, collect failures and include a `"partial": [...]` field in the admin response, mirroring the `p1_2` DDL result-contract style.

### 6. Wire error-code classification by substring on the stringified `PortError`
- **File:line:** `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:146-154` and `185-193` (`msg.contains("user not found")` decides `not_found` vs `query`).
- **Severity:** medium
- **Issue:** `PortError = Box<dyn Error>` (`ports.rs:32`) is a documented, justified boundary choice, but classifying the error by English substring is fragile: a wording change in the implementing directory (`shamir-server`) silently downgrades every `not_found` to the generic `query` code, breaking client retry/UX logic with no compile-time or test signal. Note also the inconsistency: `handle_grant_role`/`handle_revoke_role` classify, while `handle_create_user`/`handle_drop_user` (lines 56-63, 103-106) map everything to `query` unconditionally.
- **Failure scenario:** Directory error message changes from `"user not found: …"` to `"no such user …"` → all GrantRole/RevokeRole on unknown users start returning code `query`; clients that surface "user not found" specially regress silently.
- **Suggested fix:** Define a shared sentinel convention at the port boundary — e.g. document a stable prefix (`"not_found: …"`) checked via `starts_with`, or add a marker method to the `UserAdminPort` trait (`fn is_not_found(&self, e: &PortError) -> bool`) so the implementing layer owns the classification. Apply it uniformly in all four handlers.

### 7. Boot path: repo re-attach failure is `warn!` + `continue`, and repo rows for unknown databases are skipped without any log
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/core.rs:243-252` (`add_repo` failure → `log::warn!` + `continue`), `core.rs:219` (`if let Some(db) = shamir.get_db(db_name)` — no else branch for missing db), vs `core.rs:263` where `recover_v2_inflight` failure *is* propagated ("a repo that cannot recover must not serve").
- **Severity:** low
- **Issue:** The durability asymmetry is undocumented: a repo whose attach fails (e.g. fjall directory lock held by a second process, corrupted dir) is dropped from the serving set while its catalogue rows remain, whereas a repo that attaches but fails recovery aborts the whole boot. Nothing but a log line distinguishes the degraded boot; the doc comment on the recovery step states the stronger policy ("must not serve") without noting the attach-failure exception.
- **Failure scenario:** Operator restarts the server while a stray process holds the repo's lock → server boots green, `List`/catalogue still show the repo, first query on it fails with a confusing runtime `NotFound`, and committed data appears "gone" until the stray process exits and the server is restarted again.
- **Suggested fix:** Record failed-attach repos in a startup diagnostic (like `unresolved_native_artifacts`) at minimum; for disk-backed repos consider propagating (fail boot) to match the recovery policy, or document why attach is softer than recovery.

### 8. Ambient interner delta attach: errors silently skipped, contradicting the module's own doc
- **File:line:** `crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:20-21` (doc: "Errors loading an interner are surfaced as a soft `BatchError`") vs `38-45` (`Err(_) => continue`, no log); caller `crates/shamir-db/src/shamir_db/execute/db_execute.rs:97-102` logs at `debug!` only.
- **Severity:** low
- **Issue:** Doc/implementation drift plus a silent swallow: an interner-open failure produces no delta for that repo and no warn. The client keeps its stale epoch and can mis-resolve newly interned field names until a full `InternerDump`.
- **Failure scenario:** Transient store error during `repo_interner().await` → response carries no delta → client caches lag → interned field names sent by the client after the gap miss on the server.
- **Suggested fix:** `log::warn!` in the `Err` arms (or actually return the soft `BatchError` the doc promises); align doc and code in whichever direction.

### 9. `admin_result_with_op_id` panics on wall-clock regression while every other call site defaults
- **File:line:** `crates/shamir-db/src/shamir_db/execute/helpers.rs:59-62` (`.unwrap()` on `duration_since(UNIX_EPOCH)`); compare the same expression handled with `.unwrap_or_default()` at `db_management.rs:41-44` and `admin_migration.rs:83-86`.
- **Severity:** low
- **Issue:** CLAUDE.md allows `panic!` only for programmer-invariant violations; a clock set before the epoch (or a platform clock error) is environmental, not invariant. This is the sole production `unwrap()` in the crate and it is inconsistent with the established treatment of the identical expression.
- **Suggested fix:** `.unwrap_or_default()` like the sibling sites.

### 10. Stringly-typed error collapsing loses error identity on internal mappings
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/system_store.rs:156` and `178` (`BatchError` → `DbError::Internal(e.to_string())`); `crates/shamir-db/src/shamir_db/shamir_db/core.rs:764-777` (`get_ddl_op_status` wraps an existing `DbError` into `Internal(format!(…))`, so callers can no longer distinguish `NotFound`/`Validation`); `crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:87-89` (`format!("{e:?}")` — Debug, not Display — for `BatchError` into the engine gateway's `String` error).
- **Severity:** low
- **Issue:** All `?`-discipline is respected, but these mappings erase variant identity (callers/ops can no longer match on `NotFound` vs storage faults) and one site renders errors with unstable Debug formatting. Partly forced by engine-side `String`-error traits (`DbGateway`, `NetGateway`), which is outside this crate's control.
- **Suggested fix:** Prefer a lossless variant where it exists (e.g. forward the original `DbError` in `get_ddl_op_status` instead of re-wrapping; add a `DbError::Batch` variant for the implicit-tx mappings); use `Display` in `batch_err_to_string`.

### 11. `resolve_in_group` silently converts group-lookup errors into `false` without a log
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:913-918` (`self.user_in_group(user_id, gid).await.unwrap_or(false)`); doc at `910-912` documents the behaviour.
- **Severity:** low
- **Issue:** Fail-closed direction is correct and deliberate (deny rather than fail-open), but the conversion is invisible: during a transient catalogue outage, every group-based grant (member-class permissions) silently evaporates for all checks made in that window, with no log to explain the denials. Contrast the sibling fail-closed sites in `resource_meta`/`authorize_access`, which all `log::warn!` before denying.
- **Suggested fix:** Add a `log::warn!` (or `debug!` with correlation id) when `user_in_group` errors, matching the documented pattern of the other fail-closed arms.

### 12. Minor nits (grouped)
- **File:line:** `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:459, 466, 514` — `itype.unwrap()` inside error-message construction; provably safe (guarded by `non_btree = matches!(itype, Some("vector")|…)`), but `unwrap_or("?")` would stay panic-proof under future edits to the guard.
- **File:line:** `crates/shamir-db/src/shamir_db/curl_gateway.rs:226-244` — `parse_response_headers` swallows the header-file read error (`if let Ok(bytes)`): an I/O failure is indistinguishable from "no headers". A `log::warn!` would match the module's own "cleanup on every path" diligence.
- **Severity:** nit
- **Issue / Suggested fix:** as noted per item; no functional defect today.

</details>
