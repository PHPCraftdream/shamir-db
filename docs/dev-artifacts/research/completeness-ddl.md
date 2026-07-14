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
  `DONE.md`). `ON UPDATE` — ✅ реализован в кампании ②.2 (`fk_on_update.rs`).
  Билдер: `.foreign_key(...)` + `.foreign_key_on_delete(...)` +
  `.foreign_key_on_update(...)` + `.foreign_key_with_actions(...)`.

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
`run_validators_qv` per record before staging (line ~185), with skip-if-none-bound
fast path. `DropValidator` refuses if `bound_in` is non-empty (referential guard).
This is the WASM-native equivalent of CHECK + BEFORE INSERT/UPDATE triggers
(`VALIDATORS.md`).

> **АКТУАЛИЗАЦИЯ (кампания ③.2):** валидаторный слой больше **не чисто-CHECK**.
> Добавлен **декларативный transform-проход** (`apply_transforms`, `write_exec.rs`
> ДО encode — близнец ②.4c `apply_defaults`): `TransformSpec{ComputedDefault,
> AutoNowAdd, AutoNow}` мутирует запись перед записью. Это даёт computed-`DEFAULT`
> (выражение-default через `eval_write_value`) и server-stamping `created_at`
> (`auto_now_add`, insert-only) / `updated_at` (`auto_now`, каждый write). CHECK-
> валидаторы (`run_validators_loop`) остаются чистыми и неизменными — transform
> бежит ПЕРЕД ними, так что CHECK видит уже-трансформированное значение. Replay-
> безопасность бесплатна (transforms на admission, НЕ на WAL-replay — доказано
> durable-reopen-тестом). Общий side-effecting/AFTER-trigger (G13) — всё ещё future.
> См. `DONE.md` (раздел «Кампания ③»), `CAMPAIGN-3-PLAN.md`.

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
- **G6.** ✅ **РЕШЁН ПОЛНОСТЬЮ** (Phase E.4 + E.4-followon + кампания ②.1a-d).
  `RENAME` есть для function/validator (изначально), table/index/repo (Phase E.4 +
  F.1/F.2/F.3), folder/group/role (②.1a/b/c) и **db** (②.1d — чистый каталог-rekey,
  вариант γ; предпосылка «нужен on-disk fs-move + crash-recovery» оказалась ложной:
  физ-путь repo декуплён от имени db в persisted `path`-поле, boot берёт его
  оттуда → rename = каталог-rekey без переноса файлов). `RENAME column` — N/A
  (schemaless store). Изначальный gap (uniformly missing) закрыт целиком.
- **G9.** ✅ **РЕШЁН ПОЛНОСТЬЮ** (кампания ②.4 литерал + кампания ③.2 computed +
  stamping). ②.4: литерал-`default` + штамп на INSERT (`apply_defaults`); явное
  значение (вкл. явный NULL) не перетирается; replay-safe. ③.2: `default` расширен
  до `Option<FilterValue>` → **computed-`DEFAULT`** (выражение `$fn`, вычисляется
  `eval_write_value` на insert) + **server-stamping** `created_at` (`auto_now_add`)
  / `updated_at` (`auto_now`) через общий декларативный `apply_transforms` (pre-encode
  transform-проход, близнец ②.4c). Replay-безопасность доказана durable-reopen-
  тестом. Бывшая «(A)-мини-кампания mutating-валидаторов» закрыта. См. `DONE.md`.
- **G11.** No sequences / auto-increment / `SERIAL` / `IDENTITY`. `RecordId` is
  a `[u8;16]` catalogue id; there is no monotonically increasing per-table
  counter generator. App-side only.
- **G12.** No views, materialized views, or any query-name reuse layer.
- **G13.** No triggers in the SQL sense. Validators are the closest analogue
  (BEFORE INSERT/UPDATE/UPSERT/DELETE). **С кампании ③.2 BEFORE-валидаторы умеют
  МУТИРОВАТЬ** запись (декларативный `apply_transforms`: computed-`DEFAULT` +
  `auto_now`/`auto_now_add` stamping) — это BEFORE-trigger-with-write. Всё ещё
  отсутствуют: **AFTER**-хуки, общие side-effecting триггеры (произвольный WASM,
  мутирующий чужие таблицы), event-out / audit-append — future work
  (`VALIDATORS.md`).
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
- **G2.** ✅ **ЗАКРЫТО** (Phase E.1 + E.2, см. `DONE.md`). `if_exists` есть на
  всех drop-ops (E.1); `cascade` есть на `drop_table` (E.2, в дополнение к
  `drop_db`/`drop_repo`). Дроп таблицы с индексами/валидаторами покрывается
  table-level `cascade`; referential-дроп — `restrict`-семантикой (G3/Phase D.3).
- **G3.** Referential integrity on **drop** — ✅ **ЗАКРЫТО ПОЛНОСТЬЮ** (см.
  `DONE.md`). `DropValidator` refuses if `bound_in` non-empty; `DropTable`
  refuses под живым FK (`drop_refused_fk`, Phase D.3); `DropFunction` refuses,
  если функция привязана как валидатор (`drop_refused_bound`,
  `admin_function.rs:119-137` + e2e `ddl_wire_e2e/drop_function_guard.rs`).
  Бывший «остаток» (DropFunction-as-validator) закрыт.
- **G7.** FK actions — ✅ **DONE** полностью. `ON DELETE` — Phase D
  (`RESTRICT`/`CASCADE`/`SET NULL` + `NoAction`-дефолт). `ON UPDATE` — кампания
  ②.2 (`fk_on_update.rs`: триггер «referenced value changed» → no-op gate →
  Restrict / Cascade-rekey old→new / SetNull, single-field MVP). См. `DONE.md`.
- **G8.** `NOT NULL` semantics: covered via `required` + `nullable` flags in
  the schema validator (pure check), but only enforced when a schema rule is
  declared. There is no engine-level "every row must have `_id`" invariant
  beyond the storage key. Practically fine, architecturally validator-based
  rather than constraint-enforced.
- **G10.** ✅ **ЗАКРЫТО** (Phase G.4, см. `DONE.md`). Бывший «single biggest
  production-hardening gap» снят: (G.4a) owner-on-create — все mode-bearing
  ресурсы штампуют владельца; (G.4b) единообразный `Action::Create` гейт на
  `create_db/repo/table` (снят TODO authz-gap); (G.4c, P0) дефолт сменён
  `open 0o777 → enforced 0o700` для НОВЫХ объектов (`ResourceMeta::owned_enforced`
  на всех create-сайтах db/repo/table/function/validator; legacy грузится OPEN
  через `from_record`); (G.4d) group-path e2e. Гейт `authorize_access` стоит на
  всех admin-путях (каждый handler покрыт). **Остаток:** ноль (один устаревший
  doc-комментарий `db_management.rs:15` «Mode stays 0o777» — код там зовёт
  `owned_enforced`; правится в ③.0).
- **G15.** ✅ **РЕШЁН** (кампания ②.3) — формализован как **(B) defense-in-depth**,
  не баг-дубль. Два слоя КОМПЛЕМЕНТАРНЫ: probe (`schema_validator.rs`) —
  логический fail-fast, чистая `unique_violation`, O(1) через обязательный
  индекс; index-guard (`unique_write_lock`) — физическая атомарность, HIGH-A
  race-closing. Связаны DDL-инвариантом `validate_unique_indexes` (`unique`-rule
  ⟹ unique-index, иначе `unique_requires_index`). «Coherence risk» закрыт
  нормативным two-layer контрактом в коде + coherence-тестами; probe НЕ снят
  (источник физической истины — индекс; probe = ранний отказ + диагностика).
  См. `DONE.md`, `DDL-EVOLUTION-PLAN.md §②.3`.

### Intentionally out of scope (per docs)

- Columnar `ALTER TABLE ADD/DROP/RENAME COLUMN` — schemaless store, `DDL.md` §1.
- SQL `CHECK` keyword — superseded by validators, `VALIDATORS.md`.
- Mutating **BEFORE** triggers — ✅ сделаны (кампания ③.2: декларативный
  transform-проход — computed-`DEFAULT` + server-stamping). **AFTER** /
  general side-effecting triggers — всё ещё future work, `VALIDATORS.md`
  "Future extensions".

---

## 3. Prioritized gap list

### HIGH (correctness / production-blocking)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| ~~G10~~ | ~~Access defaults open (`0o777`, owner=System); gate not uniform~~ | ✅ **DONE** — Phase G.4 (owner-on-create + uniform Create-гейт + дефолт `0o777→0o700` enforced + group-path e2e) | см. `DONE.md` |
| ~~G7~~ | ~~FK has no `ON DELETE`/`ON UPDATE` actions~~ | ✅ **DONE** — Phase D (`ON DELETE`) + кампания ②.2 (`ON UPDATE`) | см. `DONE.md` |
| ~~G2~~ | ~~Drops lack `if_exists`; no `cascade` at table level~~ | ✅ **DONE** — Phase E.1 (`if_exists` на всех drop-ops) + E.2 (`cascade` на `drop_table`) | см. `DONE.md` |
| ~~G3~~ | ~~Drop refuses for validators + FK targets; `DropFunction`-as-validator ещё нет~~ | ✅ **DONE** — `DropValidator`/`DropTable` (Phase D.3) + `DropFunction`-as-validator (`drop_refused_bound`) | см. `DONE.md` |
| ~~—~~ | ~~FK + unique fail-open under autocommit~~ | ✅ снято — ложная тревога (enforced через серверную tx-обёртку), см. `DONE.md` | — |

### MEDIUM (usability / parity)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| ~~G6~~ | ~~No `RENAME` for db/repo/table/index/column/role/group/folder~~ | ✅ **DONE ПОЛНОСТЬЮ** — table/index/repo (Phase E.4 + F.1/F.2/F.3) + folder/group/role/**db** (②.1a-d); column N/A (schemaless) | см. `DONE.md` |
| ~~G9~~ | ~~No `DEFAULT` (literal or computed)~~ | ✅ **DONE ПОЛНОСТЬЮ** — литерал (②.4) + computed-`DEFAULT` + server-stamping `created_at`/`updated_at` (③.2 transform-фреймворк) | см. `DONE.md` |
| **G11** | No sequences / auto-increment | No server-side surrogate key generator; app must produce its own | **Usability** — common DBMS expectation |
| ~~G5~~ | ~~No `DESCRIBE` / `SHOW CREATE`~~ | ✅ **DONE** — Phase E.6 (`DescribeTableOp` компонует полную форму: schema+indexes+validators+meta) | см. `DONE.md` |
| ~~G15~~ | ~~Two uniqueness paths (schema-rule `unique` vs index `unique=true`)~~ | ✅ **DONE** — кампания ②.3: формализован как defense-in-depth (probe + index-guard, связаны DDL-инвариантом `unique`-rule⟹index); нормативный контракт + coherence-тесты | см. `DONE.md` |
| **G17** | No quotas / per-user / per-db limits | No way to cap a tenant's footprint | **Multi-tenancy** — fair-share / noisy-neighbour |
| **G20** | No schema-version migration framework | `schema_version` exists but no changelog/apply abstraction | **Operability** — teams can't version-control their schema |

### LOW (advanced / niche)

| # | Gap | Rationale | Impact |
|---|---|---|---|
| **G12** | No views / materialized views | Query reuse / derivation layer absent | **App architecture** — not a correctness issue |
| **G13** | No AFTER / side-effecting triggers | BEFORE-mutating (transform) ✅ done (③.2); AFTER/side-effecting still absent | **Event pipeline** — AFTER-hooks future work |
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
historical weakness was **referential lifecycle** — теперь закрыта полностью: FK
`ON DELETE` (RESTRICT/CASCADE/SET NULL) + drop-guard на `DropTable` (Phase D),
`DropFunction`-as-validator guard (G3), FK `ON UPDATE` (кампания ②.2), FK/unique
enforced под autocommit (ложная тревога снята). Бывший «single biggest ship
blocker» — open access defaults (G10) — ✅ **закрыт** Phase G.4 (owner-on-create +
uniform gate + дефолт `0o777→0o700` enforced). **Из HIGH-приоритетов открытых не
осталось.** `RENAME db` (②.1d) и computed-`DEFAULT`/server-stamping (③.2 transform-
фреймворк) — ✅ **сделаны**; интегрити-слой теперь несёт и **mutating BEFORE-
валидаторы** (transform), не только CHECK. **Реального остатка по DDL не осталось**
— только intentionally-out-of-scope + отдельные большие кампании. Живой фронтир —
Movement C (репликация), не ширина DDL.

**Counts:** 10 present feature groups (§1.1–§1.10) covering ~45 wire `BatchOp`
variants. Из 20 каталогизированных gap'ов закрыты G2/G3/G5/G6/G7/G9/G10/G15
(кампании Phase D/E/G + ①/②/③); открытые HIGH — ноль; остаток —
intentionally-out-of-scope (column DDL, SQL CHECK, sequences G11) + отдельные
кампании (Phase H репликация; AFTER-триггеры G13).
