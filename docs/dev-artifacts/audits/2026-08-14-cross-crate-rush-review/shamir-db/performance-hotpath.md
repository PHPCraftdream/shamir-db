# shamir-db -- Performance & O(x->0)

## Summary
The facade's per-request hot paths (`execute_as`/`tx_execute_as` authorization, function invocation, per-op ACL checks) violate pillar 3 (O(x→0)) through the same root cause: every `SystemStore` "single record" lookup is a filtered **full catalogue scan** (no index on system-store tables, no primary-key shortcut in the read path), and the ACL gate runs several of these scans per op. A batch of N ops against one table re-pays the full ancestor traversal N times in `execute_as` — the inline ACL cache that `tx_execute_as` already has was never ported back. Secondary items: function invocation scans the function catalogue twice per call, several comments assert a false O(1)-point-lookup cost model (hiding O(N²) introspection), and `InternerTouch` computes its epoch via a full interner traversal. Test coverage for ACL semantics is thorough (`shamir_db/tests/access_meta_tests.rs` et al.), but `benches/authorize_gate.rs` exercises only a one-record-per-catalogue database, so none of these O(catalogue-size) scalings are measured.

## Findings

### 1. ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request
File:line: `crates/shamir-db/src/shamir_db/system_store.rs:808` (`load_database`), `:828` (`load_repository`), `:860` (`load_table_record`), `:687` (`load_group`), `:613` (`load_function`), `:484` (`load_setting`), `:1036` (`load_validator`), `:1131` (`load_function_folder`); consumed by `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:41-239` (`resource_meta`) and `:849-908` (`authorize_access`).
Severity: high
Issue: Every "load one record by key" method builds `ReadQuery::new(...).filter(Eq{...})` and runs `TableManager::read`. The system-store tables are created with plain `TableConfig::new(...)` (system_store.rs:97-110) — no indexes — and the engine read path only accelerates filters via index/index2 planners, otherwise falling through to `read_streaming`, which streams and decodes **every** record (msgpack + de-intern) and evaluates the filter per row. There is no primary-key shortcut for `Filter::Eq`, even though the records were written via `SetOp` whose key is exactly those name fields. `authorize_access` calls `resource_meta` once per ancestor (Root, Database, Store, Table → up to 5 scans, two of them settings-table scans) plus the target, and `resolve_in_group` (`access_control.rs:913`) triggers another `load_group` scan per group-bearing meta. No caching exists anywhere on this path (`resource_meta` is re-read from durable storage every call).
Failure scenario: For any non-`System`/non-`Admin` actor (System/Admin bypass at `access_control.rs:839`), per-op authorization cost grows linearly with catalogue size: a deployment with 10k table-catalogue rows pays on the order of 5 × 10k record decodes for every data-op authorization; every batch multiplies by op count. The existing `authorize_gate` bench cannot see this because it runs against a 1-database/1-repo/1-table catalogue.
Suggested fix: (a) add a true key-based point lookup — the composite `(db_name[, repo_name][, table_name|name|group_id|path])` set-op key is already the storage key, so a `get`-by-key read avoids the scan; or (b) cache `ResourceMeta` per `ResourcePath` in a lock-free map (`scc::HashMap`/`ArcSwap`, Fx hasher) invalidated at every mutation site (`set_resource_meta`, rename/drop/create DDL), which also collapses the per-ancestor store I/O to one in-memory read. Either way, keep the fail-closed-on-error semantics documented at `access_control.rs:31-40`.

### 2. `execute_as` re-authorizes every op in a batch without dedupe (the inline ACL cache exists only in `tx_execute_as`)
File:line: `crates/shamir-db/src/shamir_db/execute/db_execute.rs:64-68` vs `crates/shamir-db/src/shamir_db/execute/db_tx.rs:150-167`; per-op entry list from `crates/shamir-query-types/src/batch/query_entry.rs:127-155`.
Severity: high
Issue: `collect_required_access` returns one `(Action, ResourcePath)` per op — not deduped. `tx_execute_as` wraps the loop in a stack-local `FxHashMap<(ResourcePath, Action), bool>` ("ACL inline cache", db_tx.rs:142-159) so repeated ops against the same table cost ~50 ns after the first; `execute_as` — the primary autocommit wire path — runs the raw loop, so a 1000-insert batch pays 1000 full `authorize_access` traversals (each = finding 1's scan set).
Failure scenario: batched (the pillar-3-preferred) workloads by non-admin actors are quadratic in practice: per-batch ACL cost = O(ops × ancestors × catalogue) instead of O(distinct targets × ancestors × catalogue).
Suggested fix: port the identical `FxHashMap<(ResourcePath, Action), bool>` dedupe from `tx_execute_as` into `execute_as` (the correctness argument in db_tx.rs:142-149 applies verbatim — the list is computed once per call from the same request).

### 3. Function invocation scans the function catalogue twice plus two settings scans per call
File:line: `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711-720` (and `:623-633`, `:662-671`, `:765-774`); second scan in `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:990-996` (`effective_fn_actor` → `load_function`).
Severity: medium
Issue: One `invoke_function*_as` call by a User actor does: `authorize_access(Function{name})` → `resource_meta` Function arm → `load_function` full scan + Root and FunctionNamespace `load_setting` scans; then `effective_fn_actor` re-runs `load_function(fn_name)` — the same record, scanned and decoded a second time — to decide Invoker/Definer. `ShamirFunctionInvoker::invoke_call` routes every `Call` batch op through this, so per-Call cost is O(#functions × 2) + O(#settings × 2) record decodes.
Failure scenario: function-heavy workloads degrade linearly with catalogue size; the duplication is pure waste even at small scale (two identical durable reads per call).
Suggested fix: thread the record already loaded by `resource_meta`/authorize into `effective_fn_actor` (return it from the gate or load once in the invoker and pass it down), and/or cache owner/security/setuid in the existing in-memory `function_meta` DashMap (core.rs:84), which is already populated at create/load and updated on rename/drop.

### 4. False "O(1) point lookup" comments encode a scan-based cost model (and hide an O(N²) introspection path)
File:line: `crates/shamir-db/src/shamir_db/execute/admin_access.rs:21-28` ("a direct point lookup via `load_group`, not a scan"); `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:377-379` ("each lookup is O(1) against the name-keyed catalogue table"); `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:89-93` ("the O(1) `load_validator(name)` lookup rather than a full-catalogue scan").
Severity: medium
Issue: All three claims are wrong under the current read path — `load_group`/`load_function`/`load_validator` are `Filter::Eq` full scans (finding 1). Consequently `list_functions_with_kind` (function_management.rs:374-389) performs N full catalogue scans for N registered functions — O(N²) per `LIST FUNCTIONS` call — and `list_validators_with_kind` (validator_management.rs:412-425) is the same shape; `group_id_exists` is likewise a scan despite its comment.
Failure scenario: a server with a few hundred functions/validators turns `LIST FUNCTIONS`/`LIST VALIDATORS` into tens of thousands of record decodes; future code written against these comments will keep assuming keyed lookups that don't exist.
Suggested fix: fix the comments, and make them true by implementing the keyed lookup once (see finding 1's fix) — that single change also de-quadratizes both list helpers.

### 5. `InternerTouch` computes the epoch via a full interner traversal per touch
File:line: `crates/shamir-db/src/shamir_db/execute/admin_interner.rs:170-175`.
Severity: medium
Issue: After touching the requested names, the handler calls `interner.all_entries()` and takes `.max()` of the ids — O(size of the interner dictionary) — although only the high-water id is needed and the entries themselves are not returned. The interner grows with every distinct field name ever interned (unbounded). This is precisely the pattern CLAUDE.md pillar 3 bans for `scc::*::len()` (O(N) cardinality on a code path; the fix doctrine is an atomically-mirrored high-water mark). The full-dump branch in `handle_interner_dump` (admin_interner.rs:80-82) legitimately needs the entries, so it is fine.
Failure scenario: each touch call walks a dictionary whose size grows monotonically with schema diversity; long-lived servers pay an ever-growing per-call cost on a path whose useful work is O(names-touched).
Suggested fix: maintain an `AtomicU64` high-water id updated at mint time (ids are monotonic per the doc) and read it here; alternatively compute the epoch from the touch results themselves (`mappings` max) since `touch_ind` mints gap-free ids.

### 6. Boot path pairs repos with their tables via an O(repos × tables) nested scan
File:line: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:210-242`.
Severity: low
Issue: `init` loads all repo records and all table records, then for **each** repo iterates the **entire** table list to collect that repo's tables. Startup-only, but it is a hidden O(N·M) that grows with deployment size, and the same all-tables list is re-filtered again by `boot_compile_schemas` (fine, single pass) — the pairing loop is the avoidable part.
Failure scenario: a home with thousands of repos × thousands of catalogue rows pays a quadratic scan at every restart before any request is served.
Suggested fix: build a `TFxMap<(db, repo), Vec<TableConfig>>` in one pass over `table_records`, then look up per repo.

### 7. DDL FK guards re-scan the table catalogue once per sibling table — O(tables²) per rename/drop
File:line: `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:258-288` (`rename_table_as` reverse-FK guard); `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:164-199` (`handle_drop_table` FK guard).
Severity: low
Issue: Both guards loop over `db.list_tables(repo)` and call `load_table_record(db, repo, name)` per sibling — each call a full scan of the whole tables catalogue (finding 1) — so the guard itself is O(tables²) record decodes, plus a durable read per table.
Failure scenario: renaming/dropping a table in a repo among thousands of catalogue rows takes thousands of times longer than the useful work; DDL frequency makes this tolerable today, which is the only reason this is low.
Suggested fix: one `load_tables()` pass reused across the guard (load the catalogue once, filter in memory), or the keyed lookup from finding 1.

### 8. Per-invocation gateway construction allocates and intersects allowlists via `Vec::contains`
File:line: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:832-852` (`build_net_gateway`, called from `build_invoke_ctx` `:786-796` and `function_management.rs:733`/`:787`).
Severity: low
Issue: Every function invocation builds a fresh `CurlNetGateway`, cloning and filtering the DB-wide allowlist with a linear `grants.contains(host)` scan — O(grants × allowlist) plus a `Vec`/`Arc` allocation per call. Constants are small today (allowlists are operator-configured and short), so this is polish, not a scaling bug.
Failure scenario: none at current sizes; would only matter if per-function grant lists or the DB allowlist grew large and functions were invoked at high rate.
Suggested fix: compute the effective per-function allowlist once at `create_function`/boot-load time (it only changes on DDL and `set_net_allowlist`) and store the intersection in `function_meta`; `build_net_gateway` then just clones one precomputed `Arc<Vec<String>>`.

### 9. Nit: intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounded by unique-name volume
File:line: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:53-103` (`admin_user_locks`, `group_member_locks`, `repo_create_locks`).
Severity: nit
Issue: Entries "leak by design" (documented inline at each field): one `Arc<Mutex<()>>` per unique user/group/db-name/schema-key forever. All are gated by rare admin/DDL ops, so memory growth is slow and small per entry; the contention model is documented per the CLAUDE.md exception categories. Recorded here only so the theme is complete: no eviction exists, and `admin_user_locks` has additionally accreted a second duty (schema-DDL keys, `admin_schema.rs:73-93`) beyond its original per-user RMW role while `GrantRole`/`RevokeRole` no longer take it (`admin_users_roles.rs:137-141`) — worth a periodic re-audit that every remaining key family is still DDL-only.
Failure scenario: none at current op frequencies.
Suggested fix: none required now; if a family ever migrates to a per-request path, replace with weak-value entries or an LRU under the same documented-contention discipline.
