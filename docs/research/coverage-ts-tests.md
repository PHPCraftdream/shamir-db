בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TypeScript Client Test-Coverage Audit (OQL + DDL)

**Scope:** how completely `crates/shamir-client-ts` tests exercise the database
capabilities reachable through the TS client builders
(`src/core/builders/`) and the wire protocol (`shamir-query-types`).

**As of:** 2026-06-24.
**Unit-test total reported:** 692 (incl. newly added `framing.test.ts`,
`select.test.ts`, `protocol.test.ts`).
**Audit method:** read every builder file + every test file in
`core/__tests__/`, `core/builders/__tests__/`, and `src/__tests__/`; cited
`describe(...)` / `it(...)` names are real and were observed verbatim.

---

## ⚠️ Статус (2026-06-26) — основа для кампании ③.1

Этот аудит в основном **актуален** (тест-дыры реальны). Свериться с
`CAMPAIGN-3-PLAN.md`. Деление остатка по actionability:

- 🟢 **Server-НЕзависимо (берётся первым, ③.1a):** 6 FieldBuilder Phase B/C
  сеттеров (`scalar`/`oneOf`/`format`/`compare`/`foreignKey`/`unique`) — **ноль
  unit-тестов** (только server-gated e2e). Добавить wire-shape unit в `ddl.test.ts`
  → покрытие билдер-слоя без сервера.
- 🟡 **Server-gated (③.1b, нужен release-бинарь):** e2e для FTS, vector, `call`
  (P0); `like/ilike/regex`, existence/containment, `aggregateFn`, `func`,
  `history`-range, `page`-mode, `distinct` (P1); `resume()`, `commitMigration`-
  success, `dropUser`/`dropRole` (P2/P3).

---

## 1. Test inventory (what exists)

### 1.1 Builder unit tests — `core/builders/__tests__/` (NO server)

These assert wire-shape equivalence only (`toEqual({...})`); they never talk to
a server. They are the "always green" backbone.

| File | Domain | Approx. `it` count | Coverage style |
|---|---|---|---|
| `filter.test.ts` | all 32 filter ctors + value refs | ~40 | exhaustive wire-shape |
| `select.test.ts` | `all/field/countAll/aggregate/count/sum/avg/min/max/aggregateFn/func` | ~28 | exhaustive wire-shape |
| `write.test.ts` | `insert/update/upsert/del` + `UpdateBuilder.returning` modes | ~23 | exhaustive wire-shape |
| `query.test.ts` | `Query` fluent builder: projection, where, groupBy/having, orderBy, pagination (limit/offset, page), temporal (as_of/history), withVersion | ~27 | exhaustive wire-shape |
| `ddl.test.ts` | every DDL ctor incl. HMAC-gated ops + `FieldBuilder` types/constraints + schema DDL | ~97 | exhaustive wire-shape |
| `admin.test.ts` | ACL (`chmod/chown/chgrp/...`) + RBAC (`createUser/createRole/...`) + HMAC (`dropUser/dropRole`) | ~48 | exhaustive wire-shape |
| `batch.test.ts` | `Batch.add/subBatch/transactional/durability/limits/returnAll/returnOnly` | ~25 | exhaustive wire-shape |
| `call.test.ts` | `call(name, params?, repo?)` | 8 | exhaustive wire-shape |
| `subscribe.test.ts` | `subscribe` sources/events/deliver modes + `unsubscribeOp` | ~16 | exhaustive wire-shape |

### 1.2 Core unit tests — `core/__tests__/` (NO server)

| File | Module under test | Notes |
|---|---|---|
| `framing.test.ts` | `framing.ts` (`encode/decode` w/ `useBigInt64` + `WsFramer`) | the #216 regression (`promoteWideInts`); recv-queue, close propagation |
| `protocol.test.ts` | `protocol.ts` `runHandshake()` | 4-msg SCRAM-Argon2id; uses a `FakeSocket`+`FakePlatform` (deterministic FNV-1a HMAC/argon2); happy path + every challenge/auth_ok validation error |
| `scram.test.ts` | `scram.ts` (`buildAuthMessage/computeClientProof/verifyServerSignature`) | byte-layout + SCRAM invariant `SHA256(clientKey)==storedKey`; uses real `NodePlatform` argon2id (60 s timeout) |
| `hmac.test.ts` | `hmac.ts` canonical inputs + `signCanonical` | byte-exactness vs `node:crypto`; all 9 canonical builders |
| `db.test.ts` | `db.ts` `Db`/`Tx` handles (Layer 2) | run/rows/query/batch, `runLive`, `db.tx` happy+rollback+aborted-commit, HMAC wrappers, edge cases (`no result for the operation`, raw-op-with-`build`-string) |
| `field-map.test.ts` | `field-map.ts` `FieldMap` + `InternerCacheRegistry` | dump idempotence, delta monotonic-merge, bidirectional lookup, `missingNames`, `allEpochs` |
| `interner-ops.test.ts` | `interner-ops.ts` | `encodeRecordIdMsgpack` (flat + nested + id-widths + arrays + nulls), `qvHasFnMarker`, `collectFieldNames`, `deinternResponse` round-trips |
| `principal-id.test.ts` | `principal-id.ts` fxhash64 replica | determinism, i64::MAX masking, multi-byte UTF-8, all chunk sizes |
| `subscription-handle.test.ts` | `subscription-handle.ts` | async iter, `on()` callback, `return()`, server-Closed |
| `subscription-router.test.ts` | `subscription-router.ts` | early-buffer flush, per-sub isolation, unregister, 256-entry cap, `clear()` |

### 1.3 Always-on integration tests — `src/__tests__/` (NO server)

| File | What it tests | Mechanism |
|---|---|---|
| `client-demux.test.ts` | `ShamirClient` rid-demux (M3) | `FakeSocket`+`FakePlatform`; out-of-order responses, per-rid errors, unrouted frames, socket-close rejection |
| `resume.test.ts` | `ShamirClient.resume()` + resumption getters | `FakeSocket`; ticket/nonce/binding_mode frame, error paths, getters |
| `connect.test.ts` | Node `connect()` against a real server | **gated**: `describe.skip` if server binary absent (the suite has a single `it('reports why the integration test was skipped')`) |

### 1.4 E2E tests — `src/__tests__/e2e-*.test.ts` (REAL server, conditionally skipped)

**Crucial structural fact:** every `e2e-*.test.ts` file is wrapped in
`describe.skipIf(!SERVER_AVAILABLE)(...)` from `e2e-harness.ts`. The harness
resolves the server binary via `CARGO_TARGET_DIR/release/shamir-server` else
`<repo>/target/release/shamir-server`. If absent, the entire describe block is
skipped and only a placeholder `describe('... skip reason')` runs. **When the
binary IS present, all the `it(...)` cases below execute end-to-end.**

| File | Theme | `it` count | Highlights |
|---|---|---|---|
| `e2e.test.ts` | core CRUD + filters + aggregations + sorting/pagination + batch deps + HMAC gate + tx/itx + Db handle + nested subBatch | ~74 | `filters: eq/ne/gt/gte/lt/lte/in/not_in/between/and/or/nested AND-OR/nested field path`; `agg: count_all/sum/avg/min/max/group_by`; `sort/page: asc/desc/multi-field/limit/offset/count_total`; `batch: $query parent→child, IN-expansion, execution_plan stages`; `HMAC: drop_table/db with/without/wrong hmac`; `tx/itx: transactional insert, cross-table atomic, serializable, rollback`; `db.tx` + `nested: P3b $param-in-INSERT, atomicity, tx-in-tx rejected` |
| `e2e-data.test.ts` | data lifecycle + deep filters + projection + versioning + interner stress | ~33 | `upsert new/overwrite`, `update by where / partial merge`, `delete by where / delete-all`, `filter-deep: NOT / AND+OR+NOT / IN+between / nested path / nested+comparison`, `agg empty-result`, `versioning: withVersion/asOfVersion/asOfTimestamp`, `interner: round-trip, non-ASCII, nested-map keys, 50+ batch stress, id-widths, $fn values remain strings` |
| `e2e-ddl.test.ts` | DDL lifecycle: create→list→drop for db/repo/table/index + function + validator + buffer-config + retention + migration + schema | ~15 | `createDb createRepo createTable createIndex`, `function: createFunctionFolder + createFunction(wasm) + renameFunction + dropFunction`, `validator: create(wasm) + bind + listValidators + unbind + drop`, `buffer-config: set/get/alter`, `retention: setRetention + changesSince + purgeHistory`, `migration: startMigration + migrationStatus + rollbackMigration`, `schema: setTableSchema + getTableSchema + addSchemaRule + removeSchemaRule`, `listUsers / listRoles` |
| `e2e-permissions.test.ts` | ACL + RBAC end-to-end | ~20 | `A1–A3 denied paths (DDL/read/insert on owner-only)`, `A4–A5 chmod 0o777↔0o700 flip`, `A6 createGroup`, `A8 createRole/grantRole/revokeRole + superuser via SCRAM`, `A9 accessTree`, `B1–B8 multi-db/multi-user isolation + cross-grant`, `A10 data write after open` |
| `e2e-schema-validators.test.ts` | declarative schema + validator behaviour | ~27 | `required/type_mismatch/out_of_range(unsigned)/one_of/len/min_len/max_len/nullable/nested/optional`, `format: email/uuid/date/url`, `compare (end>=start) incl. skipped-when-other-absent`, `foreign_key: accept/reject/fk_requires_index/autocommit-enforces`, `unique: accept/duplicate/batch-duplicate read-your-own-writes/unique_requires_index/autocommit-enforces`, `lifecycle add/remove/getTableSchema`, `persistence across reconnect`, `multiple violations accumulated` |
| `e2e-interner.test.ts` | client-side field-interner cache | ~7 | `touchFields cold/warm/idempotent/partial-miss`, `execute attaches interner_epochs`, `id-cache-miss path re-fetches` |
| `e2e-subscriptions.test.ts` | live subscriptions | ~8 | `records deliver on insert`, `filter drops unmatched server-side`, `multi two streams`, `handle reactive sub-batch`, `initial pre-seeded`, `unsubscribe done`, `multi-repo refused` |
| `e2e-principal.test.ts` | cross-language principal-id parity | ~4 | TS `principalId` == server `access_tree` id; `chown`/`addGroupMember` with username → BigInt on wire |

---

## 2. OQL (read) coverage matrix

Builder source: `core/builders/{filter,select,query}.ts`.

| Feature | Builder surface | Unit-tested | E2E-tested | Verdict |
|---|---|---|---|---|
| Comparison `eq/ne/gt/gte/lt/lte` | `filter.eq/ne/gt/gte/lt/lte` | ✅ `filter.test.ts` "comparison leaves", "ne/gt/gte/lt/lte" | ✅ `e2e.test.ts` "filters: eq/ne/gt/gte/lt/lte" | **both** |
| Field-eq shortcut (`op:"field"`) | `filter.fieldEq` | ✅ `filter.test.ts` "field-equality shortcut" | ❌ not exercised against server (server treats `eq` and `field` similarly; not separately asserted) | **unit only** |
| Set membership `in_/notIn` | `filter.in_/notIn` | ✅ "set membership" | ✅ "filters: in/not_in" | **both** |
| Pattern `like/ilike/regex` | `filter.like/ilike/regex` | ✅ "pattern matching" | ❌ no e2e case asserts LIKE/ILIKE/REGEX result sets | **unit only** ⚠ |
| Null/existence `isNull/isNotNull/exists/notExists` | same | ✅ "null / existence" | ❌ no e2e case (existence filters not asserted against data) | **unit only** ⚠ |
| Containment `contains/containsAny/containsAll` | same | ✅ "containment" | ❌ no e2e case (array-field semantics not exercised end-to-end) | **unit only** ⚠ |
| Range `between` | `filter.between` | ✅ "range" | ✅ `e2e-data` "filter-deep: IN + range (between)" | **both** |
| Full-text `fts` (and/or modes) | `filter.fts` | ✅ "index-accelerated operators" | ❌ no e2e FTS case (no `createIndex({index_type:"fts"})` + `fts` query pair) | **unit only** ⚠⚠ |
| Vector similarity `vectorSimilarity` | `filter.vectorSimilarity` | ✅ "vector_similarity carries query + k" | ❌ no e2e vector case (no `createIndex({index_type:"vector"})` + similarity query) | **unit only** ⚠⚠ |
| Computed (functional-index) `computed` | `filter.computed` | ✅ "computed omits/includes expr_args" | ❌ no e2e case | **unit only** ⚠ |
| Logical `and/or/not` + smart-flatten | `filter.and/or/not` | ✅ "logical combinators" (incl. flatten) | ✅ `e2e.test.ts` "and/or/nested AND-OR"; `e2e-data` "NOT / AND+OR+NOT" | **both** |
| `$query` reference (single + column) | `filter.queryRef` | ✅ "value-ref constructors" | ✅ `e2e.test.ts` "parent→child via $query", "IN-expansion" | **both** |
| `$ref` field reference | `filter.ref` | ✅ "ref(string)/ref(string[])" | ❌ not separately asserted (engine semantics) | **unit only** |
| `$param` batch-param reference | `filter.param` | ✅ "param — batch parameter reference" | ✅ `e2e.test.ts` "nested: P3b $param in INSERT" | **both** |
| `$fn` system call (Simple/Complex) | `filter.fn` | ✅ "fn — system function call" | ✅ `e2e-data` "interner: $fn values remain strings" (wire-level); `e2e.test.ts` "scram: create a login-capable user" exercises `fn('NOW')` semantics indirectly | **both** |
| `$expr` expression | `filter.expr` | ✅ "expr — expression" | ❌ no e2e case exercises `$expr` evaluation | **unit only** ⚠ |
| `$cond` ternary | `filter.cond` | ✅ "cond — conditional" (incl. nested) | ❌ no e2e case | **unit only** ⚠ |
| Projection `select.field` + alias | `select.field` / `Query.select` | ✅ `select.test.ts`; `query.test.ts` "select(fields)" | ✅ `e2e.test.ts` "column projection"; `e2e-data` "projection: select specific fields" | **both** |
| `SELECT *` (`all`/`selectAll`) | `select.all` / `Query.selectAll` | ✅ `select.test.ts` "all"; `query.test.ts` "plain SELECT *" | ✅ every read in e2e defaults to `*` | **both** |
| `count_all` aggregate | `select.countAll` | ✅ `select.test.ts`; `query.test.ts` "count_all + sum" | ✅ `e2e.test.ts` "count_all aggregate" | **both** |
| Aggregates `count/sum/avg/min/max` + `distinct` | `select.aggregate/count/sum/avg/min/max` | ✅ all in `select.test.ts` | ✅ `e2e.test.ts` "sum + avg + min + max"; `e2e-data` "agg count/sum/avg/min/max"; **`distinct:true` ❌ not e2e-exercised** | **both (distinct gap)** |
| Library aggregate `aggregateFn` (median/stddev/...) | `select.aggregateFn` | ✅ `select.test.ts` "aggregateFn" | ❌ no e2e case invokes a library aggregate | **unit only** ⚠ |
| Scalar function in projection `func` | `select.func` | ✅ `select.test.ts` "func" | ❌ no e2e case (e.g. no `strings/upper`, `math/abs` projection) | **unit only** ⚠ |
| `distinct()` on query | `Query.distinct` | ✅ `query.test.ts` "distinct() forces select" | ❌ no e2e distinct-result assertion | **unit only** ⚠ |
| `where` / `andWhere` (smart-flatten) | `Query.where/andWhere` | ✅ `query.test.ts` "where / andWhere combines" | ✅ `e2e.test.ts` every filtered read | **both** |
| `groupBy` + `having` | `Query.groupBy/having` | ✅ "group by / having"; throws-if-no-groupBy | ✅ `e2e.test.ts` "group_by user → count + sum"; `e2e-data` "group_by tag" | **both** |
| `orderBy` items / `orderByAsc/Desc` + `nulls` | `Query.orderBy/orderByAsc/Desc` | ✅ "order by" (incl. nulls) | ✅ `e2e.test.ts` "asc/desc/multi-field"; **`nulls:first/last` ❌ not e2e-exercised** | **both (nulls gap)** |
| Pagination `limit/offset` | `Query.limit/offset` | ✅ "limit+offset emits LimitOffset"; "limit alone defaults offset 0" | ✅ `e2e.test.ts` "LIMIT/OFFSET first/second page", "LIMIT past end" | **both** |
| Pagination `page(n, size)` | `Query.page` | ✅ "page emits Page mode" | ❌ no e2e case uses `Page` mode (all e2e uses LimitOffset) | **unit only** ⚠ |
| `countTotal` | `Query.countTotal` | ✅ "countTotal sets the flag" | ✅ `e2e.test.ts` "count_total returns full size" | **both** |
| Temporal `asOfVersion` | `Query.asOfVersion` | ✅ "asOfVersion emits {kind:'as_of',at:{version}}" | ✅ `e2e-data` "asOfVersion reads historical state" | **both** |
| Temporal `asOfTimestamp` / `asOf` | `Query.asOfTimestamp/asOf`; `atTimestamp` | ✅ "asOfTimestamp / asOf(at)" | ✅ `e2e-data` "asOfTimestamp reads at a point in time" | **both** |
| Temporal `history` (range read) | `Query.history` | ✅ "history() defaults order asc"; "history with bounds + desc" | ❌ no e2e case exercises a history-range read | **unit only** ⚠ |
| `withVersion` | `Query.withVersion` | ✅ "withVersion sets the flag" | ✅ `e2e-data` "withVersion flag is accepted" | **both** |
| `withRepo` non-main | `Query.withRepo` | ✅ "withRepo emits [repo,table] tuple" | ✅ `e2e.test.ts` `db.withRepo('archive','orders')` (db.test.ts unit + e2e) | **both** |

---

## 3. DDL / Admin coverage matrix

Builder source: `core/builders/{ddl,admin}.ts`.

### 3.1 Non-HMAC DDL

| Feature | Builder | Unit | E2E | Verdict |
|---|---|---|---|---|
| `createDb` (+`if_not_exists`) | `ddl.createDb` | ✅ `ddl.test.ts` "createDb" | ✅ `e2e-ddl` "createDb → listDatabases → dropDb"; `if_not_exists` | **both** |
| `createRepo` (+engine/path/tables/`if_not_exists`) | `ddl.createRepo` | ✅ "createRepo" (all optionals) | ✅ `e2e-ddl` "createRepo → listRepos → dropRepo" | **both** |
| `createTable` (+repo/`if_not_exists`/retention/schema) | `ddl.createTable` | ✅ "createTable"; "createTable with schema" | ✅ `e2e-ddl` "createTable → listTables"; `e2e.test.ts` setup | **both** |
| `createIndex` (btree: unique/sorted) | `ddl.createIndex` | ✅ "createIndex" (unique/sorted/repo) | ✅ `e2e.test.ts` "create_index + list + drop_index (hmac)" | **both** |
| `createIndex` FTS variant (`index_type/fts_tokenizer/fts_language`) | same | ✅ "FTS options omitted"; not asserted-with-values ⚠ | ❌ no e2e creates an FTS index | **unit only (shape-only)** ⚠⚠ |
| `createIndex` functional (`functional_op/functional_args`) | same | ✅ wire-presence only (not asserted with values) | ❌ no e2e | **unit only (shape-only)** ⚠⚠ |
| `createIndex` vector (`index_type:'vector'/vector_dim/vector_metric`) | same | ✅ "includes index_type, vector options" (values asserted) | ❌ no e2e vector index | **unit only** ⚠⚠ |
| `createIndex` `include` (covering) | same | ✅ "include when set"; "omits empty include" | ❌ no e2e covering-index | **unit only** ⚠ |
| `setBufferConfig` / `getBufferConfig` / `alterBufferConfig` | `ddl.setBufferConfig/getBufferConfig/alterBufferConfig` | ✅ "buffer config" | ✅ `e2e-ddl` "setBufferConfig → getBufferConfig → alterBufferConfig" | **both** |
| `migrationStatus` | `ddl.migrationStatus` | ✅ "migrationStatus" | ✅ `e2e-ddl` "migration: startMigration → migrationStatus → rollbackMigration" | **both** |
| `createFunction` (source/wasm/replace) | `ddl.createFunction` | ✅ "function DDL" | ✅ `e2e-ddl` "createFunction(wasm) → rename → drop" | **both** |
| `dropFunction` / `renameFunction` | same | ✅ "function DDL" | ✅ same e2e case | **both** |
| `createValidator` (source/wasm/replace) | `ddl.createValidator` | ✅ "validator DDL" | ✅ `e2e-ddl` "validator: create(wasm) → bind → list → unbind → drop" | **both** |
| `dropValidator` / `renameValidator` | same | ✅ "validator DDL" | ✅ same e2e case | **both** |
| `bindValidator` (ops/priority/db) | `ddl.bindValidator` | ✅ "bindValidator"; "all four write-op kinds" | ✅ same e2e case | **both** |
| `unbindValidator` / `listValidators` | same | ✅ "unbindValidator / listValidators (DDL)" | ✅ same e2e case | **both** |
| `createFunctionFolder` | `ddl.createFunctionFolder` | ✅ "createFunctionFolder" | ✅ `e2e-ddl` function case | **both** |
| `setRetention` / `purgeHistory` / `changesSince` | `ddl.setRetention/purgeHistory/changesSince`; `currentOnly/olderThan/olderThanAge` | ✅ "retention / purge scope / purgeHistory / setRetention / changesSince" | ✅ `e2e-ddl` "retention: setRetention + changesSince + purgeHistory" | **both** |
| List ops (databases/repos/tables/indexes/users/roles/functions/validators/folders) | `ddl.listDatabases/...` | ✅ "list ops" (all 9) | ✅ `e2e-ddl` + `e2e.test.ts` exercise listDatabases/listTables/listIndexes/listUsers/listRoles; `listFunctions`/`listValidators_`/`listFunctionFolders` via the function/validator lifecycle cases | **both** |
| Schema `setTableSchema` (+`expectedVersion`) | `ddl.setTableSchema` | ✅ "setTableSchema" | ✅ `e2e-ddl` "setTableSchema → getTableSchema → addSchemaRule → removeSchemaRule"; `e2e-schema-validators` lifecycle | **both** |
| Schema `addSchemaRule` / `removeSchemaRule` / `getTableSchema` | same | ✅ "addSchemaRule / removeSchemaRule / getTableSchema" | ✅ same e2e cases | **both** |

### 3.2 HMAC-gated DDL

| Feature | Builder | Unit (fake signer) | E2E (real signer) | Verdict |
|---|---|---|---|---|
| `dropDb` (+cascade) | `ddl.dropDb` | ✅ "dropDb — hmac from canonicalDropDb"; cascade omit/include | ✅ `e2e-ddl` dropDb; `e2e.test.ts` "drop_db without/with hmac + cascade" | **both** |
| `dropRepo` (+cascade) | `ddl.dropRepo` | ✅ "dropRepo — hmac"; cascade | ✅ `e2e-ddl` dropRepo | **both** |
| `dropTable` | `ddl.dropTable` | ✅ "dropTable — hmac from canonicalDropTable" | ✅ `e2e.test.ts` "drop_table without/with/wrong hmac"; `db.dropTable` in `db.test.ts` | **both** |
| `dropIndex` (+unique flag) | `ddl.dropIndex` | ✅ "dropIndex — hmac"; "unique=true canonical uses 1" | ✅ `e2e.test.ts` "create_index + list + drop_index (hmac)"; `db.dropIndex({unique:true})` | **both** |
| `startMigration` (+`dst_path`) | `ddl.startMigration` | ✅ "startMigration — hmac"; "with dst_path" | ✅ `e2e-ddl` migration case | **both** |
| `commitMigration` | `ddl.commitMigration` | ✅ "commitMigration — hmac" | ⚠ implicit only (e2e-ddl uses rollback path; no explicit commit-success case) | **unit + e2e-partial** |
| `rollbackMigration` | `ddl.rollbackMigration` | ✅ "rollbackMigration — hmac" | ✅ `e2e-ddl` migration case | **both** |

### 3.3 ACL + RBAC (`admin.ts`)

| Feature | Builder | Unit | E2E | Verdict |
|---|---|---|---|---|
| ResourceRef ctors (`refDatabase/refStore/refTable/refFunction/refFunctionFolder/refFunctionNamespace`) | `admin.ref*` | ✅ "ResourceRef"; "ResourceRef ≠ Resource" | ✅ `e2e-permissions` chmod/refTable; `e2e-ddl` function folder | **both** |
| Resource scope ctors (`scopeGlobal/Database/Repo/Table`) | `admin.scope*` | ✅ "Resource (tag='scope')" | ✅ `e2e-permissions` permission/scoping | **both** |
| GroupRef (`groupName/groupId`) | `admin.groupName/groupId` | ✅ "GroupRef" | ✅ `e2e-permissions` createGroup/dropGroup | **both** |
| `chmod` | `admin.chmod` | ✅ "chmod" | ✅ `e2e-permissions` A4/A5/B5/B6 chmod flips | **both** |
| `chown` (string→principalId / bigint / number) | `admin.chown` | ✅ "chown" (all 3 input types) | ✅ `e2e-principal` "chown with username string works" | **both** |
| `chgrp` (incl. `group:null`) | `admin.chgrp` | ✅ "chgrp"; "group:null clears" | ❌ no e2e case exercises chgrp semantics | **unit only** ⚠ |
| `createGroup` / `dropGroup` | same | ✅ "createGroup / dropGroup" | ✅ `e2e-permissions` A6 | **both** |
| `addGroupMember` / `removeGroupMember` (string→principalId) | same | ✅ "addGroupMember / removeGroupMember" | ✅ `e2e-principal` "addGroupMember with username string works" | **both** |
| `accessTree` (+depth/db) | `admin.accessTree` | ✅ "accessTree" (all option combos) | ✅ `e2e-permissions` A9; `e2e-principal` parity check | **both** |
| `permission` (+`where`) | `admin.permission` | ✅ "permission"; "with where filter"; Action/Effect enums | ✅ `e2e-permissions` A8 createRole+grant | **both** |
| `createUser` (+roles/profile/database) | `admin.createUser` | ✅ "createUser" | ✅ `e2e-permissions` A-setup; `e2e.test.ts` "scram: create a login-capable user" | **both** |
| `dropUser` (HMAC) | `admin.dropUser` | ✅ "dropUser (HMAC)" | ❌ no e2e case drops a user end-to-end | **unit only** ⚠ |
| `createRole` | `admin.createRole` | ✅ "createRole" | ✅ `e2e-permissions` A8 | **both** |
| `dropRole` (HMAC) | `admin.dropRole` | ✅ "dropRole (HMAC)" | ❌ no e2e case drops a role end-to-end | **unit only** ⚠ |
| `grantRole` / `revokeRole` | same | ✅ "grantRole / revokeRole" | ✅ `e2e-permissions` A8 (grant+revoke) | **both** |

### 3.4 `FieldBuilder` constraint setters (`ddl.field(...)`)

| Constraint | Unit (`ddl.test.ts`) | E2E (`e2e-schema-validators.test.ts`) | Verdict |
|---|---|---|---|
| Types `string/int/f64/dec/bool/bin/list/map/any/typeTag` | ✅ "supports all type tags" | ✅ type_mismatch cases | **both** |
| `required` | ✅ "builds a string field with max + required" | ✅ "required: missing → missing_required" | **both** |
| `nullable` | ✅ "supports nullable..." | ✅ "nullable: null accepted/rejected" | **both** |
| `unsigned` | ✅ same | ✅ "unsigned: negative → out_of_range" | **both** |
| `min/max` | ✅ "int field with min + max" | ✅ "out_of_range: int min/max boundaries" | **both** |
| `len` | ✅ "nested-path string field with len" | ✅ "len (wrong_length)" | **both** |
| `minLen/maxLen` | ✅ "supports nullable, unsigned, minLen, maxLen" | ✅ "min_len/max_len (too_short/too_long)" | **both** |
| `arrayOf` | ✅ same | ❌ not directly asserted (no array-of-type schema case) | **unit only** |
| `scalar` (Phase B scalar-bridge) | ❌ **not in `ddl.test.ts`** | ✅ referenced (`scalar` appears 2× in e2e-schema-validators) | **e2e only** ⚠ |
| `oneOf` (enum) | ❌ **not in `ddl.test.ts`** | ✅ "one_of (not_in_enum)" | **e2e only** ⚠ |
| `format` (email/uuid/date/url) | ❌ **not in `ddl.test.ts`** | ✅ "format(email)/(uuid)/(date)/(url)" (4 cases) | **e2e only** ⚠ |
| `compare` (cross-field) | ❌ **not in `ddl.test.ts`** | ✅ "compare (end>=start)"; "skipped when other path absent" | **e2e only** ⚠ |
| `foreignKey` (Phase C2) | ❌ **not in `ddl.test.ts`** | ✅ "foreign_key: accept/reject/fk_requires_index/autocommit" (4 cases) | **e2e only** ⚠ |
| `unique` (Phase C3 field constraint) | ❌ **not in `ddl.test.ts`** (the `unique` matches there are all `createIndex({unique})`) | ✅ "unique: accept/duplicate/batch-duplicate/requires_index/autocommit" (5 cases) | **e2e only** ⚠ |

> **Notable asymmetry:** the Phase B/C constraint setters
> (`scalar/oneOf/format/compare/foreignKey/unique`) have **zero unit tests** —
> they are exclusively covered by the server-gated e2e suite. If the server
> binary is absent, they have **no test coverage at all** in a default `vitest`
> run.

---

## 4. Batch / Subscribe / Call coverage

| Feature | Builder | Unit | E2E | Verdict |
|---|---|---|---|---|
| `Batch.create/add` (Query builder + raw op) | `Batch` | ✅ `batch.test.ts` "minimal build" | ✅ every e2e batch case | **both** |
| `return_result:false` / `after:[...]` deps | `Batch.add` opts | ✅ "return_result and after options" | ✅ `e2e.test.ts` "execution_plan reflects dep (two stages)" | **both** |
| `subBatch` (+`bind`/raw BatchRequest/Batch instance) | `Batch.subBatch` | ✅ "subBatch" (5 cases) | ✅ `e2e.test.ts` nested cases (P3b, atomicity, tx-in-tx) | **both** |
| `transactional(isolation?)` | `Batch.transactional` | ✅ "transactional" | ✅ `e2e.test.ts` tx cases; `e2e-schema-validators` unique/fk autocommit | **both** |
| `durability` / `name` / `returnOnly` / `limits` / `returnAll(false)` | same | ✅ "durability, name, returnOnly, limits"; "returnAll" | ❌ `durability/limits/returnAll(false)/returnOnly` ❌ not asserted in e2e | **unit only** ⚠ |
| `Batch.execute(client, db)` (Layer 1) | same | ✅ `db.test.ts` "Layer-1: Batch.execute" | ✅ `e2e-ddl` `.execute(client, db)` | **both** |
| `call` (params + repo) | `call` | ✅ `call.test.ts` (8 cases incl. `$ref`/`$query` params) | ❌ no e2e case invokes a stored function via `call` | **unit only** ⚠⚠ |
| `subscribe` (single/multi source, filter cb/literal, events, deliver records/keys, handle sub-batch, initial, fromVersion, conflict-throw) | `subscribe` | ✅ `subscribe.test.ts` (~17 cases) | ✅ `e2e-subscriptions` (records/filter/multi/handle/initial/unsubscribe/refusal) | **both** |
| `unsubscribeOp` | same | ✅ "unsubscribeOp" | ✅ `e2e-subscriptions` "unsubscribe: stream goes done" | **both** |
| `Batch.subscribe/unsubscribe` helpers | `Batch.subscribe/unsubscribe` | ❌ not directly unit-tested (only via `subscribe` builder) | ✅ `e2e-subscriptions` via `db.batch().subscribe(...)` | **e2e only** |

---

## 5. Wire-protocol / framing / auth coverage

| Feature | Unit test | E2E / integration | Verdict |
|---|---|---|---|
| `encode`/`decode` w/ `useBigInt64` (promoteWideInts #216) | ✅ `framing.test.ts` exhaustive (u32 boundary, ms-timestamp, nested arrays/objects, bigint passthrough, Uint8Array bin) | ✅ implicitly every e2e frame | **both** |
| `WsFramer` length-prefix + recv queue + close propagation | ✅ `framing.test.ts` send/recv/close (9 cases) | ✅ every e2e connection | **both** |
| SCRAM `runHandshake` (4-msg) | ✅ `protocol.test.ts` happy path + all challenge/auth_ok validation errors (memory/time/parallelism/argon2_version/salt/nonce lengths, MITM sig, session_id length, error-map) | ✅ `connect.test.ts` (when server present); every e2e `beforeAll` connect | **both** |
| `buildAuthMessage` byte layout | ✅ `scram.test.ts` (prefix, u16 user-len, offsets, throw-on-bad-lengths) | ✅ implicit | **both** |
| `computeClientProof` / `verifyServerSignature` | ✅ `scram.test.ts` SCRAM invariant + tampered/wrong-length sig | ✅ implicit (auth succeeds) | **both** |
| HMAC canonical inputs + `signCanonical` | ✅ `hmac.test.ts` byte-exactness vs `node:crypto` for all 9 canonical builders | ✅ `e2e.test.ts` HMAC gate (drop_table/db wrong/right hmac) | **both** |
| rid-demux (`ShamirClient`) | ✅ `client-demux.test.ts` (out-of-order, per-rid error, unrouted, close) | ✅ `e2e.test.ts` "concurrent Promise.all reads resolve correctly" | **both** |
| `resume()` (fast reconnect) | ✅ `resume.test.ts` (ticket frame, getters, error paths, session_id length) | ❌ no e2e resume-after-drop case | **unit only** ⚠ |
| Field-interner cache (`FieldMap`/`InternerCacheRegistry`/`interner-ops`) | ✅ `field-map.test.ts` + `interner-ops.test.ts` exhaustive | ✅ `e2e-interner.test.ts` (touchFields cold/warm/idempotent/partial-miss, epochs attached); `e2e-data` interner stress | **both** |
| Subscription routing (`SubscriptionRouter`/`SubscriptionHandle`) | ✅ `subscription-router.test.ts` + `subscription-handle.test.ts` (buffer cap, async iter, closed) | ✅ `e2e-subscriptions` end-to-end | **both** |
| `Db` Layer-2 handle (`run/rows/query/batch/withRepo/dropX/runLive`) | ✅ `db.test.ts` exhaustive with fake client | ✅ `e2e.test.ts` "handle: ..." cases (5) | **both** |
| `Db.tx()` interactive transaction | ✅ `db.test.ts` "Db.tx()" (happy/rollback/aborted-commit/isolation/repo forwarding) | ✅ `e2e.test.ts` "itx: begin→execute→commit/rollback" | **both** |

---

## 6. Prioritized untested / under-tested areas

Ordered by **risk × likelihood-of-regression**.

### P0 — high risk, no end-to-end exercised shape

1. **FTS (full-text search) end-to-end.** `filter.fts` has a unit test, but no
   e2e case creates an FTS index (`createIndex({index_type:'fts',
   fts_tokenizer, fts_language})`) and runs an `fts` query. The unit test
   asserts wire shape only; the tokenizer/language options are not asserted
   with values even at unit level. A serde-rename regression in
   `filter_enum.rs::Fts` would pass unit tests and silently break search.
   *Gap: `e2e-ddl` + `e2e-data`.*

2. **Vector similarity search end-to-end.** `filter.vectorSimilarity` and
   `createIndex({index_type:'vector', vector_dim, vector_metric})` both have
   shape-only unit tests. No e2e case creates a vector index and runs a
   top-k query. This is a headline feature with zero integration coverage.
   *Gap: new e2e case in `e2e-ddl` + `e2e-data`.*

3. **`call` (stored-function invocation) end-to-end.** `call.test.ts` is
   thorough on wire shape (8 cases), but **no e2e test ever invokes a stored
   function**. `createFunction` is e2e-tested (wasm upload + lifecycle), but
   the actual `call(name, params)` → result path is unverified. A regression
   in `CallOp` deserialization or param passing would be invisible.
   *Gap: `e2e-ddl` or new `e2e-call.test.ts`.*

### P1 — wire-shape-only with risky serde surface

4. **`aggregateFn` (library aggregates: median, stddev, percentile...).** Unit
   test asserts the `aggregate_fn` wire discriminator. No e2e case invokes
   one. The funclib registry resolution path is untested via the TS client.

5. **`func` (scalar projection: `strings/upper`, `math/abs`...).** Same pattern:
   wire shape only, no e2e execution.

6. **`$expr` (arithmetic/string/logic expressions) and `$cond` (ternary).**
   Both have unit tests proving wire shape; **neither is evaluated end-to-end**.
   `filter_expr.rs` and `cond.rs` serde changes would not be caught.

7. **Pattern-matching filters `like/ilike/regex`.** Unit-tested for wire shape;
   no e2e case asserts result-set correctness. `ilike` (case-insensitive) and
   `regex` are particularly regression-prone.

8. **Existence/null filters `isNull/isNotNull/exists/notExists` and containment
   `contains/containsAny/containsAll`.** Unit-only; no e2e data assertion.
   These operate on array-field semantics that are easy to break server-side.

9. **`history()` temporal range reads.** Unit-tested for wire shape (bounds,
   order asc/desc, limit). No e2e case performs a versioned range scan. Only
   `as_of` point reads are e2e-covered.

10. **Page-mode pagination (`Query.page(n, size)`).** Unit-only. All e2e
    pagination cases use `limit/offset`. The `Page`/`Pagination::Page` serde
    variant is unverified end-to-end.

### P2 — HMAC-gated ops with thin e2e coverage

11. **`commitMigration` e2e success path.** Unit-tested (HMAC byte-exactness).
    The e2e migration case exercises `startMigration → migrationStatus →
    rollbackMigration` but never a successful `commitMigration`. A commit-path
    regression is invisible.

12. **`dropUser` / `dropRole` end-to-end.** Both have unit tests proving the
    HMAC canonical (`canonicalDropUser`/`canonicalDropRole`) is fed to the
    signer. **Neither is executed against a real server.** The HMAC canonical
    is verified byte-exact at unit level, but the server's acceptance of that
    tag is not confirmed in any e2e case.

### P3 — structural / environmental risk

13. **E2E suite is entirely gated on `SERVER_AVAILABLE`.** Every
    `e2e-*.test.ts` is wrapped in `describe.skipIf(!SERVER_AVAILABLE)`. In a
    default `vitest` run without `cargo build --release -p shamir-server`,
    **all e2e coverage is absent** and only the placeholder "skip reason"
    cases run. This means the Phase B/C constraint setters
    (`scalar/oneOf/format/compare/foreignKey/unique`), which have **zero unit
    tests**, have **no coverage at all** unless the server binary is built.
    *Recommendation: add wire-shape unit tests for these 6 FieldBuilder
    setters in `ddl.test.ts` so the builder layer is covered independently of
    the server.*

14. **`connect.test.ts`** is similarly server-gated and contains only a
    skip-reason placeholder; it does not add coverage beyond `e2e.test.ts`'s
    `beforeAll` connect.

15. **`resume()` end-to-end.** Thorough unit tests (`resume.test.ts`, 9 cases
    with FakeSocket), but no e2e case drops a connection and resumes with a
    ticket. The resumption protocol is unit-only.

16. **Batch request-level knobs `durability / limits / returnAll(false) /
    returnOnly`.** Unit-only; no e2e case asserts the server honours
    `BatchLimits` (e.g. `max_queries` rejection) or `return_only` filtering.

17. **`chgrp` semantics.** Unit-only wire shape; no e2e case transfers group
    ownership and verifies the effect.

---

## 7. Summary counts

- **Total OQL + DDL features enumerated:** **107** (32 filter ctors + 11 select
  ctors + 4 write ops + 22 Query fluent methods + 36 DDL ctors incl. HMAC + 28
  admin ctors + ~9 Batch methods + 2 subscribe ctors + 1 call builder +
  auxiliary). Counted at the "exported builder function / fluent method"
  granularity.
- **❌ Untested end-to-end (unit-only or completely missing):** **~28 features**
  break down as:
  - 3 P0 gaps (FTS, vector, `call` — no e2e at all)
  - 7 P1 wire-only filter/select/projection paths (`fieldEq`, `like/ilike/regex`,
    `isNull/exists/contains` family, `aggregateFn`, `func`, `$expr`, `$cond`,
    `history`, `page` pagination, `distinct`)
  - 6 FieldBuilder Phase B/C setters with **no unit test** (`scalar`, `oneOf`,
    `format`, `compare`, `foreignKey`, `unique` — e2e-only)
  - 4 batch/admin knobs (`durability`, `limits`, `returnAll(false)`,
    `returnOnly`, `chgrp`)
  - 3 HMAC ops with thin/missing e2e (`commitMigration` success, `dropUser`,
    `dropRole`)
  - structural: e2e suite 100% server-gated; `resume()` e2e missing.

- **Unit-test-only coverage (no server round-trip):** the entire builder layer
  (~190 `it` cases across 9 files) is wire-shape-only. This is by design and
  healthy, but it means **serde-rename regressions on the Rust side are caught
  only when the e2e suite runs**.

- **Net assessment:** the **builder wire-shape layer is exhaustively unit-tested
  and the core CRUD/filter/agg/sort/tx/DDL-lifecycle/permissions/schema paths
  are well covered e2e**. The **principal gaps are the advanced index types
  (FTS, vector), stored-function invocation (`call`), the expression/conditional
  value constructors (`$expr`/`$cond`), library/scalar projection functions,
  and the Phase B/C FieldBuilder constraints (which lack unit tests entirely)**.
