בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase G — Builders finish + Access enforcement (поэтапный план)

> ✅ **ВЫПОЛНЕНО ПОЛНОСТЬЮ.** Все фазы реализованы, zero-trust-верифицированы и
> закоммичены: G.1 `one_of` (`3753cbb`), G.2 `row_idmsgpack` (`f32ed0c`), G.3 e2e
> lifecycle (`09eeeed`), G.4b create-гейт (`7ef8860`), G.4c enforced-дефолт
> Strategy A + починка 51 фикстуры (`e9769b4`), G.4d group-path e2e (`356aaf0`);
> G.4a owner-on-create — уже было сделано прежними слайсами. Финальная верификация
> (оркестратор): rust `--full` 4501/4501, e2e 709/709, clippy `--workspace` чисто.
> Факт и вердикты — в `DONE.md` (раздел «Phase G»). Документ оставлен как запись
> планирования.

Кампания закрывает оставшиеся реальные дыры билдеров/тестов (**B2, B4, C3**, P1)
и единственный настоящий **P0** — открытые access-дефолты (**A2**). Источник:
`ACTION-ITEMS.md` (синхронизирован с `DONE.md` после E.4-followon).

> **Ключевой принцип последовательности:** A2 меняет дефолтное поведение всей
> системы (`open→enforced`) и потянет массовую правку существующих тестов. Чтобы
> это не маскировало регрессии и не путалось с билдер-диффами — **P1 (G.1→G.2→
> G.3) идёт ПЕРВЫМ и приземляется чисто, A2 (G.4) — ПОСЛЕДНЕЙ и изолированно**, на
> уже-зелёной базе.

> Реализацию начинать по слову пользователя. Брифы — prompt-first в
> `docs/prompts/<area>/` под git ДО старта делегированных стадий. Вся работа —
> через crush (при падении рестарт в той же сессии; agent-tool НЕ использовать).
> Каждая фаза — отдельный коммит, zero-trust-верификация оркестратором.

---

## Общие принципы (все фазы)

- **Гейт каждой фазы:** `cargo fmt -p <крейты> -- --check` + `cargo clippy
  --workspace --all-targets -D warnings` + `./scripts/test.sh` (нужные крейты,
  `--full` для integration) + **e2e TS** через `SHAMIR_SERVER_BIN` (debug-сервер,
  `cargo build -p shamir-server`; ~быстро — НЕ release).
- **Дисциплина:** builder-only (`serde_json::Value` запрещён вне исключений),
  `#[serde(default)]` на новых wire-полях, `scc::len()` запрещён, `use` в шапке,
  один файл = один export, `mod.rs` только re-exports, тесты в `tests/`-директории.
- **Тестовая лестница на фазу:** (1) Rust unit/wire-shape → (2) Rust integration
  через `db.execute` → (3) TS wire-unit (билдер) → (4) **TS e2e** через сервер.

---

## Phase G.1 — B2: Rust `FieldBuilder::one_of()`  (быстрая победа, S)

**Цель:** паритет Rust-билдера с TS — value-enum констрейнт на поле схемы.

**Заземление:**
- `crates/shamir-query-builder/src/ddl/schema.rs` — `impl FieldBuilder` (:39),
  цепочка сеттеров (образец `scalar` :174, `array_of` :165). Хранит
  `constraints`.
- Wire: `ConstraintsDto.one_of: Option<Vec<QueryValue>>`
  (`crates/shamir-query-types/src/admin/types/schema_ops.rs:67`).
- TS-образец: `ddl.ts:681` `oneOf(values: WireValue[])` → `_constraints.one_of`.

**Срез:**
- `pub fn one_of(mut self, values: impl IntoIterator<Item = impl Into<QueryValue>>)
  -> Self` в `FieldBuilder` — собирает `Vec<QueryValue>` в `constraints.one_of`.

**Тесты:**
- Rust unit (`ddl/tests` или `schema` round-trip): `field("status").string()
  .one_of(["active","archived"])` → сериализуется в `constraints.one_of` с двумя
  значениями.
- (e2e не обязателен — это билдер-паритет; wire-форма уже принимается сервером,
  enforcement покрыт валидаторным трактом. Достаточно wire-round-trip unit.)

**Объём:** S. **Коммит:** `feat(query-builder): G.1 B2 — FieldBuilder::one_of`.

---

## Phase G.2 — B4: Rust `Insert::row_idmsgpack()`  (M)

**Цель:** дать билдеру точку входа в id-keyed msgpack (v2-оптимизация записи),
сейчас недостижимую — `build()` хардкодит пусто.

**Заземление:**
- `crates/shamir-query-builder/src/write/insert.rs` — `struct Insert` (:8),
  `row()` (:42) пушит в `values`, `build()` (:71) хардкодит
  `records_idmsgpack: Vec::new()`.
- Wire: `InsertOp.records_idmsgpack` уже на DTO с `#[serde(default)]`
  (round-trip-тесты в `crates/shamir-query-types/src/write/tests/insert_op_tests.rs`).

**Срез:**
- Поле `records_idmsgpack: Vec<Bytes>` (или аналог) в `Insert`.
- `pub fn row_idmsgpack(mut self, bytes: impl Into<Bytes>) -> Self` —
  аккумулирует; `build()` прокидывает в `InsertOp.records_idmsgpack` вместо
  хардкода `Vec::new()`.
- Решить семантику смешения `values` + `records_idmsgpack` (оба пути на одном
  InsertOp): следовать тому, как сервер их трактует — свериться чтением
  `execute_insert`/v2-decode. Если взаимоисключающи — документировать.

**Тесты:**
- Rust unit: `Insert::with_repo(...).row_idmsgpack(bytes).build()` → `InsertOp`
  с непустым `records_idmsgpack`, корректный round-trip.
- Rust integration (`db.execute`): insert через id-keyed msgpack путь →
  read-back совпадает с обычным insert.

**Объём:** M. **Коммит:** `feat(query-builder): G.2 B4 — Insert::row_idmsgpack`.

---

## Phase G.3 — C3: тонкое e2e (commit-migration / dropUser-dropRole / chgrp)  (S-M)

**Цель:** закрыть e2e-пробелы headline lifecycle-операций.

**Заземление:**
- e2e-миграция сейчас гоняет только rollback-путь; `dropUser`/`dropRole`/`chgrp`
  — unit-only. Билдеры — в `crates/shamir-client-ts/src/core/builders/{ddl,admin}.ts`.
- База: `crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts` +
  `e2e-harness.ts` (образец e2e-структуры).

**Срез (только тесты, кода фич нет):**
- `e2e-migration.test.ts` (новый или расширить существующий): успешный
  **commit**-путь миграции (begin → ops → commit → проверить применённость).
- `e2e-users-roles.test.ts`: createUser → dropUser (резолв исчезает);
  createRole → dropRole.
- `chgrp` e2e: создать объект → chgrp → проверить смену group в describe/access-meta.

**Тесты:** сами тесты И ЕСТЬ дельта. e2e через debug-сервер.

**Объём:** S-M. **Коммит:** `test(e2e): G.3 C3 — commit-migration / dropUser-dropRole / chgrp`.

> ⚠️ После G.1/G.2 (новые wire-возможности билдера) — пересобрать debug-сервер
> перед e2e G.3, если они влияют на wire. Для C3 (только existing ops) пересборка
> может не понадобиться; проверить.

---

## Phase G.4 — A2: Access enforcement  (L, ПОСЛЕДНЕЙ, изолированно)

**Цель:** закрыть P0 — система не должна по умолчанию давать world-rwx; гейт
enforced единообразно; создатель = владелец.

**Заземление:**
- `crates/shamir-types/src/access.rs`: `Mode::OPEN = 0o777` (:104); default
  `ResourceMeta` — owner=System, mode=OPEN (:172/:180/:258); `owned_by(actor)`
  ставит owner=actor, **но mode остаётся 0o777** (:189).
- **Дыра гейта:** `handle_create_table` НЕ зовёт `authorize_access`
  (`admin_table_index.rs:29-31` TODO); `add_table_as` пишет `owned_by(actor)`, но
  без гейта. Свериться: `handle_create_db`/`handle_create_repo`
  (`admin_db_repo.rs`) — те же дыры?
- `authorize_access` уже зовётся в 16 admin-файлах (охват есть, но НЕ
  единообразный — пробелы на create-путях).
- Док-трек: `docs/roadmap/DDL.md` §0/§3, `docs/prompts/.../05-permissions.md §2`.

**Под-фазы (порядок ПО РИСКУ — аддитивное первым, смена дефолта последней):**

### G.4a — owner-on-create (аддитивно, низкий риск)
Каждый create-путь (table/db/repo/index/function/validator/role/group/folder)
пишет `ResourceMeta::owned_by(actor)` (создатель = владелец, не System). Где уже
пишет — подтвердить; где нет — добавить. **Поведение доступа НЕ меняется** (mode
остаётся OPEN), поэтому существующие тесты зелёные. Коммит отдельный.

### G.4b — единообразный гейт на create-путях (аддитивно)
Закрыть дыры: `handle_create_table`/`create_db`/`create_repo` зовут
`authorize_access(Action::Create)` на родителе (снять TODO `admin_table_index.rs:29`).
Пока дефолт OPEN — гейт пропускает всё, тесты зелёные. Это подготовка к G.4c.
Коммит отдельный.

### G.4c — переход дефолта `open→enforced` (СМЕНА ПОВЕДЕНИЯ, основная churn)
Сменить дефолтный mode для **новых** объектов с `0o777` на enforced (owner-rwx,
group/other — рестриктивно). Решить стратегию совместимости:
- **Strategy A (рекоменд.):** enforced дефолт только для вновь создаваемых
  объектов; legacy-объекты с явным OPEN — без изменений; флаг/конфиг для «open
  mode» в dev/тестах.
- Массовая правка фикстур: тесты, заводящие db/repo/table и полагающиеся на
  world-rwx, теперь создают акторов с нужными правами ИЛИ используют open-режим
  фикстуры. **Вся эта churn — внутри G.4c**, не размазана.
Коммит(ы): возможно разбить на «движок дефолта» + «починка фикстур».

### G.4d — e2e + негативное покрытие
e2e (расширить `e2e-permissions.test.ts`): неавторизованный актор получает
`access_denied` на create/write/admin; владелец — проходит; chgrp/chmod-эффект.
Подтвердить, что гейт enforced на ВСЕХ admin-путях (по списку 16 файлов).

**Объём:** L. **Коммиты:** per под-фаза (G.4a/b/c/d).
**Риск-митигация:** durability-критики нет, но широта максимальна — каждый
admin-путь. G.4a/b аддитивны (зелёные тесты), вся смена поведения сосредоточена в
G.4c, поэтому регрессии локализуются.

---

## Тестовая инфраструктура (e2e на всех фазах)
- Образец — `e2e-permissions.test.ts`, `e2e-rename-*.test.ts` + `e2e-harness.ts`.
- e2e через `SHAMIR_SERVER_BIN=D:\dev\rust\.cargo-target\debug\shamir-server.exe`
  (`cargo build -p shamir-server` — debug, быстро). Пересобирать ПЕРЕД e2e, если
  фаза изменила wire/серверное поведение (G.2, G.4c — да; G.1/G.3 — проверить).

## Риски и решения
| Риск | Митигация |
|---|---|
| A2 (G.4c) ломает массу тестов | Аддитивные G.4a/b первыми (зелёные); вся churn в G.4c; Strategy A (enforced только для новых объектов) |
| A2 маскирует регрессии других треков | P1 (G.1–G.3) закоммичены и верифицированы ДО старта A2 — A2 на зелёной базе |
| B4 семантика values + records_idmsgpack | Шаг чтения `execute_insert`/v2-decode; документировать взаимоисключение |
| G.4c смена дефолта — security-корректность | Негативное e2e (G.4d) обязательно: deny для не-владельца на каждом классе пути |

## Последовательность и зависимости
- **G.1 → G.2 → G.3** — P1, независимы между собой, низкий риск, берутся по порядку.
- **G.4** — ПОСЛЕ G.1–G.3 (на зелёной базе), под-фазы строго G.4a→G.4b→G.4c→G.4d.
- Рекомендация: не начинать G.4c, пока G.1–G.3 не закоммичены и gate-зелёные.

> Таски сессии: G.1 (B2), G.2 (B4), G.3 (C3), G.4 (A2 — с под-фазами в описании).
> G.4 blockedBy G.3 (чтобы A2 стартовал на зелёной базе).
