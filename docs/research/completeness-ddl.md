בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# DDL Completeness Assessment — S.H.A.M.I.R. DB

**Scope:** the database-management / admin surface — what the typed `ddl::*`
builders in `shamir-query-builder` and the underlying `BatchOp` variants in
`shamir-query-types` can express, and what the engine actually enforces.

**Method:** read the source of truth (cited inline) and graded each feature
**present / partial / absent / out-of-scope**, distinguishing *validator-based*
(declared via schema rules / bound WASM, enforced on the write path) from
*constraint-enforced* (a structural invariant the engine checks independently of
user code).

---

## 1. What DDL HAS — feature inventory

### 1.1 Container lifecycle (db / repo / table / index)

| Feature | Builder | Wire DTO | Status |
|---|---|---|---|
| `CREATE DATABASE` | `ddl::create_db` (`create_db.rs`) | `CreateDbOp` (`db_ops.rs`) — `if_not_exists` | ✅ present |
| `DROP DATABASE` | `ddl::drop_db` | `DropDbOp` — `hmac` + `cascade` (recursive) | ✅ present |
| `CREATE REPO` (store) | `ddl::create_repo` | `CreateRepoOp` — `engine`, `path`, `tables[]`, `if_not_exists` | ✅ present |
| `DROP REPO` | `ddl::drop_repo` | `DropRepoOp` — `hmac` + `cascade` | ✅ present |
| `CREATE TABLE` | `ddl::create_table` | `CreateTableOp` — `if_not_exists`, optional inline `Retention`, optional inline declarative `schema: Vec<FieldRuleDto>` | ✅ present |
| `DROP TABLE` | `ddl::drop_table` | `DropTableOp` — `hmac` | ✅ present |
| `CREATE INDEX` | `ddl::create_index` | `CreateIndexOp` — `unique`, `sorted`, `index_type ∈ {btree, fts, functional, vector}`, `fts_tokenizer`, `fts_language`, `functional_op`, `functional_args`, `vector_dim`, `vector_metric`, covering `include[]`, `if_not_exists` | ✅ present (rich) |
| `DROP INDEX` | `ddl::drop_index` | `DropIndexOp` — `hmac` | ✅ present |

**Idempotency modifiers** exist on `CreateDb` / `CreateRepo` / `CreateTable` /
`CreateIndex` (`if_not_exists`). `CreateFunction` / `CreateValidator` use
`replace` (or-replace semantics). Drops have **no** `if_exists` and rely on
`cascade` only at db/repo level — see gap G2.

**Destructive-op HMAC.** `DropDb` / `DropRepo` / `DropTable` / `DropIndex` /
`DropUser` / `DropRole` / `Start/Commit/RollbackMigration` all carry an optional
`hmac: Option<String>` field (hex HMAC-SHA256 over a `\0`-joined canonical
string) — a deliberate "are you sure" second factor beyond the access gate.

### 1.2 Schema definition + declarative validators

The schema surface is **validator-based**, not constraint-enforced. A table
carries a list of `FieldRuleDto` (`schema_ops.rs`); the engine compiles these
into a `SchemaValidator` (`shamir-engine/src/validator/schema/schema_validator.rs`)
which is a `RecordValidator` run on the write path.

**Per-field constraints supported** (`ConstraintsDto`, `schema_ops.rs:46-94`;
engine mirror `Constraints`, `constraints.rs:34-83`):

| Constraint class | Wire field | Enforcement | Phase |
|---|---|---|---|
| type tag (`string/int/f64/dec/bool/bin/list/map/any`) | `type` | pure check | A |
| `required`, `nullable`, `unsigned` | same-named | pure check | A |
| `min` / `max` (int or f64 via `NumDto`) | same | pure check | A |
| `len` (exact), `max_len`, `min_len` | same | pure check | A |
| `one_of` (enum) | `one_of: Vec<QueryValue>` | pure check | A |
| `array_of` (element-type tag) | same | pure check | A |
| **scalar predicate** (named funclib/user scalar) | `scalar` | invokes registered scalar as `Bool` predicate | B |
| **format** (`email`/`url`/`uuid`/`date`) | `format` | in-process regex/format predicate | B |
| **cross-field compare** (`< <= == != >= >`) against another path | `compare { other, op }` | pure check | B |
| **foreign-key** (forward-only existence) | `foreign_key { ref_table, ref_field }` | async DB read via `ValidatorDb::exists_in`; **only in tx-mode** (`ctx.db() == Some`); silently skipped under autocommit | C2 |
| **unique** (no duplicate in committed + staged) | `unique: bool` | async DB read via `exists_in_self`; **only in tx-mode**; NULL bypasses (SQL semantics) | C3 |

**DDL operations on schema** (`schema.rs`):
`set_table_schema` (whole-replace, optimistic `expected_version`),
`add_schema_rule` (upsert by path),
`remove_schema_rule` (by path),
`get_table_schema` (introspection).

**Fluent builder** `field([...])` (`schema.rs:20`) exposes all of the above
plus `.unique()` / `.foreign_key(...)`.

**Key honesty notes:**
- FK + unique fail-open «под autocommit» — ✅ снято как ложная тревога (server
  оборачивает каждый батч в tx → enforced; сырой engine implicit-путь —
  defense-in-depth, не дыра данных). См. `DONE.md`.
- FK actions (`ON DELETE`: RESTRICT/CASCADE/SET NULL) — ✅ реализованы (Phase D,
  `DONE.md`). Остаётся `ON UPDATE`. Билдер: `.foreign_key(...)` +
  `.foreign_key_on_delete(...)`.

### 1.3 WASM functions + function folders

| Op | Builder | DTO |
|---|---|---|
| `create_function` (Rust source → WASM, or precompiled WASM b64) | `ddl::create_function` | `CreateFunctionOp` — `source` xor `wasm`, `replace` |
| `drop_function` | `ddl::drop_function` | `DropFunctionOp` |
| `rename_function` | `ddl::rename_function` | `RenameFunctionOp` |
| `create_function_folder` (path segments) | `ddl::create_function_folder` | `CreateFunctionFolderOp` |

Folders are securable containers (`ResourceRef::FunctionFolder`,
`access.rs:38`); the access gate's ancestor-`Execute` traversal gives them
implicit path permissions for free (per `DDL.md` §4). Note: there is **no**
`drop_function_folder` / `rename_function_folder` / `move_function` — see G4.

### 1.4 Validators (BEFORE-write WASM hooks)

Full lifecycle over the wire (`validator.rs`, `validator_ops.rs`):
`create_validator` / `drop_validator` / `rename_validator` /
`bind_validator { table, ops, priority }` / `unbind_validator` /
`list_validators(table)` (per-table bindings) + `List::Validators` (global
catalogue). Bindings carry `ops: Vec<WriteOp>` and `priority ∈ [1000,9999]`.

**Enforcement** is real and on the hot path: `write_exec.rs` calls
`run_validators_qv` per record before staging (line ~171), with skip-if-none-bound
fast path. `DropValidator` refuses if `bound_in` is non-empty (referential guard).
This is the WASM-native equivalent of CHECK + BEFORE INSERT/UPDATE triggers
(`VALIDATORS.md`).

### 1.5 Access control — two parallel models

**Model A — POSIX DAC** (`access_control.rs`, `access.rs`):
- `chmod(resource, mode)`, `chown(resource, owner)`, `chgrp(resource, group)`
  on any `ResourceRef` (Database / Store / Table / Function / FunctionFolder /
  FunctionNamespace).
- Groups: `create_group`, `drop_group`, `add_group_member`, `remove_group_member`.
- `access_tree()` introspection (read-only, requires `Manage` on root).
- Backed by `shamir-types::access::{ResourcePath, ResourceMeta, Action}`
  with `Action ∈ {Read, Write, Create, Delete, Execute, List, Manage}` and a
  POSIX 12-bit mode + setuid bit; enforced by the facade gate's ancestor-
  `Execute` traversal + `permits(actor, meta, action, in_group)`.

**Model B — RBAC** (`auth.rs`, `shamir-query-types::auth`):
- `create_user(name, password, roles?, profile?, database?)` — password is
  hashed to **Argon2id PHC** at rest (`User.password_hash`, `types.rs:148-150`),
  wrapped in `SecretString` (zeroized on drop). The brief said "SCRAM"; the
  actual at-rest scheme is Argon2id — a strictly stronger KDF than SCRAM's
  PBKDF2 default. Wire-level authentication **does** use a SCRAM-style
  challenge/response handshake (client `protocol.ts` / `scram.ts`: 4-message
  SCRAM-SHA-256 with Argon2id as the SASLprep password-stretching step);
  Argon2id here replaces PBKDF2 as the salted KDF inside that handshake, it is
  not the absence of a challenge/response protocol.
- `drop_user`, `create_role(name, permissions)`, `drop_role`,
  `grant_role(role, user)`, `revoke_role(role, user)`.
- `Permission { effect: Allow|Deny, actions: Vec<Action>, resource: Resource,
  row_filter: Option<Filter> }` — RBAC with row-level filters (RLS precursor).
- `Action ∈ {Read, Insert, Update, Delete, Create, Drop, Alter, Write,
  ManageUsers, ManageRoles, All}` — finer-grained than Model A's 7 actions.

The two models coexist; Model A is the live "facade gate" and Model B is the
declared RBAC layer. `DDL.md` flags that owner-on-create and default-mode
tightening (open `0o777` → enforced) are still in progress — see G10.

### 1.6 Migrations (online engine change)

`migration.rs` / `migration_ops.rs`: `start_migration(table → dst_repo +
dst_engine + dst_path?)`, `commit_migration(id)`, `rollback_migration(id)`,
`migration_status(id)`. This is **storage-engine migration** (move a table to a
different engine/repo online, with shadow log + cutover —
`shamir-engine/src/migration/`), NOT schema migration. All four ops carry `hmac`.

### 1.7 Temporal / retention

`retention.rs` / `retention.rs` DTO: `Retention { max_age_secs?, max_count?,
min_count? }` with `validate()` (min ≤ max). DDL: `set_retention(table, ...)`
(live swap via `ArcSwap`), `purge_history(table, scope)` (imperative
`OlderThan{timestamp}` / `OlderThanAge{age_secs}`), `changes_since(from)`
(durable-journal tail read with optional `limit`). Retention is also settable
inline at `create_table` time.

### 1.8 Tunables — buffer config

`buffer_config.rs` / `buffer_config.rs` DTO: `BufferConfigDto { max_bytes,
max_entries, ttl_ms?, flush_interval_ms, flush_batch_size }`. DDL:
`set_buffer_config` (full replace), `get_buffer_config` (read),
`alter_buffer_config(table, BufferConfigPatch)` — partial update with
double-option `ttl_ms` semantics (absent / null / value).

### 1.9 Interner management

`interner.rs` / `interner_ops.rs`: `interner_dump(repo, since?)` (full or
delta dictionary refresh), `interner_touch(repo, names[])` (idempotent name →
id registration). `interner_resolve.rs` exposes a resolve helper. This is a
field-name interning control surface unique to ShamirDB's storage model.

### 1.10 Introspection / list

`list.rs` / `list_ops.rs`: `list_databases`, `list_repos`, `list_tables(repo)`,
`list_indexes(table)`, `list_users`, `list_roles`, `list_functions(folder?)`,
`list_all_validators()`, `list_function_folders(parent?)`. Plus the per-table
`list_validators(table)` and `get_table_schema(table)` and `access_tree()` and
`get_buffer_config(table)`. **Missing:** `describe_table` / `describe_index`
(structural detail, not just names) — see G5.

---

## 2. What's MISSING or weak — vs. mature DBMS DDL

### Truly absent

- **G4.** No `ALTER FUNCTION` body swap in-place (only `replace` flag on create
  — close, but no `ALTER FUNCTION ... COMPILE`, no `SET SCHEMA`, no `OWNER TO`).
- **G5.** No `DESCRIBE` / `SHOW CREATE` — `list_*` returns names only; there is
  no single op that returns a table's full DDL (columns/schema + indexes +
  validators + retention + buffer config + owner/mode) in one round-trip.
- **G6.** No `RENAME` for db / repo / table / index / column / role / group /
  folder. Only functions and validators can be renamed. Rename is the cheapest
  non-destructive evolution op and is uniformly missing.
- **G9.** No `DEFAULT` value semantics. A field can be `required` (reject if
  absent) but the engine will not synthesize a value (literal or computed) for
  an absent field on insert. The validator design explicitly defers
  "transform / mutating validators" that could stamp defaults server-side
  (`VALIDATORS.md` "Future extensions"). Today defaults must be client-side.
- **G11.** No sequences / auto-increment / `SERIAL` / `IDENTITY`. `RecordId` is
  a `[u8;16]` catalogue id; there is no monotonically increasing per-table
  counter generator. App-side only.
- **G12.** No views, materialized views, or any query-name reuse layer.
- **G13.** No triggers in the SQL sense. Validators are the closest analogue
  (BEFORE INSERT/UPDATE/UPSERT/DELETE), but they are pure/read-only by design
  (`VALIDATORS.md` MVP scope) — no AFTER hooks, no side-effecting triggers, no
  event-out / audit-append hooks (the latter is called out as future work).
- **G14.** No `CHECK` as a first-class constraint keyword — but functionally
  covered by validators and by `one_of` / `min` / `max` / `compare` /
  `scalar` / `format` schema rules. Marking absent only in the *keyword*
  sense; the *capability* is present and arguably richer (WASM validators).
- **G16.** No table partitioning / sharding DDL. Repositories provide a
  manual partition-like axis (a table lives in a repo), but there is no
  declarative `PARTITION BY ...` and no automatic partition pruning.
- **G17.** No quotas / per-user / per-db resource limits (rows, bytes, qps,
  concurrent conns). Buffer config caps a *table's* write buffer; nothing caps
  a *user's* or *database's* footprint.
- **G18.** No `COMMENT ON` / first-class metadata annotations. `User.profile`
  is a freeform blob; tables / columns / indexes carry no description /
  tags / key-value metadata.
- **G19.** No `CREATE SCHEMA` (SQL namespace). The closest concept is "repo",
  but a repo is a storage container, not a name-resolution namespace.
- **G20.** No multi-statement DDL script / migration framework (like
  `flyway` / `refinery` / `alembic`). The `start/commit/rollback_migration`
  ops are for *storage-engine* migration, not for applying a sequence of
  schema DDL changes with version tracking. Schema does carry `schema_version`
  + `expected_version` for optimistic concurrency, but there is no
  "migration N → N+1 changelog" abstraction.

### Partial / weak

- **G1.** `ALTER TABLE` is intentionally out of scope for *column schema*
  (the store is schemaless: MessagePack + interned fields, so add/drop/rename
  column is a no-op at the storage layer). However `ALTER TABLE` for
  *accessories* (add/drop/rename **index**, **bind/unbind validator**,
  **set retention**, **set buffer config**) is expressible — just not under a
  single `ALTER TABLE` keyword; each is its own op. The `DDL.md` design doc
  explicitly endorses this ("alter means indexes / buffer config / validators
  / access"). **Verdict: intentionally out of scope (column DDL); present
  (split across ops) for accessory DDL.**
- **G2.** Idempotency is **incomplete**: `if_not_exists` exists on creates,
  but `if_exists` does **not** exist on any drop. `cascade` exists only on
  `drop_db` / `drop_repo`. Dropping a table with indexes / bound validators /
  FK references is not guarded by a `cascade` / `restrict` toggle at the table
  level (only `DropValidator` refuses-if-bound).
- **G3.** Referential integrity on **drop** — ✅ **частично закрыто** (см.
  `DONE.md`). `DropValidator` refuses if `bound_in` non-empty; `DropTable`
  теперь refuses под живым FK (`drop_refused_fk`, Phase D.3). **Остаток:**
  `DropFunction` не refuses, если функция привязана как валидатор.
- **G7.** FK actions (`ON DELETE`) — ✅ **DONE** как Phase D (см. `DONE.md`):
  `RESTRICT` / `CASCADE` / `SET NULL` + `NoAction`-дефолт. `ON UPDATE` — вне
  текущего скоупа (остаток).
- **G8.** `NOT NULL` semantics: covered via `required` + `nullable` flags in
  the schema validator (pure check), but only enforced when a schema rule is
  declared. There is no engine-level "every row must have `_id`" invariant
  beyond the storage key. Practically fine, architecturally validator-based
  rather than constraint-enforced.
- **G10.** Access-control **defaults are open** (`owner = System`, `mode =
  0o777`) and the gate is "not yet uniformly invoked on every admin path"
  (`DDL.md` §0, §3). owner-on-create and the open→enforced transition are
  explicitly unfinished. This is the single biggest production-hardening gap.
- **G15.** Constraint storage is validator-based, not catalog-enforced. A
  `unique` schema rule requires an index to exist at DDL time (fail-closed
  per the `unique()` builder doc), but the unique check itself runs through
  the validator pass (`schema_validator.rs:114-165`), not through the index
  insert path. The index's own `unique=true` flag (`CreateIndexOp.unique`) is
  a *separate*, index-level enforcement. Two paths to uniqueness is a
  coherence risk.

### Intentionally out of scope (per docs)

- Columnar `ALTER TABLE ADD/DROP/RENAME COLUMN` — schemaless store, `DDL.md` §1.
- SQL `CHECK` keyword — superseded by validators, `VALIDATORS.md`.
- Mutating / AFTER triggers — explicitly future work, `VALIDATORS.md` "Future
  extensions".

---

## 3. Prioritized gap list

### HIGH (correctness / production-blocking)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| **G10** | Access defaults open (`0o777`, owner=System); gate not uniform | Every resource world-readable/-writable until manually tightened; the entire RBAC/DAC edifice is opt-in | **Security** — ship blocker for any multi-tenant deployment |
| ~~G7~~ | ~~FK has no `ON DELETE` actions~~ | ✅ **DONE** — Phase D (RESTRICT/CASCADE/SET NULL); `ON UPDATE` остаётся | см. `DONE.md` |
| **G2** | Drops lack `if_exists`; no `cascade` at table level | Scripts can't be idempotent; can't clean up a table with indexes/validators in one op | **Operability** — every migration/CI script fragile |
| **G3** (остаток) | Drop refuses for validators + FK targets (✅); `DropFunction`-as-validator ещё нет | Drop a function-used-as-validator → silent dangling reference | **Referential integrity** |
| ~~—~~ | ~~FK + unique fail-open under autocommit~~ | ✅ снято — ложная тревога (enforced через серверную tx-обёртку), см. `DONE.md` | — |

### MEDIUM (usability / parity)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| **G6** | No `RENAME` for db/repo/table/index/column/role/group/folder | Rename is the cheapest non-destructive evolution; its absence forces dump/recreate | **Schema evolution** friction |
| **G9** | No `DEFAULT` (literal or computed) | Every insert must supply every required field; no server-side `created_at` stamping | **Usability** — app-side boilerplate |
| **G11** | No sequences / auto-increment | No server-side surrogate key generator; app must produce its own | **Usability** — common DBMS expectation |
| **G5** | No `DESCRIBE` / `SHOW CREATE` | No single op returns a table's full shape (schema+indexes+validators+meta) | **Introspection** — tooling/SDK friction |
| **G15** | Two uniqueness paths (schema-rule `unique` vs index `unique=true`) | Coherence risk: rule without index fails closed; index without rule silently enforces differently | **Correctness** — needs reconciliation |
| **G17** | No quotas / per-user / per-db limits | No way to cap a tenant's footprint | **Multi-tenancy** — fair-share / noisy-neighbour |
| **G20** | No schema-version migration framework | `schema_version` exists but no changelog/apply abstraction | **Operability** — teams can't version-control their schema |

### LOW (advanced / niche)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| **G12** | No views / materialized views | Query reuse / derivation layer absent | **App architecture** — not a correctness issue |
| **G13** | No AFTER / side-effecting triggers | Validators are pure BEFORE-hooks only (by design) | **Event pipeline** — future work per docs |
| **G16** | No declarative partitioning | Repos are manual partitions; no `PARTITION BY` | **Scale** — niche until very large datasets |
| **G18** | No `COMMENT ON` / metadata annotations | No first-class docs/tags on objects | **Discoverability** — cosmetic |
| **G19** | No `CREATE SCHEMA` namespace | Repos cover the storage axis; SQL-style namespaces absent | **SQL-compat** — only matters for SQL facade |
| **G4** | No `ALTER FUNCTION ... COMPILE/OWNER TO` | `create_function(replace)` covers body swap | **Ergonomics** — minor |
| **G8** | `NOT NULL` is validator-based not catalog-enforced | Functionally present via `required`/`nullable`; architecturally weaker | **Theoretical** — fine in practice |

---

## 4. Summary

ShamirDB's DDL is **broad where mature DBMS DDL is broad** (lifecycle, indexes,
auth, retention, tuning) and **deliberately narrow where schemaless stores make
column DDL meaningless** (no `ALTER TABLE ADD COLUMN`). Its distinctive strength
is the **validator model**: declarative schema rules (scalar/format/cross-field/
FK/unique) **plus** arbitrary WASM `BEFORE`-write hooks, all enforced on the
write path — a genuinely richer integrity surface than SQL `CHECK`. Its
historical weakness was **referential lifecycle** — теперь во многом закрыта:
FK `ON DELETE` (RESTRICT/CASCADE/SET NULL) и drop-guard на `DropTable`
реализованы (Phase D, `DONE.md`); FK/unique enforced под autocommit (ложная
тревога снята). Остатки — `DropFunction`-guard и `ON UPDATE`. The single biggest
ship blocker остаётся **open access defaults** (G10) — every other DDL feature
is moot if the gate isn't uniformly enforced.

**Counts:** 10 present feature groups (§1.1–§1.10) covering ~45 wire `BatchOp`
variants; 20 prioritized gaps (5 HIGH, 7 MEDIUM, 8 LOW).
