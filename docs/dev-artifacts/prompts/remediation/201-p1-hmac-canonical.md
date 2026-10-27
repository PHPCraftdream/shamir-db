# P1 stage H6 — p1-hmac-canonical

## Goal

HMAC canonical inputs injective, destructive-op coverage. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H6): HMAC canonical inputs are injective and cover every destructive parameter; every destructive op family is gated; HMAC-gated builders require a tag; the newest canonical helpers are tested.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-QUERY-TYPES-9CDE03EF — HMAC canonical inputs are not injective (NUL/comma/slash aliasing) and don't cover `cascade`/`if_exists`/`replace` modifiers

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/engine/db_handler/handlers/admin.rs:656; crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:160; changed by: none
- R0 note (includes the fix site): OBSERVED: DropDb confirmation canonicalizes only the database name, while execution still branches on cascade before removing children; canonical helpers also retain unescaped NUL/slash/CSV forms. INFERRED: Changing cascade preserves the confirmation bytes but changes deletion scope; #270 only improves persistence/error handling. Fix site: security/hmac.rs and its server callers, bind modifiers and use an injective versioned encoding.

### RV-QUERY-TYPES-A3D93402 — Destructive-op HMAC coverage has drifted: whole op families are ungated

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/engine/db_handler/handler.rs:631; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:757; changed by: none
- R0 note (includes the fix site): OBSERVED: The handler still promises confirmation for every drop, but the enumerated gate falls through for function, validator and replication drops whose DTOs lack confirmation fields. This remains policy/documentation drift, not an authorization bypass; #274 changes unrelated error mapping. Fix site: engine/db_handler/handler.rs, align the promise with an explicit coverage policy before any protocol expansion.

### RV-QUERY-TYPES-C7AEC31F — Canonical-input encoding ambiguities: interior NULs and empty identifiers

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/security/hmac.rs:165; crates/shamir-query-types/src/security/hmac.rs:357; changed by: none
- R0 note (includes the fix site): OBSERVED: Role/user components are joined without escaping, grants use comma joining, and empty Function renders identically to FunctionNamespace. INFERRED: The stated NUL/CSV/empty-identifier library witnesses still alias regardless of narrower server validation. Fix site: security/hmac.rs, introduce injective component encoding with a compatibility/version policy.

### RV-QUERY-TYPES-F1DC8E9A — Four newest `hmac::canonical_*` helpers have zero tests; `create_scram_user` doc shows a trailing `\0` the implementation does not emit

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/security/hmac.rs:68; crates/shamir-query-types/src/security/tests/hmac_tests.rs:2; changed by: none
- R0 note (includes the fix site): OBSERVED: Local searches find no test calls to the four named helpers; the SCRAM layout table still advertises a trailing NUL, whereas join_null inserts separators only between parts. Fix sites: security/hmac.rs and security/tests/hmac_tests.rs, reconcile the documented bytes and pin each helper independently.

### RV-QUERY-BUILDER-E6F53E38 — "HMAC-gated" builders all build successfully without a tag -- gating is advisory-only client-side

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/ddl/policy/auth.rs:49; crates/shamir-query-builder/src/ddl/schema/function.rs:95; changed by: none
- R0 note (includes the fix site): OBSERVED: CreateUser, Chmod and conditional CreateFunction builders still construct operations with None HMAC without local rejection; the server independently rejects required missing/invalid tags. This remains client-validation debt, not an authentication bypass. Fix sites: crates/shamir-query-builder/src/ddl/policy/auth.rs and crates/shamir-query-builder/src/ddl/schema/function.rs, making required-tag validation explicit and fallible.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU5 — injective HMAC canonical encoding for admin confirmations (RV-QUERY-TYPES-9CDE03EF, -C7AEC31F, -A3D93402, -F1DC8E9A)

Decided by 2x rush reviewer + @oh (unanimous on substance, 3/3): replace every v1 `join_null` canonical form with ONE length-prefixed, op-tagged v2 encoding covering every field of the op except `hmac`, `request_id` and the password; switch Rust, TS and the e2e JS helper in ONE atomic change with NO v1 acceptance window (the per-session key makes tags ephemeral, nothing persisted depends on v1, all signers are in-tree, the project is alpha; unlike RU4 there is no on-disk v1 data to keep verifying); extend the gate through an exhaustive policy to the still-ungated destructive families. The three rulers proposed slightly different byte grammars; the grammar below (@oh's) is FINAL.

Facts: `crates/shamir-query-types/src/security/hmac.rs` joins raw components with NUL (`join_null`), CSV for grants, `db://a/b/c` for resources. Aliases: `grant_role("a\0b","c")` = `("a","b\0c")`; `create_scram_user("a",["b"])` = `("a\0b",[])`; grants `["a,b"]` = `["a","b"]`; `Function{""}` = `FunctionNamespace` (`fn://`); `Store["a","b/c"]` = `Table["a","b","c"]`. The drop gate (`crates/shamir-server/src/engine/db_handler/handlers/admin.rs` ~656-663) hashes names only while `cascade` decides child deletion; `create_user` tag covers the username only; `handler.rs` ~631 promises "every drop_* op" but the gate falls through at ~757: `DropFunctionOp`, `DropValidatorOp` and the replication drops have no `hmac` field; `replace` on an invoker function skips the gate.

Acceptance:
1. Exact v2 layout. `canonical = b"shamir-db hmac canonical v2" (27 bytes) || 0x00 || op_tag:u16 BE || fields`. Atoms (no type bytes): `str` = u32 BE byte length || UTF-8 bytes (no normalisation); `u64` = 8 bytes BE; `u16` = 2 bytes BE; `bool` = 0x00/0x01; `opt(T)` = 0x00 | 0x01 || T; `list(T)` = u32 BE count || items in wire order (never sorted); `enum` = u8 variant index || payload; `digest` = 32-byte SHA-256 of the exact wire string bytes; `res` (ResourceRef): 0 Database str | 1 Store str,str | 2 Table str,str,str | 3 Function str | 4 FunctionFolder list(str) | 5 FunctionNamespace (no payload); `grp` (GroupRef): 0 Name str | 1 Id u64; `scope` (PurgeScope): 0 OlderThan u64 | 1 OlderThanAge u64.

   Op tags (a `#[repr(u16)]` enum, never reused) and schemas: 0x0001 drop_db: str db, bool cascade, bool if_exists | 0x0002 drop_repo: str db_in_use, str repo, bool cascade, bool if_exists | 0x0003 drop_table: str db_in_use, str repo, str table, bool cascade, bool if_exists | 0x0004 drop_index: str db_in_use, str repo, str table, str index, bool if_exists | 0x0005 drop_user: str user, bool if_exists | 0x0006 start_migration: str db_in_use, str repo, str table, str dst_repo, str dst_engine, opt(str) dst_path | 0x0007 commit_migration / 0x0008 rollback_migration: str db_in_use, str id | 0x0009 grant_role / 0x000A revoke_role: str role, str user | 0x000B chmod: res, u16 mode | 0x000C chown: res, u64 owner | 0x000D chgrp: res, opt(u64) group | 0x000E create_user: str name, list(str) roles, opt(str) database | 0x000F set_retention: str db_in_use, str repo, str table, opt(u64) max_age_secs, opt(u64) max_count, opt(u64) min_count | 0x0010 purge_history: str db_in_use, str repo, str table, scope | 0x0011 create_group: str name | 0x0012 drop_group: grp, bool if_exists | 0x0013 rename_group: grp, str to | 0x0014 add_group_member / 0x0015 remove_group_member: grp, u64 user | 0x0016 create_function: str name, bool replace, str security (absent = "invoker"), opt(str) visibility, list(str) secret_grants, list(str) net_grants, opt(digest) source, opt(digest) wasm | 0x0017 set_superuser / 0x0018 set_replicator: str user, bool on | 0x0019 create_scram_user: str name, list(str) roles | 0x001A drop_function / 0x001B drop_validator: str name, bool if_exists | 0x001C create_validator: str name, bool replace, opt(digest) source, opt(digest) wasm | 0x001D drop_replication_profile / 0x001E drop_publication / 0x001F drop_subscription: str name. The key derivation (`"shamir-db hmac key v1\0"`) is unchanged.
2. One function used by both sides. Each encoder takes the whole op struct and destructures it WITHOUT `..`, so adding a field fails to compile until it is encoded or explicitly excluded with a comment. The server gate and the Rust signing helpers both call one `shamir-query-types` function (for `BatchOp` and the three `DbRequest` variants); the handlers at `admin.rs` ~122, ~297, ~378 move to it too.
3. Exhaustive policy: `hmac_policy(&BatchOp)` has no wildcard arm and returns Required / NotGated. Required: every currently gated op; drop_function, drop_validator, drop_replication_profile, drop_publication, drop_subscription (add optional `hmac` fields); create_function when `definer || !secret_grants.is_empty() || replace`; create_validator when `replace`. NotGated (documented in the module doc; fix the wording at `handler.rs` ~631): every create (DB/repo/table/index/folder/profile/publication/subscription), every rename, alter_repo, buffer config, schema ops, bind/unbind_validator, alter_subscription, DML, reads/lists. Rust and TS builders get `hmac` setters for the newly gated ops.
4. Known-answer vectors: commit `crates/shamir-query-types/test-vectors/admin_hmac_v2.json` (session_id = bytes 0x00..0x1f), at least one vector per op tag; `canonical_hex` and `tag_hex` come from a checked-in stdlib-only `python -I` reference script (`struct` + `hmac`), never from the code under test; at least one vector also appears as a literal field-annotated byte string in the Rust test. Vectors include a u64 above 2^53, non-ASCII strings, empty strings, empty and non-empty lists, both Option states. Rust tests, TS `hmac.test.ts` and `tests/e2e/helpers/hmac.js` all assert the same file. (The executor cannot run Python; it writes the reference script and derives expected values by hand-checked construction, saying so; CI exercises all three implementations against the file.)
5. Alias witnesses now distinct: tests assert different canonical bytes for every v1 alias pair above, plus `FunctionFolder["a/b"]` vs `["a","b"]` and GroupRef `Name "id:3"` vs `Id 3`.
6. Flag flips change the tag: a table-driven test flips every bool/Option/list field of every Required schema and asserts the canonical bytes change. Server integration test: a tag signed for `drop_db cascade=false` sent with `cascade=true` gets `hmac_mismatch` and the DB and its repos still exist; the same for `create_user` roles.
7. Each newly gated op returns `hmac_required` without a tag, `hmac_mismatch` with a wrong tag, and succeeds with a v2 tag.
8. v1/v2 boundary: a tag computed in the test with the `hmac` crate directly over the v1 bytes (for example `b"drop_db\0mydb"`) is rejected with `hmac_mismatch`; the v1 `canonical_*` functions and `join_null` are deleted (declare in the rust-guard delta file if a campaign is active). Error codes stay `hmac_required` / `hmac_mismatch`. Fix the doc table that shows a trailing NUL the code never emitted; the four newest helpers (RV-QUERY-TYPES-F1DC8E9A) are covered by the vectors.

Compat: breaking only for HMAC tags; no disk impact; op wire shapes unchanged (`hmac` stays an optional hex string; new `hmac` fields on the newly gated ops). A v1-signing client gets `hmac_mismatch` on every gated op; a client calling a newly gated family without a tag gets `hmac_required` (fails closed). Update protocol docs and guides that quote v1 bytes in the same change. Risks the executor must guard: JS u64 precision (use BigInt) and UTF-8 byte length (not `string.length`); ForEach templated values stay signed unresolved (document).


## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
