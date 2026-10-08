<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-db — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

All 72 consolidated claims and 20 plans were independently checked. Core correctness findings remain, dedup is fixed, and optional contracts should be removed from defect counts. Curl injection gains conditional primary-source proof; membership-error severity increases. Catalogue recovery and successful schema-preservation gaps also require attention.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 72 | 60 | 2 | 1 | 0 | 0 | 9 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — `DROP DATABASE … CASCADE` executed against a different database destroys the *batch's* database's tables

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Enumeration uses the named database while table deletion uses the batch database; matching names expose destructive cross-target behavior.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L133).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — `remove_group_member` on a nonexistent group fabricates a phantom record; the wire remove path lacks the add path's guard

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Numeric missing IDs pass resolution; removal defaults absent fields and saves a row under privileged management authority.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:743](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L743); [crates/shamir-db/src/shamir_db/execute/admin_access.rs:454](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_access.rs#L454).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — `save_database`/`remove_database` and all replication-catalogue writes skip the "Durable DDL" flush; a test comment asserts they don't

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The omissions remain; existing data-store flushes do not drain history or synchronize separate Buffered WAL, and flush_all excludes SystemStore.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:204](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L204); [crates/shamir-engine/src/tx/commit.rs:973](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L973); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:634](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L634).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — Boot silently skips repository rows whose database row is missing — no warning, data invisible forever

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Boot has no missing-parent diagnostic or attachment; repair can restore visibility, so forever is excessive.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L219).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — Boot "skipping" a builtin-name-colliding function row still overwrites its `function_meta`; the live artifact diverges from the catalogue after restart

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Failed builtin-name registration still inserts catalogue metadata; effective actor independently reads that same row.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L324); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:995](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L995).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — `rename_function_as` / `rename_validator_as` re-key remove-first

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Live rename precedes old-row deletion and new-row save; failure compensation is absent.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L337); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:375](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L375).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — `create_db_as` / `create_db` silently overwrite an existing live `DbInstance`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

create_db_as inserts a fresh instance without duplicate defense; new lookups lose the prior registration.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L39).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — `drop_function_as` returns `existed = false` while deleting a durable catalogue-only function

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registry-only existed and IF EXISTS ignore durable-only rows; normal listing is registry-only too.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L284); [crates/shamir-db/src/shamir_db/execute/admin_function.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_function.rs#L138).

Grouping/duplicate: [correctness-tdd.md#8](correctness-tdd.md#review-8). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — `create_validator_inner` replace path resets `bound_in` and can mint `RecordId::default()`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Catalogue identity fallback is absent and removal clears live reverse bindings after persisting an empty list.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L249); [crates/shamir-engine/src/validator/registry.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/registry.rs#L156).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-1-10"></a>

### Claim 1.10 — Panic paths in library code: `.expect("interner touch_ind")` and `.unwrap()` on `SystemTime`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Pre-epoch clock panic remains; touch_ind has no current explicit returned-error branch, refuting that half's scenario.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L61); [crates/shamir-types/src/core/interner/interner.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L138).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-1-11"></a>

### Claim 1.11 — `foreign_key_dto_from_qv` silently coerces unknown `on_delete`/`on_update` strings to `NoAction` — then re-persists them

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Invalid present action values normalize before strict reparse; valid ordinary wire rules do not supply this bad catalogue state.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:1343](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L1343); [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:1352](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L1352).

Grouping/duplicate: [correctness-tdd.md#11](correctness-tdd.md#review-11). This is not an additional independent defect.

<a id="review-1-12"></a>

### Claim 1.12 — Bundle: silent isolation fallback / no-op version bump / shared lock namespace

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Isolation silently downgrades and no-op removal persists a version bump; current username-lock false sharing remains refuted.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_tx.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L80); [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:876](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L876); [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L137).

Grouping/duplicate: [correctness-tdd.md#12](correctness-tdd.md#review-12). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — `execute_as` re-runs the full async ACL traversal per batch op — the inline dedup cache its sibling `tx_execute_as` established was never ported

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Both entry points use Authorized's seen set; repeated-target gate-count regression remains absent.

Evidence: [crates/shamir-engine/src/query/batch/authorized.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L104); [crates/shamir-db/src/shamir_db/execute/db_execute.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_execute.rs#L40).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — Table-level DDL (CREATE/DROP/RENAME TABLE) has unserialized check-then-act races; the race is acknowledged by a `debug_assert!`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No common namespace guard covers probes, config snapshot, store copies and catalogue changes; the index-only barrier is insufficient.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L217); [crates/shamir-engine/src/repo/repo_instance.rs:563](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L563).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-2-3"></a>

### Claim 2.3 — DROP/RENAME of databases and repos bypass the #546 create locks — asymmetric serialization axes

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Drop/rename bypass creation locks, permitting destination overwrite or late stale catalogue writes.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L52); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:159](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L159).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-2-4"></a>

### Claim 2.4 — Per-request ACL metadata is re-read from storage on every authorization — no lock-free snapshot (pillar-5 `ArcSwap` fit)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Fresh catalogue reads are permitted architecture; ArcSwap caching is optional and requires a coherent invalidation design.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:851](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L851).

Grouping/duplicate: [concurrency-lockfree.md#4](concurrency-lockfree.md#review-4). This is not an additional independent defect.

<a id="review-2-5"></a>

### Claim 2.5 — Group existence validated *before* acquiring the per-group RMW lock — a completed concurrent drop can be resurrected

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

After pre-lock authorization, a dropped group is reloaded as defaults and saved under the acquired lock.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L617); [crates/shamir-db/src/shamir_db/system_store.rs:713](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L713).

Grouping/duplicate: [concurrency-lockfree.md#5](concurrency-lockfree.md#review-5). This is not an additional independent defect.

<a id="review-2-6"></a>

### Claim 2.6 — `SystemStore::load_repository_record` full-catalogue scan + linear filter for a keyed lookup

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The helper loads every repository then searches; filtered substitution is still a scan, not constant lookup.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L339); [crates/shamir-db/src/shamir_db/system_store.rs:837](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L837).

Grouping/duplicate: [concurrency-lockfree.md#6](concurrency-lockfree.md#review-6). This is not an additional independent defect.

<a id="review-2-7"></a>

### Claim 2.7 — `handle_start_migration`'s duplicate-start guard is check-then-act over `active_migrations`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

An enabled pair of starts can both pass the pre-await probe and insert distinct coordinators.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_migration.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_migration.rs#L91); [crates/shamir-db/src/shamir_db/execute/admin_migration.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_migration.rs#L195).

Grouping/duplicate: [concurrency-lockfree.md#7](concurrency-lockfree.md#review-7). This is not an additional independent defect.

<a id="review-2-8"></a>

### Claim 2.8 — `resolve_group_id(GroupRef::Name)` is a full `load_groups()` scan per call

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Name resolution loads/searches all groups; its own documentation already explicitly states scanning.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:752](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L752); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:777](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L777).

Grouping/duplicate: [concurrency-lockfree.md#8](concurrency-lockfree.md#review-8). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — Guest-controlled header values/method can inject arbitrary curl config directives (CRLF injection)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `unverified`.

Raw LF reaches configuration; inspected published parsers accept separate injected directives. Egress grants and compatible curl are prerequisites; installed-version execution remains unverified.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L83); [crates/shamir-db/src/shamir_db/curl_gateway.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L210).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Ambient interner delta exposes any repo's field-name dictionary without Store-level authorization

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Database Read plus a permitted empty batch can return denied-repository dictionary entries without Store Read.

Evidence: [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L30); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L50).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — Validator Rust-source path bypasses the WasmCompiler Execute gate (task #607)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Source creation checks no WasmCompiler gate; ordinary-user direct wire reachability uses ForEach, not top-level admin dispatch.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L221); [crates/shamir-engine/src/query/batch/query_runner.rs:758](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L758).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-3-4"></a>

### Claim 3.4 — Egress response body read without any size cap

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Granted egress uses uncapped read_to_end; time limiting does not bound response bytes.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L161).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-3-5"></a>

### Claim 3.5 — Dead TLS/password-hash dependencies with a stale "kept compiling" rationale

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Manifest declarations have no facade production consumers; runtime linkage/attack surface is not proved.

Evidence: [crates/shamir-db/Cargo.toml:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L59); [crates/shamir-db/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L64).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-3-6"></a>

### Claim 3.6 — `ShamirDb::execute` (System-actor, ACL-bypassing) is public and undiscoverable-hidden

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Embedded privilege is deliberate and live server callers use execute_as; hiding is optional API guidance.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_execute.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_execute.rs#L19); [docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md#L111).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-3-7"></a>

### Claim 3.7 — `set_net_allowlist` mutates only one clone; other `ShamirDb` clones keep the old allowlist

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Setter replaces only one handle's Arc; configure-before-use documentation limits the asserted revocation scenario.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:604](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L604).

Grouping/duplicate: [security-crypto.md#7](security-crypto.md#review-7). This is not an additional independent defect.

<a id="review-3-8"></a>

### Claim 3.8 — `wasm_hash` uses non-cryptographic FxHash and is never verified

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

FxHash remains write-only metadata, not a currently trusted admission control.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L187); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

Grouping/duplicate: [api-wire-protocol.md#10](api-wire-protocol.md#review-10). This is not an additional independent defect.

<a id="review-3-9"></a>

### Claim 3.9 — `SECURITY DEFINER` grants the guest the owner-actor raw DB gateway, including admin ops

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Owner-actor delegation is intentional; arbitrary-admin exposure requires guest logic that exposes those operations.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L285); [docs/guide-docs/guide/05-functions.md:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/05-functions.md#L284).

Grouping/duplicate: [security-crypto.md#9](security-crypto.md#review-9). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request

Status: `partially-fixed`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

Dedup removes repeated required pairs; unindexed per-distinct-check scans and common-ancestor rereads remain.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L97); [crates/shamir-engine/src/query/batch/authorized.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L104).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — `execute_as` re-authorizes every op in a batch without dedupe

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Authorized skips repeated action/path pairs in both execution modes.

Evidence: [crates/shamir-engine/src/query/batch/authorized.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L102); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L126).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — Function invocation scans the function catalogue twice plus two settings scans per call

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

User authorization loads the function before effective_fn_actor independently reloads it; folder depth adds reads.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:720](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L720); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:995](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L995).

Grouping/duplicate: [performance-hotpath.md#3](performance-hotpath.md#review-3). This is not an additional independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — False "O(1) point lookup" comments encode a scan-based cost model (and hide an O(N²) introspection path)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Listing loops perform unindexed filtered reads per live entry; structure supports a product cost, not measured latency.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L377); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:418](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L418).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-4-5"></a>

### Claim 4.5 — `InternerTouch` computes the epoch via a full interner traversal per touch

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Touch materializes all entries for max; request-only maxima and allocation counters fail global publication/gap semantics.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L170); [crates/shamir-types/src/core/interner/interner.rs:522](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L522).

Grouping/duplicate: [performance-hotpath.md#5](performance-hotpath.md#review-5). This is not an additional independent defect.

<a id="review-4-6"></a>

### Claim 4.6 — Boot path pairs repos with their tables via an O(repos × tables) nested scan

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each repository filters the complete table list; startup product work remains.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:230](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L230).

Grouping/duplicate: [performance-hotpath.md#6](performance-hotpath.md#review-6). This is not an additional independent defect.

<a id="review-4-7"></a>

### Claim 4.7 — DDL FK guards re-scan the table catalogue once per sibling table — O(tables²) per rename/drop

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each sibling invokes a filtered global tables-catalogue scan.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L265); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:173](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L173).

Grouping/duplicate: [performance-hotpath.md#7](performance-hotpath.md#review-7). This is not an additional independent defect.

<a id="review-4-10"></a>

### Claim 4.10 — FK guards silently skip decode-corrupt parent rows — the RESTRICT/refuse guarantee silently evaporates

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both guards continue on Err/None and non-list schema, and malformed FK shapes simply fail to match. These are child/sibling rows, not parent rows. A failed child lookup permits destructive parent DDL despite a live reference. Corruption diagnostics can also be discarded inside SystemStore. Oracle: inject that sibling failure and prove parent registration/data remain unchanged.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:263](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L263); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:273](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L273); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L177); [crates/shamir-db/src/shamir_db/system_store.rs:888](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L888).

<a id="review-4-8"></a>

### Claim 4.8 — Per-invocation gateway construction allocates and intersects allowlists via `Vec::contains`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Fresh Vec/filter/Arc construction remains; meaningful runtime impact is unmeasured.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:847](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L847); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:851](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L851).

Grouping/duplicate: [performance-hotpath.md#8](performance-hotpath.md#review-8). This is not an additional independent defect.

<a id="review-4-9"></a>

### Claim 4.9 — Intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounded by unique-name volume

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Admin/schema locks retain entries by design; other registries grow too, and usernames no longer populate this map in production.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L55); [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L137).

Grouping/duplicate: [performance-hotpath.md#9](performance-hotpath.md#review-9). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — `replace=true` on a WASM validator destroys persisted binding bookkeeping and can silently re-key its identity

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Empty persisted bindings plus remove/re-register erase both reverse binding stores while table-side references survive.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L284); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:305](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L305).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — Dead, exported `api::{Command, Request, Response}` wire shim that no server speaks

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Legacy DTO exports remain, with README warning and no active transport consumer.

Evidence: [crates/shamir-db/src/lib.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/lib.rs#L26); [crates/shamir-db/src/api/README.md:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/api/README.md#L17).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Convenience `execute` / `tx_begin` / `tx_execute` / `tx_commit` default to `Actor::System` (admin bypass)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Documented embedded privileged behavior; actual server transaction methods preserve authenticated actors.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_tx.rs:159](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L159); [crates/shamir-server/src/db_handler/tx_handlers.rs:247](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L247).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Builder-only query-construction rule bypassed across the facade (~31 hand-assembled wire-op sites, no exception comments; builder is dev-dep only)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Manual DTO construction lacks builder-exception rationale; no malformed operation is demonstrated.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L130); [crates/shamir-db/Cargo.toml:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L102).

Grouping/duplicate: [api-wire-protocol.md#4](api-wire-protocol.md#review-4). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — Wire error-`code` contract populated unevenly across handler families; `TransactionInfo::aborted` reason mixes stable codes with free text

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Current DTO documentation expressly permits these shapes; a closed vocabulary would be a new contract.

Evidence: [crates/shamir-query-types/src/batch/batch_error.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_error.rs#L36); [crates/shamir-query-types/src/batch/transaction_info.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/transaction_info.rs#L14).

Grouping/duplicate: [api-wire-protocol.md#5](api-wire-protocol.md#review-5). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — `tx_begin` accepts any isolation string and silently falls back to Snapshot

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Unknown strings succeed with Snapshot while the server reports the original requested string.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_tx.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L80); [crates/shamir-server/src/db_handler/tx_handlers.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L64).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — `to_qv` converts serialization failure into `QueryValue::Null` inside Ok responses

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Generic error collapse is proven, but a currently failing event DTO and projection corruption were not shown.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L73).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — Dead TLS/network dependencies and stale `net` doc

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unused TLS declarations and absent net documentation persist.

Evidence: [crates/shamir-db/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L64); [crates/shamir-db/src/lib.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/lib.rs#L8).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-5-9"></a>

### Claim 5.9 — Malformed client input mapped to `DbError::Internal` in `get_ddl_op_status`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Parse and table-resolution variants are both collapsed into Internal Strings.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:764](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L764); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:772](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L772).

Grouping/duplicate: [error-handling-lifecycle.md#10](error-handling-lifecycle.md#review-10). This is not an additional independent defect.

<a id="review-5-10"></a>

### Claim 5.10 — Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Only function version is written as 1; hashes remain metadata with no verified integrity consumer.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L205); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L267).

Grouping/duplicate: [api-wire-protocol.md#10](api-wire-protocol.md#review-10). This is not an additional independent defect.

<a id="review-5-11"></a>

### Claim 5.11 — `create_db_as` reports success even when the catalogue write fails

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Warn-only save failure still returns DbInstance and created=true.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L58); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L70).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-5-12"></a>

### Claim 5.12 — Bundle: wire-handler hygiene

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Clock unwrap, Debug gateway error, codec demo and ports typo remain; Debug retains code text.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L61); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L88); [crates/shamir-db/src/main.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/main.rs#L7); [crates/shamir-db/src/shamir_db/ports.rs:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/ports.rs#L6).

Grouping/duplicate: [api-wire-protocol.md#12](api-wire-protocol.md#review-12). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — Catalogue-persistence failures are swallowed (`warn!` + continue) across the DB/repo/table lifecycle, so multi-step mutations can return `Ok(())` half-migrated

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Failed save-new does not stop remove-old or successful lifecycle results; prerequisite failures can also follow live mutation.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:184](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L184); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L337).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — `rename_function_as` / `rename_validator_as` / `rename_function_folder_as` destroy the durable record *before* writing the new one (remove-before-write)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

All three old-row-first orderings remain; folder no-partial-state prose is contradicted by its two mutation loops.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:567](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L567); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:375](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L375).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — No error-path test injects a system-store failure into the lifecycle paths above

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Read-fault injection exists; lifecycle write/history/WAL failure assertions and correct MVCC-routing injection remain absent.

Evidence: [crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs:741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs#L741); [crates/shamir-engine/src/repo/repo_instance.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L377).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — `SystemStore` group-member methods silently fabricate a phantom group

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both member methods save fabricated defaults when the record is absent.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:713](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L713); [crates/shamir-db/src/shamir_db/system_store.rs:743](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L743).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Cascade-drop paths discard per-table drop errors with `let _ =` — not even a log line

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Outer cleanup outcomes are discarded; some inner failures warn, and table rows alone cannot restore missing parents.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L130); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L377).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — Wire error-code classification by substring on the stringified `PortError`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The text classifier matches current directory wording but is not a guaranteed port-level taxonomy.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L148); [crates/shamir-server/src/user_directory.rs:590](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L590).

Grouping/duplicate: [error-handling-lifecycle.md#6](error-handling-lifecycle.md#review-6). This is not an additional independent defect.

<a id="review-6-7"></a>

### Claim 6.7 — Boot path: repo re-attach failure is `warn!` + `continue`; repo rows for unknown databases are skipped without any log

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Attach failures warn/continue, missing parents are silent, and user-repository replay failures abort boot.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:243](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L243); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:263](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L263).

Grouping/duplicate: [error-handling-lifecycle.md#7](error-handling-lifecycle.md#review-7). This is not an additional independent defect.

<a id="review-6-8"></a>

### Claim 6.8 — Ambient interner delta attach: errors silently skipped, contradicting the module's own doc

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Loading failures continue without the documented soft error; later retry can recover dictionary synchronization.

Evidence: [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L20); [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L40).

Grouping/duplicate: [error-handling-lifecycle.md#8](error-handling-lifecycle.md#review-8). This is not an additional independent defect.

<a id="review-6-9"></a>

### Claim 6.9 — `admin_result_with_op_id` panics on wall-clock regression while every other call site defaults

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The epoch-duration unwrap remains; post-epoch clock regression is insufficient to trigger it.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L61).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-6-10"></a>

### Claim 6.10 — Stringly-typed error collapsing loses error identity on internal mappings

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Internal wrappers lose variants and gateway Strings lose structure, although Debug can retain textual codes.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L156); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:772](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L772); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L88).

Grouping/duplicate: [error-handling-lifecycle.md#10](error-handling-lifecycle.md#review-10). This is not an additional independent defect.

<a id="review-6-11"></a>

### Claim 6.11 — `resolve_in_group` silently converts group-lookup errors into `false` without a log

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A non-owner group member denied Read under mode 0704 gains Other Read when a group lookup error becomes false; logging alone is inadequate.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:916](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L916); [crates/shamir-types/src/access.rs:682](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L682); [crates/shamir-types/src/access.rs:709](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L709).

Grouping/duplicate: [error-handling-lifecycle.md#11](error-handling-lifecycle.md#review-11). This is not an additional independent defect.

<a id="review-6-12"></a>

### Claim 6.12 — Bundle: panic-proofing / swallowed header-file read

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Guards establish Some at unwrap sites; header-file read errors still become empty headers.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:448](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L448); [crates/shamir-db/src/shamir_db/curl_gateway.rs:228](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L228).

Grouping/duplicate: [error-handling-lifecycle.md#12](error-handling-lifecycle.md#review-12). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Function-local `use` statements in production code violate the imports-at-top rule

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Ten cited production imports remain without a documented imports-at-top exception.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:759](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L759); [crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs#L109); [crates/shamir-db/src/shamir_db/execute/admin_replication.rs:498](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_replication.rs#L498).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — `shamir_db::shamir_db` module inception suppressed with an unannotated allow

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Unexplained allow remains; top-level reexports exist and deep public paths need compatibility handling.

Evidence: [crates/shamir-db/src/shamir_db/mod.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/mod.rs#L7); [crates/shamir-db/src/lib.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/lib.rs#L32).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — `SYSTEM_DB_NAME` constant declared inside `mod.rs` (re-exports only)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

SYSTEM_DB_NAME remains defined rather than reexported in mod.rs.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/mod.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/mod.rs#L15).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Blanket `#![allow(deprecated)]` with no named reason

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Attributes remain; external README already explains the legacy API's compatibility reason.

Evidence: [crates/shamir-db/src/main.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/main.rs#L1); [crates/shamir-db/src/api/types.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/api/types.rs#L1); [crates/shamir-db/src/api/README.md:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/api/README.md#L11).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — Stray empty statement and dead comment in `curl_gateway.rs`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Bare semicolon and repeated comment remain behaviorally inert.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L132).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

<a id="review-7-6"></a>

### Claim 7.6 — `tests/mod.rs` manifests use private `mod` instead of the documented `pub mod` form

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Private declarations are registered by parent cfg(test); visibility is not a discovery requirement.

Evidence: [crates/shamir-db/src/shamir_db/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/mod.rs#L1); [crates/shamir-db/src/shamir_db/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/mod.rs#L1).

Grouping/duplicate: [style-claude-md.md#6](style-claude-md.md#review-6). This is not an additional independent defect.

<a id="review-7-7"></a>

### Claim 7.7 — Inconsistent qualified vs imported spelling of `new_map`/`QueryValue` throughout the facade

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Equivalent spellings violate no specified rule; normalization is optional preference.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L9); [crates/shamir-db/src/shamir_db/system_store.rs:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L148).

Grouping/duplicate: [style-claude-md.md#7](style-claude-md.md#review-7). This is not an additional independent defect.

<a id="review-7-8"></a>

### Claim 7.8 — `schema_management` alone breaks the sibling export convention

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Crate-visible shared helpers and internal deep imports require no mandatory matching sibling visibility.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/mod.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/mod.rs#L8).

Grouping/duplicate: [style-claude-md.md#8](style-claude-md.md#review-8). This is not an additional independent defect.

<a id="review-7-9"></a>

### Claim 7.9 — `ports.rs` carries four public exports in one file (borderline cohesion)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The documented shared identity/dependency-direction seam fits the closely coupled export exception.

Evidence: [crates/shamir-db/src/shamir_db/ports.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/ports.rs#L1).

Grouping/duplicate: [style-claude-md.md#9](style-claude-md.md#review-9). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 20 | 18 | 0 | 2 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Wrong database targeting and discarded cleanup outcomes remain. Preserve supported cross-database DDL by using the actual target; rejecting all cross-database operations is a contract change. Cover repository/index cleanup and observe unchanged A data/bindings in the regression.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L133); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L377); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L252).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `unverified`.

Compatible published parser source now proves directive injection. Reject raw configuration controls before writing guest values; separately define valid HTTP header/method syntax. Test the production config-building seam, not merely a copied escape helper.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L83); [crates/shamir-db/src/shamir_db/curl_gateway.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L210); [crates/shamir-db/src/shamir_db/tests/curl_gateway_tests.rs:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/curl_gateway_tests.rs#L6).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Warn-only writes and remove-first renames remain. Write-before-remove preserves a copy but does not establish atomic multi-row rename, rollback or ownership consistency. Return honest pre/post-WAL-commit outcomes and reconcile live registrations rather than claiming every Err aborted.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:184](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L184); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L337); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:567](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L567); [crates/shamir-engine/src/repo/repo_instance.rs:1153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L1153).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Binding-preserving replacement is not used. Handle catalogue-only IDs, require successful live replacement or register/restore bookkeeping, preserve table-side bindings and existing metadata, and account for concurrent bind/drop. The native implementation is not a safe template.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L95); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L249); [crates/shamir-engine/src/validator/registry.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/registry.rs#L112).

<a id="plan-p0-5"></a>

### Plan P0.5 — P0.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Absent-row mutation remains. Reject under the same group lock after reloading, including rename; a dispatcher-only precheck leaves completed-drop resurrection. Keep removal of a nonmember from an existing group idempotent.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:743](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L743); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L617); [crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs:570](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs#L570).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `partially-fixed`.

Prior-cycle decision: `partially-fixed`.

Authorized dedup is complete. Scans and false O(1) comments remain. A keyed shortcut must preserve exact key derivation, interning and MVCC visibility; a metadata cache requires ownership/group invalidation. Group-name lookup remains a separate index problem.

Evidence: [crates/shamir-engine/src/query/batch/authorized.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L102); [crates/shamir-db/src/shamir_db/system_store.rs:813](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L813); [crates/shamir-engine/src/table/table_manager_streaming.rs:229](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_streaming.rs#L229).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Namespace-wide serialization remains absent. Cover both rename names and parent changes in a consistent order; existing-table admission alone cannot cover creation. Avoid reacquiring the nonreentrant creation mutex through a newly locked facade method.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L52); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L50); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L217).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ambient Store-Read and validator WasmCompiler-Execute gates remain missing. Thread the effective actor and fail closed on actual authorization errors. System wrappers are deliberate, and legacy API removal is compatibility-sensitive; those are not prerequisites for closing the two gates.

Evidence: [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L22); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L221); [docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md#L111).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Durability and startup diagnostics remain unresolved. Add system-repository recovery before catalogue reads and a real WAL/history durability barrier; data_store.flush alone is insufficient. Builtin continue does not fix the independent catalogue-driven effective actor.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L112); [crates/shamir-engine/src/tx/commit.rs:973](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L973); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L324); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:995](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L995).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

FK guards retain failed/missing/malformed-row skips. A one-pass rewrite must inspect corruption diagnostics, distinguish legitimate schema absence from malformed presence, and prevent concurrent namespace/schema changes from invalidating the proof before mutation.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:269](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L269); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:273](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L273); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L177).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `partially-fixed`.

Prior-cycle decision: `partially-fixed`.

A registered read-fault replacement seam already exists. It does not replace repository MVCC routing and forwards writes, so lifecycle tests need a production-relevant history/WAL fault extension. Assert outcomes, old/new rows and reopened state, not bare is_err or graceful reopen.

Evidence: [crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs:741](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs#L741); [crates/shamir-engine/src/repo/repo_instance.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L377); [crates/shamir-engine/src/tx/commit.rs:968](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L968).

<a id="plan-p1-12"></a>

### Plan P1.12 — P1.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Isolation validation and fallible conversion are absent. Preserve valid snapshot/serializable behavior and add invalid-string rejection before BEGIN success. A closed error vocabulary is optional redesign, not restoration of the current optional-code/human-readable contract.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_tx.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L80); [crates/shamir-db/src/shamir_db/execute/helpers.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L73); [crates/shamir-query-types/src/batch/transaction_info.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/transaction_info.rs#L14).

<a id="plan-p1-13"></a>

### Plan P1.13 — P1.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Substring classification, Internal wrappers and pre-epoch unwrap remain. Preserve underlying resolution variants; Display alone may omit codes. Introducing shared Batch error types needs a dependency-direction proof. The interner returned-Err panic is not a current mechanism.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L148); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:772](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L772); [crates/shamir-db/src/shamir_db/execute/helpers.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L61).

<a id="plan-p1-14"></a>

### Plan P1.14 — P1.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Catalogue-only function existence is ignored. Read catalogue state fallibly before mutation, combine existence without skipping cleanup, preserve builtin/live-only semantics and propagate read failures rather than calling them absence.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L284); [crates/shamir-db/src/shamir_db/execute/admin_function.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_function.rs#L138).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

ACL snapshots are optional. The membership part requires more than warn logging: propagate genuine lookup failure and deny, since false can select broader Other rights. If caching is chosen, invalidate ownership, membership and create/drop/rename state coherently.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:916](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L916); [crates/shamir-types/src/access.rs:682](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L682).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The structural optimizations remain unimplemented. Preserve per-invocation authority and allowlist changes, table-before-recovery order, and distinct published-max versus contiguous-delta epoch semantics. No measured benefit is established.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:995](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L995); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:230](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L230); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:847](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L847); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L170).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Byte caps, shared-update semantics and dependency/docs cleanup remain. Do not rely solely on external curl max-filesize to bound host reads across unknown versions. Define in-flight revocation semantics; a DML-only definer restriction would be incompatible capability tightening.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L161); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:604](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L604); [crates/shamir-db/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L64); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L285).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Typed construction rationale, metadata semantics, duplicate creation, strict FK DTO parsing and migration reservation remain. Acquire creation locks once, preserve absent-field legacy FK defaults, and key migration reservation by database/repository/table with cleanup on every terminal path.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L39); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L52); [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:1343](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L1343); [crates/shamir-db/src/shamir_db/execute/admin_migration.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_migration.rs#L91).

<a id="plan-p2-19"></a>

### Plan P2.19 — P2.19

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ancillary logging and small hygiene remain. No-op version behavior is an API-policy choice; current username false sharing is refuted and guarded unwraps are safe. Keep successful mutation distinct from failed ancillary delta attachment.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:876](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L876); [crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs#L137); [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L40); [crates/shamir-db/src/shamir_db/curl_gateway.rs:228](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L228).

<a id="plan-p2-20"></a>

### Plan P2.20 — P2.20

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Actual import/constant/allow/semicolon hygiene remains. Private test visibility, port cohesion, mixed spelling and schema visibility require no mandatory change. Public flattening must retain compatibility; no commit or edit was authorized.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:759](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L759); [crates/shamir-db/src/shamir_db/shamir_db/mod.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/mod.rs#L15); [crates/shamir-db/src/shamir_db/mod.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/mod.rs#L7); [crates/shamir-db/src/shamir_db/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/mod.rs#L1).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — SystemStore catalogue WAL is not replayed before boot reads

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

SystemStore::init only calls DbInstance::add_repo, whose factory path constructs registrations without replay. ShamirDb immediately reads catalogue tables. A process crash after a catalogue commit reaches WAL but before history drain can therefore leave surviving committed metadata invisible at boot. Version-floor scanning reads WAL numbers, not values. Lazy drainer seeding may recover later after another operation starts it, too late for the already-built serving set. No crash/reopen experiment was run.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L112); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L194); [crates/shamir-engine/src/db_instance/db_instance.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/db_instance/db_instance.rs#L52); [crates/shamir-engine/src/repo/repo_instance.rs:719](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L719); [crates/shamir-engine/src/tx/drainer.rs:1017](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L1017); [crates/shamir-engine/src/tx/recovery.rs:329](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/recovery.rs#L329).

<a id="observation-new-2"></a>

### Observation NEW.2 — Successful repository rename discards declarative schema catalogue fields

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

rename_repo_as loads the full child record but rewrites it through save_table, which constructs only names, enable_indexes and ResourceMeta. schema/schema_validator_id/schema_version are lost even when every write succeeds. Live info-store bindings travel with the repository; after cold reopen boot_compile_schemas skips the now-schema-less rows, leaving persisted schema bindings without their compiled validator. Applicable writes then fail closed. The registered rename test covers data/indexes, not schema plus cold reopen.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:563](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L563); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:573](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L573); [crates/shamir-db/src/shamir_db/system_store.rs:365](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L365); [crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs#L660); [crates/shamir-engine/src/table/table_manager_validators.rs:331](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_validators.rs#L331); [crates/shamir-db/tests/rename_repo_e2e.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/rename_repo_e2e.rs#L51).

<a id="observation-new-3"></a>

### Observation NEW.3 — Relaxed catalogue reads can turn corrupt metadata into default-open ACLs

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

The normal unindexed metadata path enumerates opaque MVCC bodies. A malformed non-map body reaches RecordView rejection; read_streaming records corruption but returns a successful result without that row. SystemStore lookup discards corrupt_records and returns None, and resource_meta maps None to open metadata. With an already-live restricted database and healthy ancestor settings, its non-owner can consequently gain access. This requires malformed bytes admitted by the storage/log layer or catalogue mutation; it is not a claim that checksum-detected backend errors are swallowed or that a default client can manufacture corruption.

Evidence: [crates/shamir-engine/src/table/table_manager_streaming.rs:229](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_streaming.rs#L229); [crates/shamir-engine/src/query/filter/eval_bytes.rs:546](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/eval_bytes.rs#L546); [crates/shamir-types/src/record_view/lens.rs:767](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L767); [crates/shamir-engine/src/table/read_exec.rs:1635](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/read_exec.rs#L1635); [crates/shamir-db/src/shamir_db/system_store.rs:819](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L819); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L56).

<a id="observation-new-4"></a>

### Observation NEW.4 — Namespace Create permits replacement of another owner's private function

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

create_function_with_opts_as checks FunctionNamespace Create but never existing-function Write/Manage before replace=true; it overwrites both artifact and ResourceMeta with the caller. A User with database visibility and namespace Create can use a one-element nontransactional ForEach body to bypass the server's top-level-only coarse admin gate and replace a known System/other-user-owned function. This establishes cross-owner artifact takeover; automatic secret extraction is not claimed. Preserve explicit replacement/ownership policy and require a target permission if private ownership is intended to protect replacement.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_function.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_function.rs#L38); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L140); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:245](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L245); [crates/shamir-server/src/db_handler/handler.rs:512](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L512); [crates/shamir-query-types/src/batch/batch_op.rs:577](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L577); [crates/shamir-engine/src/query/batch/query_runner.rs:758](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L758); [crates/shamir-engine/src/query/batch/query_runner.rs:988](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L988).

<a id="observation-new-5"></a>

### Observation NEW.5 — Host source compilation blocks the async caller directly

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Both async creation methods directly call synchronous compile_rust_source. Its implementation performs filesystem work, toolchain probes and child.wait_timeout on the caller thread, without a facade spawn_blocking boundary. A permitted source submission therefore blocks a Tokio worker; a current-thread embedder blocks its whole runtime. This contradicts the inspected async/offload rule, without claiming measured starvation duration or CPU usage.

Evidence: [CLAUDE.md:338](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L338); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L172); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L221); [crates/shamir-wasm-host/src/compile.rs:454](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L454); [crates/shamir-wasm-host/src/compile.rs:592](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L592).

Grouping/duplicate: [../shamir-wasm-host/concurrency-lockfree.md#2](../shamir-wasm-host/concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="observation-new-6"></a>

### Observation NEW.6 — Repeated curl resolve lines discard earlier validated addresses

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

build_resolve_lines emits one same-host/port entry per IP and claims curl tries all in order. Exact published curl 8.10.1 Curl_loadhostpairs deletes the previous cache entry for that pair before inserting the next. Only the last validated address remains, potentially losing a working fallback; it does not introduce an unvalidated destination. The current multi-IP unit test asserts this incorrect text oracle. A single comma-separated address-list entry matches the inspected parser. Installed curl behavior was not executed.

Evidence: [crates/shamir-db/src/shamir_db/curl_gateway.rs:190](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L190); [crates/shamir-db/src/shamir_db/curl_gateway.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/curl_gateway.rs#L197); [crates/shamir-db/src/shamir_db/tests/curl_gateway_tests.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/curl_gateway_tests.rs#L37).

## Evidence and recipe corrections

- Current decision totals must be recomputed: several optional/design rows are not-applicable, curl is conditionally confirmed, and membership fallback is high under a concrete permission configuration.
- flush_all iterates only the public dbs registry; it neither flushes nor recovers the private SystemStore repository.
- Immediate catalogue durability is not repaired by copying existing data_store.flush calls after the MVCC overlay/history cutover.
- Five explicit Harness entries do not mean exactly five discovered benchmark targets; the root-level allocator helper is auto-discoverable.
- Normal top-level admin wire operations are superuser-gated. ForEach is not is_admin, and its body reaches the facade dispatcher, providing a concrete qualified ordinary-user route.
- Write-before-remove prevents one absence window, not multi-row atomicity; persisted old rows remain active inputs to boot.
- The existing ACL replacement seam is real but requires correct MVCC/history/WAL routing for lifecycle write-failure acceptance.
- Do not retain blanket fail-closed ACL assurances: membership errors and discarded corruption diagnostics can select permissive outcomes.
- No current dependency pin identifies the installed curl; the compatible-version parser witness and deployment verification are separate facts.

## Module scope and limitations

Coverage: 8 assigned documents, 143 current claim rows, 20 plan rows, 0 pre-existing observation rows; 6 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- All eight documents, including their complete historical sections, were inspected. No current NEW.N observation headings or TASK_GROUPS.md exist in the assigned directory.
- Read-only inspection only: no files changed and no compiler, test, benchmark, reproduction, server, dependency operation or child agent ran.
- HEAD matched the required revision; inspected source and Cargo.lock have no changes relative to the reports' stated source snapshot.
- Runtime measurements, test success, exact resource-exhaustion thresholds and exact crash-loss intervals were not established.
- The deployed curl executable remains unidentified. Published curl 8.10.1 and 8.14.1 parser source establishes a compatible-version injection witness, not execution on this machine.
- This is bounded claim revalidation. New findings arise from the inspected guarantees and call chains, not an exhaustive workspace security audit.

## Guarantee checks

- **Repeated batch authorization requirements are deduplicated in both facade execution paths.** — `supported`. Both callers mint Authorized; its TFxSet skips repeated action/path pairs. The initial Database-Read check remains outside that set. History adb0c8fd shows removal of the old loop; current source proves the fix. Reference: crates/shamir-engine/src/query/batch/authorized.rs:98.
- **Catalogue failures cannot produce an authorization grant.** — `diverges`. Explicit resource_meta Err is denied, but membership Err becomes Other-class access, and relaxed catalogue reads can discard corrupt rows before the facade treats their absence as open metadata. Reference: crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:851.
- **DDL catalogue mutations are immediately durable.** — `diverges`. Implicit commits append Buffered WAL and publish overlays. Flushing the vestigial data store neither drains history nor synchronizes the separate WAL. Published fjall 3.1.6 src/db.rs persist(SyncAll) concerns its own journal; its cached archive checksum matches Cargo.lock:1335. Reference: crates/shamir-db/src/shamir_db/system_store.rs:282.
- **WAL recovery occurs before catalogue data determines the serving set.** — `diverges`. SystemStore::init attaches its repository but does not recover it before load_databases/load_repositories. User repositories have explicit recovery; WAL version-floor scanning is not replay. Reference: crates/shamir-db/src/shamir_db/system_store.rs:89.
- **Replacing a validator preserves stable identity and table bindings.** — `diverges`. replace_artifact provides that operation, but both facade replacement implementations rebuild empty bound_in and remove the old registry entry. WASM replacement also lacks catalogue identity fallback. Reference: crates/shamir-engine/src/validator/registry.rs:104.
- **Table/repository rename preserves catalogue metadata and rejects dangling foreign keys.** — `diverges`. FK discovery skips failed/malformed sibling records. Repository rename rebuilds child rows through save_table rather than preserving schema fields. Write-before-remove ordering alone does not establish atomicity. Reference: crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:171.
- **System convenience wrappers constitute a current authentication bypass.** — `diverges`. The documented embedded trust boundary deliberately permits privileged convenience APIs, and inspected server handlers use actor-aware calls. Misusing a wrapper in a future handler is a prospective integration risk, not an existing bypass. Reference: docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:111.
- **All errors require a closed machine-readable code and abort reasons contain only codes.** — `diverges`. QueryError.code is explicitly optional; TransactionInfo.reason is explicitly human-readable. Normalization would introduce a new contract. Reference: crates/shamir-query-types/src/batch/batch_error.rs:36.
- **Only WasmCompiler-Execute-authorized actors can trigger host source compilation.** — `diverges`. The function source path checks the singleton; the validator source path does not. Top-level wire admin requests are superuser-gated, but a nontransactional ForEach body reaches facade admin dispatch without that coarse recursive gate. Reference: crates/shamir-wasm-host/src/compile.rs:31.
- **Curl configuration escaping prevents guest-controlled directives.** — `diverges`. Raw LF survives escaping. The [curl 8.14.1 parser](https://raw.githubusercontent.com/curl/curl/curl-8_14_1/src/tool_parsecfg.c) reads physical lines and accepts an unterminated quoted parameter; an injected following directive and comment suffix are parsed independently. Reference: crates/shamir-db/src/shamir_db/curl_gateway.rs:210.
- **Every validated DNS address remains available to curl in order.** — `diverges`. Repeated same-host/port resolve entries replace earlier addresses in [curl 8.10.1 Curl_loadhostpairs](https://raw.githubusercontent.com/curl/curl/curl-8_10_1/lib/hostip.c). The final address remains validated, but the claimed multi-address fallback is absent. Reference: crates/shamir-db/src/shamir_db/curl_gateway.rs:190.
- **The crate has exactly five benchmark targets and the allocator helper is not a target.** — `diverges`. There are five explicit Harness targets, but edition 2021 leaves autobenches enabled; benches/bench_allocator.rs is also discoverable. This follows the [Cargo target-discovery contract](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#target-auto-discovery), without running Cargo. Reference: crates/shamir-db/Cargo.toml:108.
- **Registered test organization establishes coverage of the reported lifecycle failures.** — `diverges`. The four unit trees and ddl_wire_e2e integration harness are registered. Default test.sh selects lib tests; integration targets require full selection. Registration and happy-path roundtrips do not establish the missing adverse-state oracles. Reference: crates/shamir-db/src/shamir_db/tests/mod.rs:1.
- **Facade execution is universally lock-free.** — `diverges`. No direct forbidden synchronous lock type was found in facade production source, but exact dashmap 6.1.0 src/lib.rs stores RwLock-protected shards. Published archive checksum matches Cargo.lock:1099; concurrent-map use is not universal lock-free execution. Reference: crates/shamir-db/src/shamir_db/shamir_db/core.rs:51.
- **The persisted WASM hash is an integrity control.** — `diverges`. The field is write-only metadata. Exact rustc-hash 2.1.2 identifies FxHasher as noncryptographic; its published archive checksum matches Cargo.lock:3010. Function version remains 1, while validator creation writes no version field. Reference: crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:187.
- **to_qv is a universally lossless conversion.** — `diverges`. Both encoding and decoding errors collapse to Null. Exact rmp-serde 1.3.1 decode.rs from_slice delegates to its deserializer; container dispatch can return decoding errors. Its published archive checksum matches Cargo.lock:2952. No presently failing event DTO was demonstrated. Reference: crates/shamir-db/src/shamir_db/execute/helpers.rs:67.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- security-crypto.md#1, SUMMARY.md#3.1 and Plan P0.2 change from unverified to confirmed-open for compatible curl versions: published physical-line parsing and unterminated-quote handling provide the missing mechanism proof. Installed-version verification remains a limitation.
- error-handling-lifecycle.md#11 and SUMMARY.md#6.11 remain open but increase from low to high: mode 0704 supplies a concrete deny-to-grant membership-error witness. The logging-only plan is insufficient.
- api-wire-protocol.md#3/#5, security-crypto.md#6/#9, concurrency-lockfree.md#4 and their SUMMARY counterparts change from confirmed-open to not-applicable: privileged embedded APIs, optional error metadata, definer delegation and snapshot architecture are documented behavior or optional design choices.
- style-claude-md.md#7/#8 and SUMMARY.md#7.7/#7.8 change from confirmed-open to not-applicable because no normative or behavioral violation supports the optional spelling/visibility preferences.
- The prior assurance that bench_allocator.rs is not an additional benchmark target is contradicted by edition-2021 auto-discovery and the absence of autobenches=false.
- The prior durability explanation overcredits flush_all: it excludes SystemStore. A successful graceful reopen with ignored operation results is not proof of immediate catalogue crash durability.
- The original missing ACL dedup mechanism is genuinely fixed. Existing interner-error counter-evidence, private-test registration and cohesive-port support are pre-existing refutations/non-findings, not fixes.
- All current rows were checked independently before producing these verdicts; historical green labels and first-cycle conclusions were not treated as proof.

## Current follow-up order

1. Correct cross-database CASCADE targeting and cross-owner artifact replacement authorization; include actual server recursion paths in acceptance.
2. Repair conditional ACL fail-open paths: membership lookup errors and corrupt catalogue rows must not become broader permissions.
3. Recover SystemStore WAL before boot reads and establish genuine catalogue WAL/history durability; preserve schema metadata through repository rename.
4. Preserve validator identity/bindings and repair catalogue mutation/rename error outcomes with pre/post-commit-discriminating oracles.
5. Reject absent groups under locks, serialize namespace mutations, fail closed in FK discovery and reject unknown isolation strings.
6. Reject curl configuration controls, bound response bytes, correct multi-address pin construction, enforce validator compiler permission and offload synchronous compilation.
7. Keep ACL dedup closed; address remaining structural scans with visibility-safe reads and evidence before performance claims. Separate optional API/style preferences from defects.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-db — Synthesized 7-lens review (consolidated SUMMARY)

Crate: `crates/shamir-db/` — the database facade (`ShamirDb::execute`/`tx_*` over
`BatchRequest`/`BatchOp`): system catalogue (`SystemStore`), ACL enforcement, the
function/validator registries, admin wire handlers, and the curl egress gateway.

Review basis: the seven 2026-08-14 cross-crate lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md`. This document **synthesizes** them (dedup + prioritization);
it is not a fresh review. Format calibrated against the two completed exemplars,
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. Workspace
context (not copied): `../SUMMARY.md` files shamir-db at **71 lens-tagged findings,
0c / 10h**, verdict "needs focused remediation — silent catalogue corruption
(cascade mis-target, phantom writes, swallowed renames)", with the CASCADE
wrong-target and curl-gateway CRLF injection both in the workspace-wide P0.
Read-only pass — no build/test/lint commands; no source modified. Spot-checks
performed to verify headline file:line references (all confirmed; one genuine new
defect found and added below, marked in place).

Finding numbering below is `<lens>.<source-finding>` and matches the source files'
own numbering 1:1, so every entry can be traced back to its full write-up.

## Executive summary

The largest facade in the workspace and structurally among the most
CLAUDE.md-conformant (no banned locks, `THasher` everywhere, exemplary
rationale-comment culture, honest Red/Green tests), but the high band is dominated
by one theme — **silent catalogue corruption**: the `DROP DATABASE … CASCADE`
wrong-target argument destroys a *different* database's tables and validator
bindings, catalogue-write failures are `warn!`-and-continued so renames can return
`Ok(())` half-migrated (with three renames doing remove-*before*-write, the inverse
of the crate's own crash-safety convention), a `replace=true` validator wipes
`bound_in` and re-enables an unsafe drop, and absent-group writes fabricate phantom
records. Fix first: (1) the CASCADE wrong-target plus the curl-gateway CRLF
injection — both workspace-wide P0 and wire/operator-reachable; (2) the
swallowed-write / remove-first rename family and the Durable-DDL flush gaps;
(3) the validator replace path and the phantom-group fabrication. The second theme
is cost, not corruption: the ACL gate re-pays full catalogue scans per ancestor per
op, and the inline dedup cache that `tx_execute_as` already has was never ported to
the hotter `execute_as`.

---

## 1. correctness-tdd

*Source: `correctness-tdd.md` (12 findings). Lens verdict: Red/Green/Refactor
discipline is visibly honoured and no vacuous-test pathologies were found; gaps are
specific reachable edge cases listed per finding.*

### 1.1 [HIGH] `DROP DATABASE … CASCADE` executed against a different database destroys the *batch's* database's tables *(workspace P0 #9)*
- **File:line:** `src/shamir_db/execute/admin_db_repo.rs:126-141` (wrong arg at
  **:133** — spot-check confirmed `&self.db_name` passed while the loop enumerates
  `op.drop_db`'s repos); cf. `src/shamir_db/shamir_db/table_management.rs:118-137`.
- **Issue:** the cascade loop enumerates `repos`/`tables` from `db = get_db(&op.drop_db)`
  but invokes `drop_table_cleaning_validators(&self.db_name, repo, table)` — the
  batch's database, not `op.drop_db`. Admin ops carry no `table_ref`, so a
  `DropDb("B")` op inside a batch executed on "A" is authorized against B
  (correct) but executed against A (wrong).
- **Failure scenario:** `execute("A", batch{ drop_db("B", cascade=true) })` where
  both A and B have repo `main`/table `items`: A's live validator bindings are
  stripped and persisted, A's `items` table is dropped for real, A's catalogue row
  is deleted — then `remove_repo("B", …)` runs; every step's error is swallowed by
  `let _ =`.
- **Fix:** pass `&op.drop_db` (or reject `op.drop_db != self.db_name` with a typed
  error); stop swallowing cascade-step errors (see 6.5).

### 1.2 [HIGH] `remove_group_member` on a nonexistent group fabricates a phantom record; the wire remove path lacks the add path's guard *(primary — also flagged by error-handling 6.4 and concurrency 2.5)*
- **File:line:** `src/shamir_db/system_store.rs:742-762` (spot-check confirmed:
  `load_group` → `Ok(None)` tolerated → `unwrap_or_default()` members/name,
  `unwrap_or(Actor::System)` owner → unconditional `save_group`; also
  `add_group_member` :712-734); wire gap at
  `src/shamir_db/execute/admin_access.rs:454-489` (guard exists only on add,
  :390-395).
- **Issue:** a remove against a missing group **writes** a record with empty name,
  empty members, owner System. `handle_add_group_member` guards this explicitly;
  `handle_remove_group_member` only resolves + authorizes (Root-Manage
  short-circuits before any existence check). The test at
  `admin_access_validation_tests.rs:479-486` asserts the wrong invariant for
  remove; the nonexistent-*group* case is untested (the one test removes a
  non-member *user* from an *existing* group). The concurrency lens adds the TOCTOU
  facet: existence is validated *before* the per-group RMW lock, so a completed
  concurrent `drop_group` is resurrected by the later `save_group` — same
  `None`-tolerant store behaviour, same fix.
- **Failure scenario:** `REMOVE GROUP MEMBER (group = Id{999_999}, user = 1)` via
  wire returns `Ok` and fabricates group 999999 with name `""`; it occupies the
  counter namespace, shows up unnamed in `access_tree`, and can't be addressed by
  name. Or: admin A drops group 7 while B's rename/add on 7 is mid-flight → a
  zombie group reappears, invisible to the operator who deleted it.
- **Fix:** `SystemStore::add_group_member`/`remove_group_member` return
  `DbError::NotFound` when absent (as `set_group_owner` :768-772 already does);
  re-load under the per-group lock in `rename_group_as`/`add_group_member_as`;
  add the Red test (remove-from-nonexistent-group must not create a row).

### 1.3 [MEDIUM] `save_database`/`remove_database` and all replication-catalogue writes skip the "Durable DDL" flush; a test comment asserts they don't
- **File:line:** `src/shamir_db/system_store.rs:187-207`, `:210-223`;
  `src/shamir_db/execute/admin_replication.rs:87-101, 168-182, 278-292, 393-407`;
  wrong comment `tests/rename_db_e2e.rs:309-314`.
- **Issue:** every other system-catalogue write ends with `data_store().flush()`
  under the "Durable DDL" comment; the database row and all four replication
  handlers stop at `interner().persist()`. `rename_db_e2e.rs:312` states
  save_database "already calls flush internally" — false — and the test only
  survives because it calls `flush_all()` by hand.
- **Failure scenario:** `DROP DATABASE x` (or `CREATE PUBLICATION`) followed by a
  crash inside the ~500 ms MemBuffer window resurrects the dropped db row on next
  boot (boot registers a `DbInstance` per persisted row) or loses the replication
  definition, with no error anywhere.
- **Fix:** add the flush (or route through the set+flush helper) in both methods
  and the four replication handlers; fix the comment; add a TDD test that
  crash-simulates without an explicit `flush_all`.

### 1.4 [MEDIUM] Boot silently skips repository rows whose database row is missing — no warning, data invisible forever *(primary — also flagged by error-handling 6.7, folded in)*
- **File:line:** `src/shamir_db/shamir_db/core.rs:210-219` (spot-check confirmed:
  `if let Some(db) = shamir.get_db(db_name)` with no `else`); contrast the
  adjacent warn-logged `add_repo`-failure branch :243-252.
- **Issue:** a repo row whose `(db)` row is absent (crash between
  `save_repository` and `save_database`; the unflushed-delete resurrection of 1.3
  in the opposite direction; manual surgery) is skipped in total silence. The
  error lens adds: an attach *failure* (e.g. fjall directory lock held by a second
  process) is `warn!`+`continue`, while recovery failure aborts boot ("must not
  serve") — an undocumented durability asymmetry.
- **Failure scenario:** a durable fjall repo with committed WAL data exists on
  disk and in `repositories`, but its db row was lost; every restart skips it
  without a log line. The operator sees "my database came back empty"; a later
  `create_db` of the same name does not re-attach it (repos re-attach only at
  boot). Or: restart while a stray process holds the repo lock → boots green,
  first query on the repo fails with a confusing runtime `NotFound`.
- **Fix:** `else { log::warn!("skipping orphan repo row …") }` matching
  `unresolved_native_artifacts`; record failed-attach repos in a startup
  diagnostic (or propagate for disk-backed repos); consider a `recover_orphans`
  helper.

### 1.5 [MEDIUM] Boot "skipping" a builtin-name-colliding function row still overwrites its `function_meta`; the live artifact diverges from the catalogue after restart
- **File:line:** `src/shamir_db/shamir_db/core.rs:318-333` (register-failure
  branch :324-329, unconditional meta insert :331-332).
- **Issue:** for a catalogue row named like a builtin (creatable pre-restart via
  `create_function_with_opts_as` with `replace=true`, which does overwrite the
  builtin), `functions.register` fails, the log says "skipping catalogue load" —
  but `function_meta.insert(...)` runs anyway. Post-restart the registry serves
  the builtin while `function_meta` (grants) and `effective_fn_actor`'s
  SECURITY DEFINER escalation use the *user's* row.
- **Failure scenario:** `CREATE OR REPLACE FUNCTION argon2id … SECURITY DEFINER`
  runs the replacement; after restart `argon2id` silently executes the builtin
  again — escalated to the record's owner and with the replaced row's grants.
- **Fix:** `continue` before touching `function_meta`; surface the collision in
  `unresolved_native_artifacts`-style diagnostics; longer term honour `replace`
  consistently at boot.

### 1.6 [→ 6.2] `rename_function_as` / `rename_validator_as` re-key remove-first *(dedup: primary write-up at 6.2, severity HIGH there)*
Same root-cause defect as error-handling #2; see 6.2 (which also covers
`rename_function_folder_as`).

### 1.7 [LOW] `create_db_as` / `create_db` silently overwrite an existing live `DbInstance`
- **File:line:** `src/shamir_db/shamir_db/db_management.rs:37-68`.
- **Issue:** the wire path holds `db_create_lock` and guards on `has_db` (#546),
  but the public `create_db_as` does `dbs.insert(name, db)` unchecked — a second
  caller replaces the registered instance, dropping every repo handle and open
  table; `save_database` failures are demoted to `warn!`.
- **Failure scenario:** boot/tooling calls `create_db("main")` while a wire-created
  durable `main` exists: the live instance with attached repos is dropped from the
  registry; reads see an empty database until restart.
- **Fix:** take `db_create_lock`, return `KeyExists` (or accept `if_not_exists`);
  propagate `save_database` errors.

### 1.8 [LOW] `drop_function_as` returns `existed = false` while deleting a durable catalogue-only function
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:274-296`; the
  registry-only `if_exists` early-exit compounds it at
  `src/shamir_db/execute/admin_function.rs:138`.
- **Issue:** `existed` comes solely from the live registry; `kind = Native` rows
  are deliberately not materialised at boot (`core.rs:290-296`), so a
  catalogue-only function is deleted while reporting `{"existed": false}` — and an
  `IF EXISTS` drop never deletes it at all.
- **Failure scenario:** operator lists a native function, issues
  `DROP FUNCTION IF EXISTS`, gets `existed: false`, and the row still resurrects
  artifacts on next boot.
- **Fix:** `existed = registry.remove(...) || load_function(name).is_ok_some()`;
  include the catalogue in the `if_exists` probe.

### 1.9 [→ 5.1] `create_validator_inner` replace path resets `bound_in` and can mint `RecordId::default()` *(dedup: primary write-up at 5.1, severity HIGH there)*
Same root-cause defect as api-wire #1; the catalogue `_id` fallback detail
(asymmetry vs `register_native_validator_as` :94-107) is folded into 5.1.

### 1.10 [LOW] Panic paths in library code: `.expect("interner touch_ind")` and `.unwrap()` on `SystemTime` *(primary — error-handling 6.9 flags the SystemTime half)*
- **File:line:** `src/shamir_db/execute/admin_schema.rs:1038`;
  `src/shamir_db/execute/helpers.rs:58-63` (the sole production `.unwrap()` in the
  crate; sibling sites `db_management.rs:41-44`, `admin_migration.rs:83-86` use
  `.unwrap_or_default()`).
- **Issue:** `serialise_one_rule_catalogue` panics on interner failure — the same
  failure one line away in `handle_interner_touch` (`admin_interner.rs:158-164`)
  is correctly mapped to `BatchError`; `admin_result_with_op_id` unwraps
  `duration_since(UNIX_EPOCH)` (environmental, not an invariant — CLAUDE.md
  allows `panic!` only for programmer bugs).
- **Failure scenario:** a poisoned/full interner during `SetTableSchema` panics
  the request task; a pre-epoch clock panics every DDL response builder.
- **Fix:** map `touch_ind` through `err(...)`; `.unwrap_or_default()` for epoch
  time.

### 1.11 [LOW] `foreign_key_dto_from_qv` silently coerces unknown `on_delete`/`on_update` strings to `NoAction` — then re-persists them
- **File:line:** `src/shamir_db/execute/admin_schema.rs:1332-1360` vs the
  hard-error boot parser `parse_fk_action` at
  `src/shamir_db/shamir_db/schema_management.rs:371-397`.
- **Issue:** F-4 made unrecognised FK actions a hard error at boot so "garbled"
  can't be downgraded to `NoAction`; the DDL RMW read path still has a
  `_ => FkAction::default()` arm, and `ADD`/`REMOVE SCHEMA RULE` round-trip the
  whole rule list through it and re-serialise.
- **Failure scenario:** a row persisted by a newer binary with an added action
  (`"set_default"`) is silently rewritten to `NoAction` by an older binary
  handling an unrelated schema edit — referential semantics change without any
  error.
- **Fix:** reuse `parse_fk_action` (or fail the DTO) so the RMW handlers reject
  instead of normalise.

### 1.12 [NIT] Bundle: silent isolation fallback / no-op version bump / shared lock namespace
- **File:line:** `src/shamir_db/execute/db_tx.rs:82-85`;
  `src/shamir_db/execute/admin_schema.rs:860-876`, `:77-98`.
- (a) `tx_begin_as` maps *any* unknown isolation string to `Snapshot` — dedup:
  primary write-up at **5.6** (medium there).
- (b) `handle_remove_schema_rule` bumps `schema_version` and rewrites the catalogue
  (with rollback machinery) even when `removed == false` — skip the persist/bump.
- (c) `admin_user_locks` multiplexes bare usernames and `"schema:db/repo/table"`
  keys in one namespace; a username shaped like `schema:a/b/c` false-shares a lock
  — give schema locks their own map or document the namespace (the map's doc says
  "keyed by user_name").

*Test-coverage note:* strong barrier-synchronised race regressions and honest
"currently unenforced" pins; gaps mirror findings 1.1-1.6/1.8/1.10 (cross-db
cascade, phantom group write, durability without external `flush_all`, orphan repo
row, builtin collision across restart, fault-injected rename failure,
catalogue-only drop semantics).

---

## 2. concurrency-lockfree

*Source: `concurrency-lockfree.md` (8 findings). Lens verdict: pillar-clean on
primitives — no banned locks anywhere, DashMap+`THasher` throughout, shard guards
dropped before `.await`, documented lock order; the weaknesses are O(x→0) and
check-then-act (TOCTOU) axes.*

### 2.1 [HIGH] `execute_as` re-runs the full async ACL traversal per batch op — the inline dedup cache its sibling `tx_execute_as` established was never ported *(primary — performance-hotpath 4.2 flags the same defect as its #2)*
- **File:line:** `src/shamir_db/execute/db_execute.rs:64-68` (spot-check
  confirmed: bare `for (action, path) in collect_required_access(...)` loop);
  contrast the stack-local `FxHashMap<(ResourcePath, Action), bool>` cache at
  `src/shamir_db/execute/db_tx.rs:140-167`.
- **Issue:** the per-request hot path authorizes once at batch level, then again
  per (action, path) pair with no memoization. Each non-System
  `authorize_access` walks all ancestors, each resolved via separate async
  system-store reads (`access_control.rs:825-908`) plus group lookups — ~4-6+
  storage round-trips per op before the query starts, where `tx_execute_as`'s
  comment prices repeats at ~50 ns. O(N × catalogue-reads) where O(1) is
  achievable, on the hottest path in the crate.
- **Failure scenario:** non-System actors pay ~8-10 system-table reads per
  single-op request; a 1000-insert batch pays 1000 full traversals (see 4.1 for
  what each traversal costs); concurrent load multiplies latency and system-repo
  I/O for zero added security.
- **Fix:** port the `FxHashMap<(ResourcePath, Action), bool>` dedup verbatim
  (stack-local, dropped at exit); hoist the batch-level Database-Read result into
  the same cache.

### 2.2 [MEDIUM] Table-level DDL (CREATE/DROP/RENAME TABLE) has unserialized check-then-act races; the race is acknowledged by a `debug_assert!`
- **File:line:** `src/shamir_db/execute/admin_table_index.rs:50-69` (create),
  `:104-297` (drop), `:299-350` (rename);
  `src/shamir_db/shamir_db/table_management.rs:216-228` (guards), `:314-317`
  (`debug_assert!(existed, "rename_table_stores returned false despite has_table
  guard")`).
- **Issue:** #546 closed create-db/create-repo TOCTOU with per-name
  `tokio::sync::Mutex`es; table ops got no equivalent — create/drop/rename
  each check-then-act with nothing serializing them against each other, and the
  `debug_assert` codifies the race as anticipated.
- **Failure scenario:** two concurrent `RENAME TABLE from→to` both pass the
  guards: debug builds panic the loser on the assert; release builds write a
  second `(db, repo, to)` row and remove the already-removed `from` row —
  duplicate/dangling catalogue state, or a racing `CREATE TABLE to` producing two
  live registrations under one name.
- **Fix:** extend the #546 pattern to a
  `DashMap<(String,String,String), Arc<Mutex<()>>, THasher>` keyed (db, repo,
  table) around the guard→mutate window, or route through the engine's
  `ddl_admission`/`begin_write_barrier` entry point used by the index path.

### 2.3 [MEDIUM] DROP/RENAME of databases and repos bypass the #546 create locks — asymmetric serialization axes
- **File:line:** `src/shamir_db/execute/admin_db_repo.rs:215-221` (create-repo
  takes the lock) vs `:77-149` (drop-db, no lock), `:321-397` (drop-repo, no
  lock), `:399-482` (renames, no lock); `db_management.rs:157-162` (rename_db's
  transient `remove`/`insert` window), `:431-452` (`remove_repo`).
- **Issue:** the #546 fix serializes create-vs-create only; drop-vs-create and
  rename-vs-create on the same name are unserialized, and `rename_db_as`
  transiently removes the source key from `dbs` outside any lock.
- **Failure scenario:** concurrent `CREATE REPO IF NOT EXISTS r` + `DROP REPO r`:
  the create passes its guard under the lock while the drop proceeds unlocked →
  an in-memory repo whose catalogue row was removed resurrects on next `init`, or
  the create's catalogue write lands after the drop's removal, resurrecting a
  "dropped" repo durably. Same shape for `CREATE DATABASE to` racing
  `RENAME DATABASE from→to`.
- **Fix:** acquire the existing `db_create_lock`/`repo_create_locks[db]` entries
  in the drop/rename handlers — completing the documented pattern, not inventing
  one (`core.rs:88-103` already frames them as the per-name serialization point).

### 2.4 [LOW] Per-request ACL metadata is re-read from storage on every authorization — no lock-free snapshot (pillar-5 `ArcSwap` fit)
- **File:line:** `src/shamir_db/shamir_db/access_control.rs:122-179`
  (`load_setting(...)` per gate evaluation), `:55-91` (per-ancestor record loads).
- **Issue:** even with 2.1's dedup and 4.1's keyed lookups, every fresh
  authorization re-reads read-mostly/write-rarely catalogue rows; fail-closed
  semantics couple data-plane availability to system-store I/O.
- **Failure scenario:** sustained non-System load keeps a constant read stream on
  the system repo; a slow/flushing system store directly inflates every request's
  gate latency, and a transient storage error *denies* the request.
- **Fix:** `ArcSwap<HashMap<ResourcePath, ResourceMeta, THasher>>` published on
  miss and re-published by `set_resource_meta`/DDL — must be invalidated on every
  mutation arm (a stale-positive ACL cache is an auth bypass; ship with an
  invalidation test) and must preserve fail-closed `Err`→deny for genuine storage
  errors.

### 2.5 [→ 1.2] Group existence validated *before* acquiring the per-group RMW lock — a completed concurrent drop can be resurrected *(dedup: primary write-up at 1.2)*
Same root-cause defect as correctness #2 (`None`-tolerant group member writes +
missing re-load under the lock); the `rename_group_as` lock-placement detail
(`access_control.rs:573-586`, `:604-627`) is folded into 1.2's fix.

### 2.6 [LOW] `SystemStore::load_repository_record` full-catalogue scan + linear filter for a keyed lookup
- **File:line:** `src/shamir_db/system_store.rs:334-354` vs the filtered
  `load_repository` at `:828-857`.
- **Issue:** loads every `repositories` row to match one `(db, repo)` pair that is
  the storage key; hidden O(N) helper (pillar 3). Only used by the rare
  rename-repo path, hence low. (Instance of the 4.1 scan family.)
- **Fix:** replace the body with `self.load_repository(db, repo)` (identical
  `Ok(None)` semantics) or delete in favour of the direct call.

### 2.7 [LOW] `handle_start_migration`'s duplicate-start guard is check-then-act over `active_migrations`
- **File:line:** `src/shamir_db/execute/admin_migration.rs:91-101` (probe) and
  `:193-195` (insert after the whole snapshot/drain await sequence);
  acknowledged at `core.rs:549-552`.
- **Issue:** two concurrent `StartMigration` ops can both pass the `iter().any()`
  probe and both insert coordinators; explicitly documented debt of the
  experimental, opt-in API — recorded so it isn't forgotten when hardened.
- **Failure scenario:** double snapshot/drain against the same source table;
  messy but contained (rollback removes only its own dst name).
- **Fix:** reserve the slot atomically before the snapshot (`entry().or_insert`
  claim or an `AtomicBool` state machine), released on rollback.

### 2.8 [NIT] `resolve_group_id(GroupRef::Name)` is a full `load_groups()` scan per call
- **File:line:** `src/shamir_db/shamir_db/access_control.rs:775-783`.
- **Issue:** name→id resolution scans the whole groups table; reached from
  `resource_meta`'s Group arm during `authorize_access` for group-managed
  resources; undocumented on cost. (Complements the 4.1 family — group names are
  unique, so an O(1) point path exists.)
- **Fix:** `Filter::Eq` point read, or a `DashMap<String, u64, THasher>` mirror
  maintained at create/rename/drop (the AtomicUsize-mirror precedent).

*Test-coverage note:* the mechanisms that exist are proven deterministically
(`execute_tests.rs:779-997` lock blocking; `group_tests.rs:64-500` RMW windows;
`keyset_safe_write_barrier_tests.rs` F-37 barrier). None of the gaps above is
tested: drop-vs-create/rename-vs-create races (2.2-2.3), completed-drop
resurrection (2.5/1.2), ACL cost under batch load (2.1).

---

## 3. security-crypto

*Source: `security-crypto.md` (9 findings). Lens verdict: crypto correctly
delegated to the injected `UserAdminPort`, zero `unsafe`, fail-closed ACL and #995
ordering consistently applied, DNS-rebind-pinned SSRF guard; the defects are at
untrusted-input boundaries.*

### 3.1 [HIGH] Guest-controlled header values/method can inject arbitrary curl config directives (CRLF injection) *(workspace P0 #10)*
- **File:line:** `src/shamir_db/curl_gateway.rs:83-89` (header interpolation),
  `:71-75` (url/method), `:210-220` (`escape_curl_value` — spot-check confirmed:
  escapes only `\` and `"`).
- **Issue:** curl config files have no escape for line breaks; a raw `\n`/`\r`
  in a value terminates the config line and the rest parses as new top-level
  directives. `HttpRequest.headers`/`.method` are guest-controlled (third-party
  WASM logic) and reach the config file unfiltered for CR/LF. Quoted values can't
  be closed by the attacker, but unquoted directives need no quotes.
- **Failure scenario:** a guest calls `ctx.http_fetch()` with a header value
  containing `\nproxy = http://<internal>:8080\n` → curl routes the request
  through the attacker's proxy, bypassing the SSRF guard's `--resolve` pinning
  (which constrains destination resolution, not proxying) → loopback/metadata
  access and TLS-intercepted egress; `output = <path>` / `config = <file>` give
  file-write redirection and nested-config loading. The URL path is largely
  protected (WHATWG parsing strips tab/newline); headers/method are not.
  `curl_gateway_tests.rs:6-18` covers only `\` and `"` — the injection class is
  untested.
- **Fix:** reject (not escape) CR, LF, and other C0 controls in every
  guest-supplied string written into the config file, returning a typed egress
  error; unit tests: newline-bearing header rejected; generated `curl.cfg` never
  contains a raw `\n` inside a value.

### 3.2 [MEDIUM] Ambient interner delta exposes any repo's field-name dictionary without Store-level authorization
- **File:line:** `src/shamir_db/execute/ambient_interner.rs:22-57`; called from
  `db_execute.rs:98-102`.
- **Issue:** `execute_as` attaches `response.interner_delta` for every
  client-requested `(repo, epoch)` with **no** `authorize_access` on the named
  repo; the explicit admin op for the same data, `handle_interner_dump`
  (`admin_interner.rs:50-57`), requires `Action::Read` on `ResourcePath::Store` —
  the delta is an ACL-free side door to the same resource.
- **Failure scenario:** an actor with Read on database `app` but no access to
  repo `app/hr` sends any batch with `interner_epochs: {"hr": 0}` and receives
  `hr`'s complete interned field-name vocabulary (schema shape, internal
  attribute names).
- **Fix:** skip repos failing `authorize_access(actor, store(db, repo), Read)`
  in `attach_interner_delta` (actor is available at the call site); ACL test for
  the denied case.

### 3.3 [MEDIUM] Validator Rust-source path bypasses the WasmCompiler Execute gate (task #607)
- **File:line:** `src/shamir_db/shamir_db/validator_management.rs:210-229`
  (`create_validator_inner`, `Source` arm); wire entry
  `execute/admin_validator.rs:36-43`; contrast the gated function path
  `function_management.rs:161-177`.
- **Issue:** `create_function_with_opts_as` gates `FunctionSource::Source` behind
  `authorize_access(WasmCompiler, Execute)` (#607: compiling Rust source runs a
  host compiler process); the structurally identical validator path calls
  `compile_rust_source` with no authorization, and `handle_create_validator`
  checks only Create on FunctionNamespace. `wasm_compiler_permission_tests.rs`
  covers the function path only.
- **Failure scenario:** operator hardens `WasmCompiler` to `0o700`; a user holding
  FunctionNamespace-Create (default `0o777`) still triggers arbitrary host
  toolchain builds (cargo executes build scripts/proc-macros — host code
  execution at compile time) by submitting `create_validator` with `source`.
- **Fix:** add the same WasmCompiler-Execute check in the validator `Source` arm;
  extend the permission tests with the validator analogue.

### 3.4 [LOW] Egress response body read without any size cap
- **File:line:** `src/shamir_db/curl_gateway.rs:100` (`max-time = 30` only),
  `:156-167` (`read_to_end`).
- **Issue:** egress is capped by time, not size; the dumped response file is
  slurped into memory unbounded and no `--max-filesize` is set.
- **Failure scenario:** a function fetching from an allowlisted (or compromised
  allowlisted) host receives a multi-GB response; the host process OOMs/degrades,
  taking down the whole DB server — guest-triggerable amplification with no
  guest-visible limit.
- **Fix:** `max-filesize` in the generated config and/or stream-copy with a byte
  budget (typed egress error on exceed), consistent with `WasmLimits`.

### 3.5 [LOW] Dead TLS/password-hash dependencies with a stale "kept compiling" rationale *(primary — api-wire 5.8 flags the same defect plus the false `net` doc)*
- **File:line:** `Cargo.toml:59` (`argon2`), `:64-68` (`rustls`,
  `tokio-rustls`, `rcgen`); plus `src/lib.rs:5-9` still advertising a `net`
  module (5.8's addition).
- **Issue:** the manifest comment says the legacy `db/net/*` TLS module is "kept
  compiling" — but `src/net` no longer exists and no symbol from these four
  crates is referenced under `src/`. An entire TLS stack, a certificate
  generator, and a password-hashing crate compile into the facade for nothing.
- **Failure scenario:** pure supply-chain/attack-surface cost (four heavyweight
  crypto crates linked into every consumer), plus a manifest that misstates where
  the crypto boundary lives; the crate doc misstates the public surface.
- **Fix:** delete the four dependencies, the stale comment block, and the `net`
  doc line; re-add with features if a net module ever returns.

### 3.6 [→ 5.3] `ShamirDb::execute` (System-actor, ACL-bypassing) is public and undiscoverable-hidden *(dedup: primary write-up at 5.3)*
Same root-cause defect as api-wire #3; the #606-mitigation contrast
(`db_management.rs:14-33`, `table_management.rs:142-168`) is folded there.

### 3.7 [LOW] `set_net_allowlist` mutates only one clone; other `ShamirDb` clones keep the old allowlist
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:599-605` (field
  `core.rs:79`).
- **Issue:** `ShamirDb` is a cheap-clone Arc-fielded type, but
  `set_net_allowlist(&mut self)` *replaces* the `Arc`, so only the mutated handle
  sees the new allowlist; the doc's "must be called before any function
  invocation" is the only guard, and the codebase itself clones eagerly.
- **Failure scenario:** an operator tightens the egress allowlist at runtime
  through one handle; concurrently-cloned handles keep serving functions with the
  old, possibly broader allowlist.
- **Fix:** `Arc<ArcSwap<Vec<String>>>` so every clone observes the same
  allowlist (lock-free pillar), or document + assert single-clone ownership.

### 3.8 [→ 5.10] `wasm_hash` uses non-cryptographic FxHash and is never verified *(dedup: primary write-up at 5.10)*
Same root-cause defect as api-wire #10 (which adds the dead `version: 1` field);
folded there.

### 3.9 [NIT] `SECURITY DEFINER` grants the guest the owner-actor raw DB gateway, including admin ops
- **File:line:** `src/shamir_db/shamir_db/db_gateway.rs:285-294` (raw-byte
  `execute` passthrough) + `access_control.rs:990-1017`
  (`effective_fn_actor` escalation).
- **Issue:** the definer-owner actor applies to *all* `BatchOp`s the gateway
  accepts — including admin ops (chmod/chown/user-lifecycle), not just DML.
  Requires an operator to create a function as System and chmod it open, but that
  combination silently becomes a full-admin oracle for any caller who can Execute
  it; `effective_fn_actor`'s doc doesn't cover the admin-op breadth.
- **Fix:** document/warn at create time that `SECURITY DEFINER` + open visibility
  exposes owner-privileged admin ops; consider a gateway flag restricting
  definer-context guests to data ops unless owner == caller.

*Test-coverage note:* strong where it exists (gateway ACL-threading, #607
function-path gate, enforcement e2e, SSRF pinning tests). The three gaps mirror
3.1-3.3: no CRLF-injection test, no validator-path WasmCompiler test, no
interner-delta ACL test.

---

## 4. performance-hotpath

*Source: `performance-hotpath.md` (9 findings). Lens verdict: the per-request hot
paths violate pillar 3 through one root cause — every `SystemStore` "point lookup"
is a filtered full catalogue scan, and the ACL gate runs several per op;
`benches/authorize_gate.rs` runs a one-record-per-catalogue database so none of
the O(catalogue-size) scalings are measured.*

### 4.1 [HIGH] ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request *(primary systemic finding)*
- **File:line:** `src/shamir_db/system_store.rs:808` (`load_database`), `:828`
  (`load_repository`), `:860` (`load_table_record`), `:687` (`load_group`),
  `:613` (`load_function`), `:484` (`load_setting`), `:1036` (`load_validator`),
  `:1131` (`load_function_folder`); consumed by
  `src/shamir_db/shamir_db/access_control.rs:41-239` (`resource_meta`) and
  `:849-908` (`authorize_access`).
- **Issue:** every "load one record by key" method builds
  `ReadQuery::new(...).filter(Eq{...})`; system-store tables have no indexes
  (`system_store.rs:97-110`) and the engine read path falls through to
  `read_streaming` — decoding **every** record per scan. `authorize_access` calls
  `resource_meta` per ancestor (up to 5 scans, two settings-table scans) plus the
  target, and `resolve_in_group` adds a `load_group` scan per group-bearing meta;
  nothing is cached.
- **Failure scenario:** for any non-System/Admin actor, per-op authorization cost
  grows linearly with catalogue size: a 10k-row table catalogue ≈ 5 × 10k record
  decodes per data-op authorization; every batch multiplies by op count. The
  existing bench cannot see it (1-db/1-repo/1-table catalogue).
- **Fix:** (a) true key-based point lookup (the composite SetOp key is already the
  storage key), or (b) a lock-free `ResourceMeta` cache invalidated at every
  mutation site (same cache as 2.4 — one change collapses both). Either way keep
  the fail-closed semantics documented at `access_control.rs:31-40`.

### 4.2 [→ 2.1] `execute_as` re-authorizes every op in a batch without dedupe *(dedup: primary write-up at 2.1)*
The performance framing of the same defect (batched workloads quadratic in
practice: O(ops × ancestors × catalogue) instead of O(distinct targets × …));
fix identical.

### 4.3 [MEDIUM] Function invocation scans the function catalogue twice plus two settings scans per call
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:711-720` (also
  `:623-633`, `:662-671`, `:765-774`); second scan in
  `access_control.rs:990-996` (`effective_fn_actor` → `load_function`).
- **Issue:** one `invoke_function*_as` by a User actor loads the function record
  via `resource_meta`, then `effective_fn_actor` re-runs `load_function(fn_name)`
  — the same record scanned and decoded twice — to decide Invoker/Definer;
  `ShamirFunctionInvoker::invoke_call` routes every `Call` op through this.
- **Failure scenario:** function-heavy workloads degrade linearly with catalogue
  size; the duplication is pure waste even at small scale.
- **Fix:** thread the already-loaded record into `effective_fn_actor` (or cache
  owner/security/setuid in the existing in-memory `function_meta` DashMap,
  `core.rs:84`).

### 4.4 [MEDIUM] False "O(1) point lookup" comments encode a scan-based cost model (and hide an O(N²) introspection path)
- **File:line:** `src/shamir_db/execute/admin_access.rs:21-28`;
  `function_management.rs:377-379`; `validator_management.rs:89-93`.
- **Issue:** all three claims are wrong under the current read path (4.1), so
  `list_functions_with_kind` (`function_management.rs:374-389`) performs N full
  catalogue scans for N functions — O(N²) per `LIST FUNCTIONS` — and
  `list_validators_with_kind` (`validator_management.rs:412-425`) is the same
  shape; `group_id_exists` is likewise a scan despite its comment.
- **Failure scenario:** a server with a few hundred functions turns
  `LIST FUNCTIONS`/`LIST VALIDATORS` into tens of thousands of record decodes;
  future code keeps assuming keyed lookups that don't exist.
- **Fix:** fix the comments and make them true by implementing the keyed lookup
  once (4.1's fix also de-quadratizes both list helpers).

### 4.5 [MEDIUM] `InternerTouch` computes the epoch via a full interner traversal per touch
- **File:line:** `src/shamir_db/execute/admin_interner.rs:170-175`.
- **Issue:** after touching the requested names, the handler calls
  `interner.all_entries().max()` — O(size of the interner dictionary), which
  grows monotonically with every distinct field name ever interned — although
  only the high-water id is needed. The full-dump branch (`:80-82`) legitimately
  needs the entries.
- **Failure scenario:** long-lived servers pay an ever-growing per-call cost on a
  path whose useful work is O(names-touched).
- **Fix:** an `AtomicU64` high-water id updated at mint time, or compute the
  epoch from the touch results (`touch_ind` mints gap-free ids).

### 4.6 [LOW] Boot path pairs repos with their tables via an O(repos × tables) nested scan
- **File:line:** `src/shamir_db/shamir_db/core.rs:210-242`.
- **Issue:** for each repo, the entire table-record list is re-filtered; hidden
  O(N·M) at every restart. Startup-only.
- **Failure scenario:** thousands of repos × catalogue rows pay a quadratic scan
  before any request is served.
- **Fix:** build a `TFxMap<(db, repo), Vec<TableConfig>>` in one pass, look up
  per repo.

### 4.7 [LOW] DDL FK guards re-scan the table catalogue once per sibling table — O(tables²) per rename/drop
- **File:line:** `src/shamir_db/shamir_db/table_management.rs:258-288`
  (rename reverse-FK guard); `src/shamir_db/execute/admin_table_index.rs:164-199`
  (drop FK guard).
- **Issue:** both guards loop `db.list_tables(repo)` and call
  `load_table_record` per sibling — each a full catalogue scan — so the guard is
  O(tables²) record decodes. Tolerable at DDL frequency, hence low.
- **Fix:** one `load_tables()` pass reused across the guard, or the keyed lookup
  from 4.1.

### 4.10 [MEDIUM] *(added during synthesis)* FK guards silently skip decode-corrupt parent rows — the RESTRICT/refuse guarantee silently evaporates
- **File:line:** `src/shamir_db/shamir_db/table_management.rs:263-270` and
  `src/shamir_db/execute/admin_table_index.rs:170-178` (spot-check confirmed both:
  `match load_table_record(...).await { Ok(Some(r)) => r, _ => continue }`).
- **Issue:** none of the 7 lens files flags the guard's error handling (4.7 flags
  only the cost). A `load_table_record` that fails (storage/decode fault) — or a
  sibling row whose schema field fails to parse — is silently `continue`d, so the
  "still referenced by a foreign key" refusal never fires for that sibling. The
  workspace SUMMARY's P1.a already carried this row ("FK scans silently skip
  decode-corrupt parent rows (m)") without a corresponding finding in the seven
  files; this entry closes that gap.
- **Failure scenario:** a transient read fault (or one corrupt sibling row) during
  `RENAME TABLE`/`DROP TABLE` → the guard passes without seeing the referencing
  child → the rename dangles the child's persisted `ref_table` name / the drop
  orphans its FK, with no error — silent referential breakage.
- **Fix:** fail closed: propagate the load error as `Err` (only `Ok(None)` may
  `continue`, and log it), matching the crate's fail-closed ACL convention; ride
  the 4.7 single-pass rewrite.

### 4.8 [LOW] Per-invocation gateway construction allocates and intersects allowlists via `Vec::contains`
- **File:line:** `src/shamir_db/shamir_db/core.rs:832-852` (`build_net_gateway`,
  called from `:786-796`, `function_management.rs:733`/`:787`).
- **Issue:** every invocation builds a fresh `CurlNetGateway`, cloning/filtering
  the DB-wide allowlist with a linear `grants.contains(host)` — O(grants ×
  allowlist) plus allocations per call. Constants are small today.
- **Fix:** compute the effective per-function allowlist once at create/boot-load
  (it changes only on DDL/`set_net_allowlist`) and store the intersection in
  `function_meta`; `build_net_gateway` clones one precomputed `Arc<Vec<String>>`.

### 4.9 [NIT] Intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounded by unique-name volume
- **File:line:** `src/shamir_db/shamir_db/core.rs:53-103` (`admin_user_locks`,
  `group_member_locks`, `repo_create_locks`).
- **Issue:** entries "leak by design" (documented inline); all gated by rare
  admin/DDL ops. `admin_user_locks` has accreted a second duty (schema-DDL keys,
  `admin_schema.rs:73-93`) while `GrantRole`/`RevokeRole` no longer take it
  (`admin_users_roles.rs:137-141`) — worth a periodic re-audit that every key
  family is still DDL-only.
- **Fix:** none now; if a family migrates to a per-request path, replace with
  weak-value entries or an LRU under the same documented-contention discipline.

---

## 5. api-wire-protocol

*Source: `api-wire-protocol.md` (12 findings). Lens verdict: the real wire surface
is mature (typed DTOs, e2e serde/error-code suites built through the builder,
documented stable-string contracts, F-43's loud rejections); findings are
contract-shape issues at the edges.*

### 5.1 [HIGH] `replace=true` on a WASM validator destroys persisted binding bookkeeping and can silently re-key its identity *(primary — correctness 1.9 flags the same defect)*
- **File:line:** `src/shamir_db/shamir_db/validator_management.rs:248-252`
  (spot-check confirmed: `id_for_name(name).unwrap_or_default()`),
  `:282-285` (spot-check confirmed: `bound_in` unconditionally reset to `[]`),
  `:302-313` (remove+register instead of `replace_artifact`); registry removal
  wipes live `bound_in` (`crates/shamir-engine/src/validator/registry.rs:143`;
  `replace_artifact` at :93 exists for exactly this and is unused).
- **Issue:** with `replace=true` the catalogue row is rebuilt with
  `bound_in: []`, clobbering the persisted binding list (unlike
  `rename_validator_as`, which preserves the record), and the registry path wipes
  the live `bound_in` too. When the registry already lost the name (boot skipped
  a row after a compile failure, `core.rs:445-451`), `unwrap_or_default()` mints
  `RecordId::default()` — the native path (:89-107) falls back to the catalogue
  `_id` to prevent exactly this.
- **Failure scenario:** operator replaces a table-bound validator → `bound_in`
  emptied in registry and catalogue → `drop_validator`'s `is_bound` guard now
  passes → the validator is dropped while tables still carry
  `ValidatorBinding { validator_id }` → every write to those tables fails closed
  ("Missing") with no live validator to rebind. In the `unwrap_or_default()`
  variant the replacement is registered under a different id, so surviving table
  bindings point at a validator that no longer exists under the old id.
- **Fix:** use `ValidatorRegistry::replace_artifact` for the same id; carry the
  old record's `bound_in`/`_id` into the new row (as the native path and
  `rename_validator_as` do); regression test "replace a bound validator, then
  attempt drop".

### 5.2 [HIGH] Dead, exported `api::{Command, Request, Response}` wire shim that no server speaks
- **File:line:** `src/api/types.rs:7-41`; `src/api/mod.rs:3`; `src/lib.rs:26`.
- **Issue:** the crate exports a plausible-looking client/server envelope
  (`Request { request_id, command }`, `Response { …, result: Result<_, String> }`,
  `Command::{Put,Get,Del,Execute}`) with zero consumers outside its own test. It
  is not the real protocol (`BatchRequest`/`BatchOp`/`DbRequest`), carries no
  version field, flattens errors to `String`, and uses externally-tagged msgpack
  shapes that match nothing on the wire.
- **Failure scenario:** an SDK/FFI author adopts `shamir_db::api::Request` and
  ships a client no `shamir-server` build will ever answer — or a contributor
  "completes" this protocol in parallel to the real one, splitting the wire
  format.
- **Fix:** delete `src/api/` (the round-trip test adds nothing over
  `tests/ddl_wire_e2e/serde_roundtrip.rs`), or `#[doc(hidden)]` + `#[deprecated]`
  with a pointer to `shamir-query-builder` before 0.1.0.

### 5.3 [MEDIUM] Convenience `execute` / `tx_begin` / `tx_execute` / `tx_commit` default to `Actor::System` (admin bypass) *(primary — security 3.6 flags the same defect)*
- **File:line:** `src/shamir_db/execute/db_execute.rs:16-22`;
  `src/shamir_db/execute/db_tx.rs:31-45, 102-110, 204-212`; contrast the #606
  treatment at `db_management.rs:10-33, 325-346`.
- **Issue:** the `_as` variants take the authenticated actor; the bare public
  variants stamp `Actor::System`, which bypasses every ACL check — and remain
  first-class documented public methods whose doc says only "for backward
  compatibility", while far less attractive methods got `#[doc(hidden)]` + SAFETY
  comments under #606.
- **Failure scenario:** an embedder picks the obvious `db.execute("prod", &batch)`
  and silently gets superuser semantics for every op; invisible because
  everything succeeds. (`facade_gateway_acl_tests.rs:1-3` itself documents that
  `execute()` "is `execute_as(Actor::System)`".)
- **Fix:** apply the #606 treatment to the System-actor wrappers, or deprecate in
  favour of `*_as` now that internal callers all use `execute_as`.

### 5.4 [MEDIUM] Builder-only query-construction rule bypassed across the facade (~31 hand-assembled wire-op sites, no exception comments; builder is dev-dep only)
- **File:line:** `src/shamir_db/system_store.rs` (20 struct-literal sites: 199,
  212, 270, 293, 400, 420, 473, 551, 566, 660, 789, 902, 926, 964, 990, 1005,
  1058, 1085, 1100, 1157); `src/shamir_db/execute/admin_replication.rs` (8 sites:
  87, 125, 168, 206, 278, 316, 393, 596); `src/shamir_db/shamir_db/db_gateway.rs`
  (107-142, 168-197, 230-265); `Cargo.toml:102` (builder under
  `[dev-dependencies]` only).
- **Issue:** CLAUDE.md's "builder only" rule mandates a one-line "why" wherever
  the builder doesn't apply; the facade hand-assembles
  `SetOp`/`DeleteOp`/`Filter::Eq`/`ReadQuery`/`InsertOp`/`BatchRequest` at ~31
  production sites with no comments and no builder dependency. Credit: fully
  typed (no raw `serde_json`/`json!` anywhere in `src/`), and struct literals
  fail to compile when a field is added — the cost is convention drift and
  duplicated 15-field literals, not corruption.
- **Failure scenario:** `BatchRequest`/`ReadQuery` grow semantics beyond their
  fields (defaults change, new invariants) and hand-built sites silently diverge
  from builder-produced requests; the unrecorded rationale leaves the next
  reviewer unable to distinguish oversight from decision.
- **Fix:** either promote `shamir-query-builder` and route `db_gateway.rs` (the
  clear client-shaped case) through the builders, or add the mandated one-line
  exception comment per file explaining the facade sits below the builder.

### 5.5 [MEDIUM] Wire error-`code` contract populated unevenly across handler families; `TransactionInfo::aborted` reason mixes stable codes with free text
- **File:line:** coded examples `admin_db_repo.rs:62-65, 117-123, 177-183` + all
  `access_denied` sites; uncoded ~68 `code: None` sites — e.g.
  `db_execute.rs:44-48`, `db_tx.rs:70-81, 169-173, 231-242`, `helpers.rs:84-113`,
  and every non-access error in `admin_replication.rs`, `admin_access.rs`,
  `admin_function.rs`, `admin_validator.rs`, `admin_migration.rs`,
  `admin_buffer.rs`, `admin_interner.rs`; reason mixing `db_tx.rs:252-272`.
- **Issue:** `BatchError.code` is a real client-facing contract (dedicated
  `tests/ddl_wire_e2e/error_codes.rs` suite; coded retry logic like
  `version_conflict`), but whether a failure carries a code depends on the
  handler family; `tx_commit_as` maps four `CommitError` variants to stable codes
  but `Storage`/`Expired` to prose in the same field.
- **Failure scenario:** a client matches `{ "exists", "access_denied" }` per the
  DDL contract, then gets `None` for `Repository not found` from `tx_begin` and
  every replication DDL error, and can't distinguish retryable from permanent
  without parsing prose; commit-reason switches misroute
  `storage: ...`/`tx expired: ...` to the default arm.
- **Fix:** a small closed code vocabulary (not_found / validation / exists /
  unsupported_field / storage / tx_expired alongside access_denied) defined once,
  seeded into the shared `err` closures in `helpers.rs`; make
  `TransactionInfo::aborted` take only stable codes.

### 5.6 [MEDIUM] `tx_begin` accepts any isolation string and silently falls back to Snapshot *(primary — correctness 1.12(a) flags the same defect as a nit)*
- **File:line:** `src/shamir_db/execute/db_tx.rs:82-85` (spot-check confirmed:
  `"serializable" => Serializable, _ => Snapshot`).
- **Issue:** no validation; the typed `IsolationLevel` exists but isn't exposed
  at the facade boundary; any typo or future level ("repeatable_read") is
  silently downgraded.
- **Failure scenario:** a client requests `"serializable"` with a typo (or a
  newer SDK asks for an unknown level) and receives a Snapshot transaction —
  weaker guarantees — with an unqualified success; surfaces only as a production
  data anomaly.
- **Fix:** typed validation error (`unsupported_isolation_level`, naming accepted
  values), or accept `IsolationLevel` directly and parse at the transport layer.

### 5.7 [MEDIUM] `to_qv` converts serialization failure into `QueryValue::Null` inside Ok responses *(source file tags this medium-low; carried as medium)*
- **File:line:** `src/shamir_db/execute/helpers.rs:73-78`; consumed at
  `admin_retention.rs:206` (ChangesSince events), `admin_buffer.rs`,
  `admin_access.rs`.
- **Issue:** the msgpack round-trip helper chains `.ok().and_then(..).ok()` into
  `.unwrap_or(QueryValue::Null)`, so a struct that fails to encode/decode is
  silently replaced by `Null` in an otherwise successful admin response —
  contradicting the project's don't-swallow rules on the response path.
- **Failure scenario:** `ChangesSince` returns `events: [Null, {...}, Null]`
  after a `ChangelogEvent` field stops round-tripping; a changefeed consumer
  treats the Nulls as deletions/unknown events and corrupts its projection, with
  no error anywhere.
- **Fix:** make `to_qv` return `Result<QueryValue, BatchError>` (or log `error!`
  + fail the op); at minimum log loudly at the collapse point as
  `parse_one_rule_default` does.

### 5.8 [→ 3.5] Dead TLS/network dependencies and stale `net` doc *(dedup: primary write-up at 3.5)*
Same root-cause defect as security #5; the `src/lib.rs:5-9` false `net` doc line
is folded in there.

### 5.9 [→ 6.10] Malformed client input mapped to `DbError::Internal` in `get_ddl_op_status` *(dedup: primary write-up at 6.10)*
Same root-cause defect as error-handling #10's `core.rs:764-777` site; the
`Validation`/`NotFound` fix is folded in there.

### 5.10 [LOW] Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade *(primary — security 3.8 flags the FxHash half)*
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:186-189, 205,
  216-218, 230-233`; `validator_management.rs:236-238, 267-270`.
- **Issue:** every function/validator row persists `wasm_hash` (FxHash over the
  bytes) and `version: 1`, but nothing in the workspace reads either, and
  `replace=true` keeps `version: 1` so the field can never mean what it says.
  FxHash is non-cryptographic and not stable across `rustc-hash` major releases —
  fine for in-process keyed structures, wrong as a persisted
  content-integrity/version identity.
- **Failure scenario:** a later task "verifies" a function against its persisted
  `wasm_hash` (the name invites it) and either accepts a spoofed module or
  rejects every row after a dependency upgrade.
- **Fix:** delete both fields until a consumer exists, or switch to a stable
  digest (SHA-256), actually increment `version` on replace, and document the
  on-disk contract next to `ArtifactKind::as_str`.

### 5.11 [→ 6.1] `create_db_as` reports success even when the catalogue write fails *(dedup: primary write-up at 6.1)*
The `db_management.rs:58-64` instance of error-handling #1's swallowed-write
family (incoherent with sibling `add_repo_as`'s `DbResult<()>`; wire answers
`{"created": true}` for a database that vanishes on restart).

### 5.12 [NIT] Bundle: wire-handler hygiene
- (a) `helpers.rs:59-63` SystemTime `.unwrap()` — dedup: folded into **1.10**.
- (b) `db_gateway.rs:87-89` `batch_err_to_string` formats with `{e:?}` (unstable
  internal enum dump, drops `code`) — dedup: folded into **6.10**.
- (c) `src/main.rs` — a Hello-World codec demo binary (with
  `#![allow(deprecated)]`) shipping in what is otherwise a library facade.
- (d) `ports.rs:7` — doc typo: "the narrow injected surface lets" → "that lets".

*Coverage note:* test layout conforms (manifest-only `tests/mod.rs` trees, no
inline `#[cfg(test)]` in `src/`, builder-built round-trip harness); wire-surface
coverage broad (30+ integration files). Themed gap: no test exercises
`replace=true` on a table-bound validator (5.1).

---

## 6. error-handling-lifecycle

*Source: `error-handling-lifecycle.md` (12 findings). Lens verdict: broadly
faithful to the error rules (`Result` + `DbError` throughout, no `anyhow`, no raw
panics outside invariants, exemplary fail-closed ACL handling, compensating-write
rollback in schema activation); the systemic weakness is the catalogue-lifecycle
layer treating storage-write failures as `warn!`-and-continue.*

### 6.1 [HIGH] Catalogue-persistence failures are swallowed (`warn!` + continue) across the DB/repo/table lifecycle, so multi-step mutations can return `Ok(())` half-migrated *(primary — api-wire 5.11 flags the `create_db_as` instance)*
- **File:line:** `src/shamir_db/shamir_db/db_management.rs:184-201`
  (`rename_db_as` — both writes `if let Err(e) => log::warn!`), also `:58-64`
  (`create_db_as`), `:77-85` (`remove_db`), `:232-251` + `:298-319` (rename_db
  re-key loops), `:396-426` (`add_repo_as`), `:435-446` (`remove_repo`),
  `:534-553` + `:571-598` (`rename_repo_as`); `table_management.rs:56-74`
  (`add_table_as`), `:95-107` (`drop_table`), `:323-350` (`rename_table_as`).
- **Issue:** every one writes the live registration first, then persists
  catalogue changes with a write-new-before-remove-old design whose comments
  promise crash-safety — but an *error* in either write is only logged, the
  function never propagates, and remove-old proceeds even when write-new failed.
  `flush_all` (`db_management.rs:632-656`) demonstrates the crate's own better
  pattern (log each, return the first error).
- **Failure scenario:** `rename_db_as("a","b")`: in-memory key moves to `b`;
  `save_database("b")` fails on transient fjall I/O (warn); `remove_database("a")`
  succeeds → catalogue holds **no** record while the caller got `Ok(())`. On
  restart, `core.rs:194-201` registers no `DbInstance` and `core.rs:219` silently
  skips every repo row whose db is missing → all repos/tables of the renamed DB
  are unavailable after reboot with zero signal beyond earlier warns. Mirror for
  `rename_table_as`: table gone from the catalogue, physical stores already
  renamed, data orphaned.
- **Fix:** propagate the first catalogue error after attempting the compensating
  step (mirror `flush_all`); minimally never remove-old when write-new failed;
  add the `else { warn! }` for repo rows whose db is missing at boot (1.4).

### 6.2 [HIGH] `rename_function_as` / `rename_validator_as` / `rename_function_folder_as` destroy the durable record *before* writing the new one (remove-before-write) *(primary — correctness 1.6 flags the function/validator pair)*
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:335-347`
  (`remove_function(from)` at :337 before `save_function(to)` at :344);
  `validator_management.rs:373-385` (remove :375, save :382);
  `function_management.rs:565-574` (folder rename removes **all** old keys, then
  saves all new keys).
- **Issue:** the only rename paths in the crate that remove the old durable row
  before the new one is durably written — the inverse of the convention
  `rename_db_as`/`rename_repo_as`/`rename_table_as` document and follow. The
  folder rename's doc explicitly claims "no partial state is left if a write
  fails mid-way" — the implementation violates that claim. Additionally the live
  registry is renamed before the catalogue, so a catalogue error leaves
  registry=`to`, catalogue=`from` (resurrects under the old name next boot).
- **Failure scenario:** `rename_function_as("f","g")`: registry renamed;
  `remove_function("f")` succeeds; `save_function("g")` fails (fsync/I/O) → `Err`
  returned but the durable record exists under *neither* name; after restart the
  function is silently gone (boot loads only from the catalogue). Same for
  validators and for an entire renamed folder subtree.
- **Fix:** reorder to persist-new-first then remove-old (both `?`-propagated),
  matching the db/repo/table renames; fix the folder doc or implement it;
  optionally roll back the live-registry rename on catalogue failure. Add a
  fault-injecting Store test (the seam from 6.3) asserting the catalogue retains
  `from` when the `to` write fails.

### 6.3 [MEDIUM] No error-path test injects a system-store failure into the lifecycle paths above
- **File:line:** `src/shamir_db/shamir_db/tests/schema_rollback_tests.rs` (covers
  `compile_table_schema` rollback); `tests/p1065_ddl_status_contract_tests.rs:264`
  (fault-injected `DbError::Storage` — index DDL op-status contract only); ~100
  `is_err()` assertions cover validation/not-found/denied only.
- **Issue:** none of the swallowed catalogue-write paths (6.1, 6.2, 5.11) has a
  test, because nothing can currently fail `SystemStore::save_*`/`remove_*` in a
  test. The engine side has the seam (`shamir-engine` test-util fault-injecting
  Store); `SystemStore` has no equivalent.
- **Failure scenario:** a refactor reorders remove-old before write-new in
  `rename_db_as` (as `rename_function_as` already did) and every test stays
  green.
- **Fix:** test-only fault-injecting seam for `SystemStore` writes, then test:
  (a) rename_db/repo/table with write-new failing → `Err`, old rows intact;
  (b) rename_function/validator with save failing → old record survives;
  (c) boot with an orphan repo row → diagnostic surfaced.

### 6.4 [→ 1.2] `SystemStore` group-member methods silently fabricate a phantom group *(dedup: primary write-up at 1.2)*
Same root-cause defect as correctness #2; the store-vs-dispatcher layering
argument (self-defending methods per `create_group_as`) is folded there.

### 6.5 [MEDIUM] Cascade-drop paths discard per-table drop errors with `let _ =` — not even a log line
- **File:line:** `src/shamir_db/execute/admin_db_repo.rs:130-137`
  (`handle_drop_db` cascade — the same loop as 1.1), `:377-384`
  (`handle_drop_repo` cascade); `admin_table_index.rs:252, 261-264, 274, 279-282`
  (index-cascade `let _ =`).
- **Issue:** `drop_table_cleaning_validators` returns `DbResult<bool>`; cascade
  call sites throw the whole `Result` away. A failing table drop during
  `DROP DATABASE … CASCADE` is invisible, the op reports success, and stale
  `(db, repo, table)` rows survive — which boot re-creates as tables under a
  dropped db/repo.
- **Failure scenario:** system-store blip during cascade: `DROP DATABASE`
  answers `{"dropped": true}`; on restart the dropped database partially
  resurrects from surviving table-catalogue rows (or orphaned rows linger with no
  diagnostic).
- **Fix:** at minimum `warn!` per failed step; better, collect failures and
  include `"partial": [...]` in the admin response (same fix passes the right db
  arg — 1.1).

### 6.6 [MEDIUM] Wire error-code classification by substring on the stringified `PortError`
- **File:line:** `src/shamir_db/execute/admin_users_roles.rs:146-154` and
  `:185-193` (`msg.contains("user not found")` decides `not_found` vs `query`);
  `PortError = Box<dyn Error>` at `ports.rs:32` (documented boundary).
- **Issue:** classifying by English substring is fragile — a wording change in
  the implementing directory silently downgrades every `not_found` to `query`.
  Also inconsistent: grant/revoke classify, create/drop user
  (`:56-63`, `:103-106`) map everything to `query` unconditionally.
- **Failure scenario:** the directory's message changes to `"no such user …"` →
  all GrantRole/RevokeRole on unknown users return code `query`; clients that
  surface "user not found" specially regress silently.
- **Fix:** a stable sentinel convention at the port boundary (documented prefix
  checked via `starts_with`, or a `fn is_not_found(&self, e: &PortError) -> bool`
  marker on `UserAdminPort`); apply uniformly in all four handlers.

### 6.7 [→ 1.4] Boot path: repo re-attach failure is `warn!` + `continue`; repo rows for unknown databases are skipped without any log *(dedup: primary write-up at 1.4)*
Same root-cause defect as correctness #4; the attach-vs-recovery durability
asymmetry (`core.rs:243-252` vs `:263`) is folded in there.

### 6.8 [LOW] Ambient interner delta attach: errors silently skipped, contradicting the module's own doc
- **File:line:** `src/shamir_db/execute/ambient_interner.rs:20-21` (doc: errors
  "surfaced as a soft `BatchError`") vs `:38-45` (`Err(_) => continue`, no log);
  caller `db_execute.rs:97-102` logs at `debug!` only. (Same site as the 3.2 ACL
  gap — distinct defect: error swallowing vs missing authorization.)
- **Issue:** doc/implementation drift plus a silent swallow; the client keeps its
  stale epoch and can mis-resolve newly interned field names until a full
  `InternerDump`.
- **Failure scenario:** transient store error during `repo_interner()` → response
  carries no delta → client caches lag → field names sent after the gap miss on
  the server.
- **Fix:** `warn!` in the `Err` arms (or actually return the promised soft
  `BatchError`); align doc and code in whichever direction.

### 6.9 [→ 1.10] `admin_result_with_op_id` panics on wall-clock regression while every other call site defaults *(dedup: primary write-up at 1.10)*
The `helpers.rs:59-62` SystemTime `.unwrap()`; folded into correctness #10's
panic-path pair.

### 6.10 [LOW] Stringly-typed error collapsing loses error identity on internal mappings *(primary — api-wire 5.9 flags the `get_ddl_op_status` site, 5.12(b) the gateway site)*
- **File:line:** `src/shamir_db/shamir_db/system_store.rs:156` and `:178`
  (`BatchError` → `DbError::Internal(e.to_string())`);
  `core.rs:764-777` (`get_ddl_op_status` wraps an existing `DbError` into
  `Internal(format!(…))`, so callers can no longer distinguish
  `NotFound`/`Validation` for caller-supplied input);
  `db_gateway.rs:87-89` (`format!("{e:?}")` — Debug, not Display — across the
  WASM gateway boundary).
- **Issue:** `?`-discipline is respected, but these mappings erase variant
  identity, and one site renders errors with unstable Debug formatting. Partly
  forced by engine-side `String`-error traits (outside this crate's control).
- **Failure scenario:** a client that mangles an op id gets an "Internal error"
  the operator treats as a corruption event and pages on; gateway error text
  changes shape between builds and loses `code`.
- **Fix:** forward the original `DbError` in `get_ddl_op_status` (`Validation`
  for the parse failure, `NotFound` for table resolution; reserve `Internal` for
  genuine invariants); add a `DbError::Batch` variant for the implicit-tx
  mappings; use `Display` + `code` suffix in `batch_err_to_string`.

### 6.11 [LOW] `resolve_in_group` silently converts group-lookup errors into `false` without a log
- **File:line:** `src/shamir_db/shamir_db/access_control.rs:913-918`
  (`user_in_group(...).await.unwrap_or(false)`; doc at `:910-912`).
- **Issue:** fail-closed direction is correct and deliberate, but the conversion
  is invisible — during a transient catalogue outage every group-based grant
  silently evaporates, with no log; sibling fail-closed sites in
  `resource_meta`/`authorize_access` all `warn!` before denying.
- **Fix:** `warn!` (or `debug!` with correlation id) when `user_in_group` errors,
  matching the documented pattern.

### 6.12 [NIT] Bundle: panic-proofing / swallowed header-file read
- `admin_table_index.rs:459, 466, 514` — `itype.unwrap()` inside error-message
  construction; provably safe today, `unwrap_or("?")` stays panic-proof under
  future guard edits.
- `curl_gateway.rs:226-244` — `parse_response_headers` swallows the header-file
  read error (`if let Ok(bytes)`); I/O failure indistinguishable from "no
  headers"; a `warn!` would match the module's cleanup diligence.

---

## 7. style-claude-md

*Source: `style-claude-md.md` (9 findings). Lens verdict: structural conformance
strong — re-export-only `mod.rs` files, four conforming `tests/` trees, exemplary
rationale-comment culture; main deviation is imports-at-top, plus one papered-over
module inception and one-line hygiene nits.*

### 7.1 [MEDIUM] Function-local `use` statements in production code violate the imports-at-top rule
- **File:line:** `src/shamir_db/shamir_db/core.rs:759-761`;
  `src/shamir_db/shamir_db/schema_management.rs:109-110, 277, 302, 375, 418`;
  `src/shamir_db/execute/admin_replication.rs:498` — ten sites, three files.
- **Issue:** none of the three documented exceptions applies and no site carries
  the required justification comment; the `RecordId` import at `core.rs:760`
  **duplicates** the file-header import at `core.rs:13`, and
  `admin_replication.rs:498` locally imports `Actor` while the header (:16)
  already pulls from `crate::access` — drift already happening.
- **Failure scenario:** the rule exists so a file's dependency surface is
  greppable from its header; a future reader auditing `schema_management.rs` sees
  two schema types at the top, not the six actually used.
- **Fix:** hoist all ten imports to the file headers and delete the duplicate
  `RecordId` line; one style-sweep commit per CLAUDE.md.

### 7.2 [MEDIUM] `shamir_db::shamir_db` module inception suppressed with an unannotated allow
- **File:line:** `src/shamir_db/mod.rs:7-8`
  (`#[allow(clippy::module_inception)] pub mod shamir_db;`).
- **Issue:** the crate ships `shamir_db::shamir_db::ShamirDb`; `lib.rs`'s doc
  records that the old `db/` wrapper was lifted precisely to remove a redundant
  level, yet the inner nesting stayed and the lint is suppressed with no comment
  (unlike the workspace's annotated allow convention).
- **Failure scenario:** every new file must be placed at the correct one of two
  identically-named levels (`shamir_db/ports.rs` vs `shamir_db/shamir_db/core.rs`);
  the ambiguity already produced sibling files living at both depths; docs and
  stack traces carry doubly-qualified paths.
- **Fix:** flatten `src/shamir_db/shamir_db/*` into `src/shamir_db/*` (both
  `mod.rs` files are already re-export-only, so the move is mechanical), or add
  the inline justification if the nesting is load-bearing.

### 7.3 [LOW] `SYSTEM_DB_NAME` constant declared inside `mod.rs` (re-exports only)
- **File:line:** `src/shamir_db/shamir_db/mod.rs:15`.
- **Issue:** the only definition in any of the crate's six `mod.rs` files;
  CLAUDE.md: "mod.rs files contain re-exports only."
- **Fix:** move into `core.rs` (consumers reach it via `super::SYSTEM_DB_NAME`
  unchanged).

### 7.4 [LOW] Blanket `#![allow(deprecated)]` with no named reason
- **File:line:** `src/main.rs:1`; `src/api/types.rs:1`;
  `src/api/tests/api_tests.rs:1`.
- **Issue:** three file-wide allows with no comment naming which deprecated API
  requires them or until when; a genuinely new deprecation (e.g. in
  `shamir-types` value/codec APIs) will also be silenced.
- **Fix:** item-scoped `#[allow(deprecated)] /* reason: <API, task> */`, or a
  one-line comment naming the item and removal condition (moot for `api/` and
  `main.rs` if 5.12(c) deletes the demo bin and 5.2 deletes the shim).

### 7.5 [NIT] Stray empty statement and dead comment in `curl_gateway.rs`
- **File:line:** `src/shamir_db/curl_gateway.rs:132-133`.
- **Issue:** the cleanup comment is followed by a bare `;` — leftover of a removed
  statement; the accurate copy lives at :172.
- **Fix:** delete lines 132-133 (keep :172).

### 7.6 [NIT] `tests/mod.rs` manifests use private `mod` instead of the documented `pub mod` form
- **File:line:** `src/api/tests/mod.rs:1`; `src/shamir_db/tests/mod.rs:1-26`;
  `src/shamir_db/shamir_db/tests/mod.rs:1-3`;
  `src/shamir_db/execute/tests/mod.rs:1`.
- **Issue:** content-wise conforming manifest-only files; only the visibility
  spelling differs from the CLAUDE.md snippet, so greps/templates copied from it
  won't match.
- **Fix:** switch to `pub mod` or update the CLAUDE.md snippet; pick one.

### 7.7 [NIT] Inconsistent qualified vs imported spelling of `new_map`/`QueryValue` throughout the facade
- **File:line:** e.g. `system_store.rs:9` vs `148, 229, 320, 452, 487, 586, …`;
  `access_control.rs:2` vs `314, 359, 372`; `db_management.rs:4` vs `48-57`;
  `function_management.rs:12` vs `206-243, 423-436`; `validator_management.rs` vs
  `110-136, 376-380, 578-589`.
- **Issue:** files import these at the top yet also spell them fully-qualified
  inline, sometimes both forms within ten lines — the dominant source of visual
  noise in the crate's largest files; not a rule violation, but the same
  header-vs-body ambiguity the imports rule prevents.
- **Fix:** normalize to the top-level imports within touched files (rides the
  7.1 sweep).

### 7.8 [NIT] `schema_management` alone breaks the sibling export convention
- **File:line:** `src/shamir_db/shamir_db/mod.rs:8`; consumers at
  `execute/admin_schema.rs:42-44`, `admin_table_index.rs:6`,
  `admin_describe.rs:7`.
- **Issue:** every sibling is a private mod re-exported at the parent;
  `schema_management` is `pub(crate) mod` consumed via the deep
  `crate::shamir_db::shamir_db::schema_management::{…}` path in three files — two
  surface conventions coexist.
- **Fix:** add the parent `pub(crate) use schema_management::{…}` re-exports,
  revert to private `mod`, update the three import sites.

### 7.9 [NIT] `ports.rs` carries four public exports in one file (borderline cohesion)
- **File:line:** `src/shamir_db/ports.rs:32, 39, 64, 99`.
- **Issue:** `PortError`, `PrincipalInfo`, `PrincipalResolver`, `UserAdminPort`
  in one file; the module doc explicitly frames them as a single identity seam
  with a shared dependency-direction rationale, so this reads as the sanctioned
  closely-coupled-group case — flagged only because the group is four items.
- **Fix:** no action needed; if the seam grows, split read-side from write-side.

*Positive conformance (for the record):* re-export-only `mod.rs` (sole exception
7.3); zero inline `#[cfg(test)]` blocks (grep-verified) and four conforming
`tests/` trees; ~30 unit-test files + ~35 integration files with `ddl_wire_e2e/`
a proper multi-file harness; `doctest = false` documented; all five benches use
`bench_scale_tool::Harness` with `harness = false`; comment discipline is a
workspace model. (`shamir-funclib` dual dependency/dev-dependency entry is
redundant but commented — not raised.)

---

## Finding counts

| Severity | Lens-tagged findings | Distinct defects after dedup (from the 7 files) |
|---|---|---|
| critical | 0 | 0 |
| high | 10 | 9 — 1.1, 1.2 (=6.4, 2.5), 2.1 (=4.2), 3.1, 4.1, 5.1 (=1.9), 5.2, 6.1 (=5.11), 6.2 (=1.6) |
| medium | 22 | 20 — 1.3, 1.4 (=6.7), 1.5, 2.2, 2.3, 3.2, 3.3, 4.3, 4.4, 4.5, 5.3 (=3.6), 5.4, 5.5, 5.6 (absorbs 1.12a), 5.7, 6.3, 6.5, 6.6, 7.1, 7.2 |
| low | 27 | 19 — 1.7, 1.8, 1.10 (=6.9, absorbs 5.12a), 1.11, 2.4, 2.6, 2.7, 3.4, 3.5 (=5.8), 3.7, 4.6, 4.7, 4.8, 5.10 (=3.8), 6.8, 6.10 (=5.9, absorbs 5.12b), 6.11, 7.3, 7.4 |
| nit | 12 | 11 — 1.12, 2.8, 3.9, 4.9, 5.12, 6.12, 7.5, 7.6, 7.7, 7.8, 7.9 |
| **total** | **71** | **59** |

Dedup convention (per the workspace SUMMARY): a defect flagged by multiple lens
files is listed once under its primary lens with the others cross-referenced.
Twelve lens-tagged findings are absorbed: 4.2→2.1, 6.4→1.2, 2.5→1.2, 5.11→6.1,
1.6→6.2, 1.9→5.1, 3.6→5.3, 6.7→1.4, 5.8→3.5, 3.8→5.10, 6.9→1.10, 5.9→6.10 (the
latter two also absorb bundle items 5.12a/5.12b; 1.12a is absorbed by 5.6 without
changing its bundle's count). Severity of a merged group is the primary's.

**Plus one defect added during this synthesis pass** (none of the 7 files caught
it; it closes the workspace SUMMARY's otherwise-orphan "FK scans silently skip
decode-corrupt parent rows (m)" row): **4.10** (medium). Total on record after
synthesis: **60 distinct defects — 0 critical, 9 high, 21 medium, 19 low,
11 nit** (71 lens-tagged).

---

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Fix the CASCADE wrong-target.** Pass `&op.drop_db` to
   `drop_table_cleaning_validators` (or reject `op.drop_db != self.db_name` with
   a typed error), stop the `let _ =` swallowing (collect/log or fail the op),
   and add the cross-db Red test. Closes **1.1**, and the same edit closes
   **6.5**.
2. **Close the curl-gateway CRLF injection.** Reject CR/LF/C0 controls in every
   guest-supplied string (url, method, header name/value) before
   `escape_curl_value`; tests asserting rejection and a `curl.cfg` free of raw
   newlines in values. Closes **3.1**.
3. **Stop the silent catalogue-loss family.** Propagate the first catalogue-write
   error (mirror `flush_all`) instead of `warn!`-and-continue, never remove-old
   when write-new failed, and reorder function/validator/folder renames to
   write-new-before-remove-old. Closes **6.1**, **6.2** (incl. 1.6, 5.11), and
   the rename half of the boot-amnesia scenario.
4. **Fix the validator replace path.** Use `ValidatorRegistry::replace_artifact`,
   carry `bound_in`/`_id` into the new row, port the catalogue `_id` fallback;
   regression test "replace a bound validator, then attempt drop". Closes
   **5.1** (incl. 1.9).
5. **Stop fabricating phantom groups.** `SystemStore::add/remove_group_member`
   return `NotFound` on absent records (as `set_group_owner` does); re-load under
   the per-group lock in `rename_group_as`/`add_group_member_as`; add the
   remove-path wire guard + Red test. Closes **1.2** (incl. 6.4, 2.5).

**P1 — soon**
6. **Port the ACL inline cache and de-scan the gate.** `FxHashMap<(ResourcePath,
   Action), bool>` dedup in `execute_as`; true keyed point lookups (or the 2.4/4.1
   lock-free `ResourceMeta` cache with fail-closed-on-error preserved); fix the
   false O(1) comments. Closes **2.1/4.2**, **4.1**, **4.4** (and collapses
   2.6/2.8 en route).
7. **Complete the #546 lock coverage.** Per-(db,repo,table) lock (or
   `ddl_admission`) for table create/drop/rename; acquire the existing
   db/repo locks in drop/rename handlers. Closes **2.2**, **2.3**.
8. **Close the two authorization gaps.** Store-Read ACL on the ambient interner
   delta; WasmCompiler-Execute gate on the validator source path (+
   `#[doc(hidden)]` the System-actor convenience wrappers, and delete the dead
   `api::` shim — one-file deletions/annotations closing cheap traps). Closes
   **3.2**, **3.3**, **5.3** (incl. 3.6), **5.2**.
9. **Durability & boot diagnostics.** Add the Durable-DDL flush to
   `save_database`/`remove_database` + replication handlers (fix the false
   comment); `warn!` on orphan repo rows and failed attaches; `continue` before
   `function_meta` on builtin collisions. Closes **1.3**, **1.4** (incl. 6.7),
   **1.5**.
10. **FK guards fail closed + stop the quadratic rescan.** One catalogue pass per
    guard; propagate load errors instead of `_ => continue`. Closes **4.7**,
    **4.10**.
11. **Add the SystemStore fault-injection seam and the Red tests it enables**
    (rename failure injection, orphan boot, durability without external
    `flush_all`). Closes **6.3** and the test-gap halves of P0 items 3-5.
12. **Wire contract tightening.** Closed error-code vocabulary in `helpers.rs`;
    isolation-string validation; `to_qv` returns `Result`. Closes **5.5**,
    **5.6** (incl. 1.12a), **5.7**.
13. **Error-identity cleanups.** Port-boundary `not_found` sentinel; forward
    original `DbError` in `get_ddl_op_status`; `Display` in
    `batch_err_to_string`; drop the two library panic paths. Closes **6.6**,
    **6.10** (incl. 5.9, 5.12b), **1.10** (incl. 6.9).
14. **Drop-semantics correctness.** `existed` includes the catalogue;
    `IF EXISTS` probes it. Closes **1.8**.

**P2 — backlog**
15. `ArcSwap` ACL snapshot with per-mutation invalidation + invalidation test.
    Closes **2.4** (with 6.11's `warn!` on silent group denial).
16. Perf polish: thread the loaded function record into `effective_fn_actor`;
    one-pass boot repo/table pairing; per-function precomputed allowlist;
    `InternerTouch` high-water id. Closes **4.3**, **4.6**, **4.8**, **4.5**.
17. Hardening: egress `max-filesize`; `set_net_allowlist` via `ArcSwap` across
    clones; dead argon2/rustls/tokio-rustls/rcgen removal + `net` doc fix;
    SECURITY DEFINER admin-breadth doc/warn. Closes **3.4**, **3.7**, **3.5**
    (incl. 5.8), **3.9**.
18. API hygiene: builder rule resolution (dependency or exception comments);
    delete/annotate dead `wasm_hash`/`version`; `create_db_as` lock +
    `KeyExists` + error propagation; `replace` FK-action parsing in the RMW read
    path; migration-API atomic reservation when hardened; `to_qv` consumers'
    Null-collapse guard rides 5.7. Closes **5.4**, **5.10**, **1.7**, **1.11**,
    **2.7**.
19. Nit bundles & logging: no-op schema-rule version bump; schema-lock
    namespace; interner-delta `Err` logging; header-file read `warn!`;
    `itype.unwrap_or("?")`; demo bin + doc typo. Closes **1.12** (b, c),
    **6.8**, **6.12**, **5.12** (c, d).
20. Style sweep in its own commit: hoist the ten mid-function imports, flatten
    (or justify) the module inception, `SYSTEM_DB_NAME` out of `mod.rs`,
    item-scoped `allow(deprecated)`, manifest visibility spelling, qualified-name
    normalization, `schema_management` re-export, stray `;`. Closes **7.1**-**7.9**.

</details>
