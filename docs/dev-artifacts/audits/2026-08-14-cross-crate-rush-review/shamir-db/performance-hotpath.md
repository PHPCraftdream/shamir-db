<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-db — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Batch ACL deduplication is fixed, but unindexed catalogue scans and repeated function/list/FK lookups remain. Current byte-level prefilters make the original claim of fully decoding and de-interning every scanned row inaccurate. No latency conclusions were measured.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 1 | 1 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request

Status: `partially-fixed`. Current risk: `medium`.

System lookup filters still scan unindexed catalogues, but Authorized now deduplicates repeated action/path pairs. Cost is per distinct check, across differently sized ancestor catalogues; byte-level prefiltering avoids full de-interning of every rejected row.

Evidence: [crates/shamir-db/src/shamir_db/system_store.rs:97](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L97); [crates/shamir-db/src/shamir_db/system_store.rs:813](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L813); [crates/shamir-engine/src/query/batch/authorized.rs:102](../../../../../crates/shamir-engine/src/query/batch/authorized.rs#L102); [crates/shamir-engine/src/table/read_exec.rs:895](../../../../../crates/shamir-engine/src/table/read_exec.rs#L895); [crates/shamir-engine/src/table/table.rs:318](../../../../../crates/shamir-engine/src/table/table.rs#L318).

<a id="review-2"></a>

### Claim 2 — `execute_as` re-authorizes every op in a batch without dedupe (the inline ACL cache exists only in `tx_execute_as`)

Status: `fixed`. Current risk: —.

Both entry points now use Authorized's set-based deduplication before execution. The original N-identical-operations/N-traversals mechanism no longer exists.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_execute.rs:40](../../../../../crates/shamir-db/src/shamir_db/execute/db_execute.rs#L40); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:126](../../../../../crates/shamir-db/src/shamir_db/execute/db_tx.rs#L126); [crates/shamir-engine/src/query/batch/authorized.rs:104](../../../../../crates/shamir-engine/src/query/batch/authorized.rs#L104).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Function invocation scans the function catalogue twice plus two settings scans per call

Status: `confirmed-open`. Current risk: `medium`.

User invocation still resolves the function through authorization and loads it again in effective_fn_actor. Root/namespace and any folder ancestors add their own reads; System/Admin skip authorization but effective_fn_actor still loads.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L711); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:720](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L720); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:93](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L93); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:995](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L995).

<a id="review-4"></a>

### Claim 4 — False "O(1) point lookup" comments encode a scan-based cost model (and hide an O(N²) introspection path)

Status: `confirmed-open`. Current risk: `medium`.

The comments remain false for current unindexed filtered reads. Listing performs one scan per live function/validator, structurally O(live registrations × catalogue rows), quadratic when both scale together.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_access.rs:21](../../../../../crates/shamir-db/src/shamir_db/execute/admin_access.rs#L21); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:377](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L377); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:382](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L382); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:418](../../../../../crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L418).

<a id="review-5"></a>

### Claim 5 — `InternerTouch` computes the epoch via a full interner traversal per touch

Status: `confirmed-open`. Current risk: `medium`.

all_entries still allocates/materializes the dictionary before max. The interner already has an allocation counter, but IDs can have gaps and concurrent publication holes. Max of this request's mappings is not the global epoch, especially for retouches or an empty request.

Evidence: [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:170](../../../../../crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L170); [crates/shamir-types/src/core/interner/interner.rs:157](../../../../../crates/shamir-types/src/core/interner/interner.rs#L157); [crates/shamir-types/src/core/interner/interner.rs:328](../../../../../crates/shamir-types/src/core/interner/interner.rs#L328); [crates/shamir-types/src/core/interner/interner.rs:522](../../../../../crates/shamir-types/src/core/interner/interner.rs#L522).

<a id="review-6"></a>

### Claim 6 — Boot path pairs repos with their tables via an O(repos × tables) nested scan

Status: `confirmed-open`. Current risk: `low`.

Every repository still filters the entire table_records list. One-pass grouping would remove the product term; this is startup structural work, not a measured restart delay.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:210](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L210); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:230](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L230).

<a id="review-7"></a>

### Claim 7 — DDL FK guards re-scan the table catalogue once per sibling table — O(tables²) per rename/drop

Status: `confirmed-open`. Current risk: `low`.

Both guards still call load_table_record per sibling, and that lookup scans the global table catalogue. Exact structural cost is O(repo siblings × global catalogue rows), not necessarily the square of one count.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:258](../../../../../crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L258); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:165](../../../../../crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L165); [crates/shamir-db/src/shamir_db/system_store.rs:870](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L870).

<a id="review-8"></a>

### Claim 8 — Per-invocation gateway construction allocates and intersects allowlists via `Vec::contains`

Status: `confirmed-open`. Current risk: `low`.

Function metadata is cloned, a fresh effective Vec is built using linear contains, and a gateway Arc is allocated per invocation. Allocation/product complexity is proven; list sizes, material latency and benefits of caching are not measured.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:832](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L832); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:847](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L847); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:851](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L851).

<a id="review-9"></a>

### Claim 9 — Nit: intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounded by unique-name volume

Status: `confirmed-open`. Current risk: `nit`.

Per-key lock entries remain non-evicting and document rare mutation contention. The 'only unbounded-growth sites' claim is unsupported: catalogues/registries also grow. The admin_user_locks production key family is currently schema-only.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:55](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L55); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:66](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L66); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:99](../../../../../crates/shamir-db/src/shamir_db/shamir_db/core.rs#L99); [crates/shamir-db/src/shamir_db/execute/admin_schema.rs:93](../../../../../crates/shamir-db/src/shamir_db/execute/admin_schema.rs#L93).

## Corrections and qualified non-findings

- Replace per-op ACL cost with per-distinct-(action,path) cost after Authorized deduplication; do not call a two-variable product inherently quadratic.
- Remove the 5 × 10k full-decodes example: ancestors use different catalogue tables, and byte-level prefiltering rejects rows without full de-interning.
- authorize_gate still sets up one database/repository/table and has no catalogue-cardinality axis; it does not validate the reported scaling or latency multipliers.
- A Filter::Eq substitution is not a keyed storage shortcut. Current SetOp key semantics do not by themselves prove an efficient facade get-by-key read exists.
- InternerTouch's suggested mappings-max fix is incorrect. A replacement epoch accessor must respect global published-entry and gap semantics.
- Per-key lock retention is documented admin/schema design debt, not an established request-hot-path leak or exclusive source of unbounded growth.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
