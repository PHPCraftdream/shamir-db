בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Репликация — пользовательская поверхность (DDL / билдеры / тесты)

> **Дата:** 2026-07-03.
> **Контекст:** [`REPLICATION.md`](REPLICATION.md) §5.5 (publication/
> subscription-модель, ReplicationProfile-шаблоны, node accounts),
> [`../research/REPLICATION-PRE-REFACTOR-2026-06-30.md`](../research/REPLICATION-PRE-REFACTOR-2026-06-30.md)
> (внутренний pull-протокол PR0–PR5).
> **Назначение:** план-документ, который PR0–PR5 НЕ покрывал — они про
> внутренний wire-протокол (`ReplHello`/`ReplPull`) и pre-refactoring.
> Здесь — ПОЛЬЗОВАТЕЛЬСКАЯ поверхность: как оператор объявляет репликацию
> декларативно, через тот же DDL/builder-конвейер, что и остальные DDL.

---

## 1. Принцип — репликация конфигурируется как обычный DDL

По §5.5 REPLICATION.md репликация настраивается декларативно:
publication/subscription + именованные `ReplicationProfile`-шаблоны,
применяемые к node accounts. Это ложится в существующий DDL-конвейер
ровно как всякий другой admin-op — **никакого отдельного механизма**:

- `crates/shamir-query-types/src/admin/*` — тип op'а (по файлу на op);
- `BatchOp` вариант + `is_admin()`/`is_write()` классификация (PR1 уже
  завёл `is_write` — новые репл-op'ы обязаны быть в нём **write=true**);
- `crates/shamir-query-builder/src/ddl/*.rs` — Rust-билдер (файл на op);
- `crates/shamir-client-ts/src/core/builders/*.ts` — TS-билдер;
- юнит-тесты по обе стороны + wire-e2e + leader/follower-e2e.

Запись такой конфигурации проходит tx-commit-путь ⇒ сама реплицируется
на follower'ы через system-changefeed (V1a, PR0 подтвердил).

## 2. DDL-поверхность (набросок — финализируется при R1)

Именование — предмет уточнения на R1; ниже — форма, не финальный контракт.

| Операция | Смысл |
|---|---|
| `create_replication_profile` | именованный шаблон: `(scope, direction, mode)` streams — что реплицировать, в какую сторону (push/pull/both), в каком режиме (ro/rw) |
| `drop_replication_profile` | удалить шаблон |
| `create_publication` | что лидер отдаёт: набор `(db, repo[, table])` scope'ов |
| `drop_publication` | — |
| `create_subscription` | подписка follower'а на publication удалённого узла + привязанный профиль |
| `drop_subscription` | — |
| `alter_subscription` | пауза/резюм/смена профиля |
| node-account роль | `create_user ... roles(["replicator"])` — уже выразимо (PR2 `has_role`), отдельный op НЕ нужен |
| introspection (read-only) | `list_publications` / `list_subscriptions` / `replication_status` — как `list`/`describe_table`, `is_write=false` |

`system/*` (users, roles, settings) реплицируется включением его scope в
publication — не отдельная DDL-сущность (§5.5 инсайт).

## 3. Разбиение на таски

Порядок исполнения — сверху вниз; `[R0]`/`[R1]` = зависит от фазы.

1. **Репл-DDL ops в `shamir-query-types`** (таск #372) — admin-op типы +
   `BatchOp` варианты + `is_admin`/`is_write` (обязательно write=true для
   мутирующих, false для `list_*`/`*_status`) + serde round-trip юнит-тесты.
   Зависит от R0-решения PR5 (`DbRequest::Repl` vs top-level) — согласовать
   имена до генерации. [R1]
2. **Rust query builder** (таск #373) — `ddl/replication.rs` (или per-op
   файлы по конвенции: один primary export на файл) + юнит-тесты в
   `ddl/tests/`. Билдер — единственный санкционированный путь конструирования
   (CLAUDE.md «builder only»). Блок: #372.
3. **TS client builder** (таск #374) — `builders/replication.ts` +
   `builders/__tests__/replication.test.ts` (vitest) + экспорт из
   `builders/index.ts` + типы в `core/types`. Wire-паритет с Rust-билдером
   (сверять msgpack-форму). Блок: #372.
4. **Rust интеграционные тесты** (таск #375) — (а) `ddl_wire_e2e`-стиль
   round-trip репл-DDL через `ShamirDbHandler`; (б) leader+follower e2e по
   шаблону `tests/mvp_e2e.rs` (TLS+SCRAM стек): создать publication на
   лидере, subscription на follower'е, записать на лидере → проверить
   применение на follower'е; проверить read-only гейт (PR4 NodeMode) на
   follower'е. Блок: #373, R1 (`apply_replicated`).
5. **TS интеграционные тесты** (таск #376) — vitest против живого сервера
   (по образцу существующих e2e в `shamir-client-ts/src/__tests__`):
   объявить репликацию через TS-билдер, проверить round-trip и введённые
   ошибки (`read_only_replica`). Блок: #374, R1.

## 4. Тест-матрица (Definition of Done фичи)

- **Rust unit:** serde round-trip каждого op'а; `is_write`/`is_admin`
  классификация; билдер → корректная wire-форма.
- **TS unit:** каждый билдер → ожидаемый msgpack; типы компилируются.
- **Rust integration:** wire-e2e round-trip; leader→follower применение;
  read-only гейт; идемпотентность повторного apply (watermark, V4).
- **TS integration:** end-to-end против сервера; коды ошибок.
- **Кросс-язык:** Rust-билдер и TS-билдер дают БАЙТ-идентичный msgpack
  для одного логического op'а (добавить сверочный фикстур-тест).

_Заметка: OQL-поверхность репликации ведётся отдельным треком (вне этого
документа). Если потребуется — завести парную таску на OQL-парсер/грамматику
для репл-DDL._
