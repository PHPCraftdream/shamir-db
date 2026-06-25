בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

> ✅ **ВЫПОЛНЕНО.** Кампания «Phase E — Completeness & Operability» реализована
> целиком (9 фаз E.1–E.9, commit-per-phase, запушено `5a0ad7e..40f66f3`). Факт и
> коммиты — в `DONE.md`. E.4 ограничен Object 1 (RenameTable) из-за
> архитектурного MVCC-overlay-барьера; остаток — задача #250. Документ оставлен
> как запись планирования/созерцания кампании.

# Next Campaign — выбор следующей кампании

Созерцание по итогам `docs/research/` (после выпила выполненного в `DONE.md`).
Задача: выбрать следующую кампанию — **похожую** на только что закрытую
(Phase D / keyset) и **несущую наибольшую пользу**.

---

## Две линзы (что сделало прошлую кампанию сильной)

- **Phase D** — реально недостающая *referential/lifecycle*-способность с
  ценностью корректности данных. Многофазная (D.0–D.3).
- **keyset** — *машинерия в движке уже была*, нужен был только язык-surface →
  лучший ROI («engine-ready, surface-absent»).

Прогоняю остаток `ACTION-ITEMS.md` через обе линзы.

---

## Кандидаты (ранжировано: импакт × похожесть × ROI)

### ① Phase E — безопасный / идемпотентный DDL-lifecycle  ⭐ ГЛАВНЫЙ ВЫБОР

Прямой наследник Phase D. Связка `completeness-ddl.md` G2 + G3-остаток + G6.

| Фаза | Что | Источник | Engine-ready? |
|---|---|---|---|
| **E.1** | `if_exists` на всех drop-ops (идемпотентность) | G2 | тонкий guard |
| **E.2** | table-level `cascade` на drop (таблица + её индексы/валидаторы/схема одной op) | G2 | `cascade` уже есть на db/repo — расширить |
| **E.3** | `DropFunction`-as-validator guard (закрывает referential lifecycle, начатый drop-guard'ом Phase D.3) | G3-остаток | паттерн `DropValidator`/`drop_refused_fk` готов |
| **E.4** | `RENAME` для db/repo/table/index/role/group/folder | G6 | механический (rekey + reverse-index) |

- **Похожесть на Phase D:** ★★★★★ — та же referential/lifecycle-мышца, что
  Phase D.3 (drop-guard). Естественная «Phase E».
- **ROI (engine-ready):** ★★★★☆ — cascade на db/repo есть, guard-паттерн есть,
  if_exists/RENAME тонкие.
- **Польза (operability):** ★★★★★ — «каждый migration/CI-скрипт хрупок» без
  идемпотентности + non-destructive evolution. Ежедневная боль.
- **Скоуп:** S-M на фазу. Чистая декомпозиция как у Phase D.

### ② OQL surface wins — близнецы keyset (engine-ready, surface-absent)

`completeness-oql.md` M7 + `completeness-ddl.md` G5 (+ опц. M5).

- **RETURNING-симметрия для INSERT/DELETE** (M7) — `UpdateSelect` уже есть для
  UPDATE → симметрия. Экономит read-after-write round-trips.
- **DESCRIBE / SHOW CREATE** (G5) — собрать полную форму объекта
  (schema+indexes+validators+retention+buffer+owner/mode) из уже существующих
  catalogue-reads. Нужно SDK/тулингу.
- **EXPLAIN / dry-run plan** (M5, опц.) — `QueryStats` post-hoc уже есть; превью
  плана без исполнения.
- **Похожесть на keyset:** ★★★★★ (тот же ROI-фид). **Польза:** ★★★☆☆ (DX/тулинг).

### ③ A2 — открытые access-дефолты  (абсолютный максимум пользы)

`completeness-ddl.md` G10. Открытые дефолты `0o777`/owner=System, гейт не везде.

- owner-on-create (создатель = владелец, не System) · open→enforced дефолт mode ·
  единообразный гейт на ВСЕХ admin-путях.
- **Польза:** ★★★★★ — единственный оставшийся настоящий **P0**, security
  ship-blocker. «Every other DDL feature is moot if the gate isn't enforced».
- **Похожесть:** ★★☆☆☆ — security-hardening sweep, не feature-capability.
- **Риск/скоуп:** L, трогает каждый admin-путь. Риск сломать доступ повсеместно.

### ④ C1 — интеграционное покрытие headline-фич

`coverage-ts-tests.md` P0. FTS / vector / `call` — флагманские фичи с нулевым e2e.

- createIndex(fts)+fts-query · createIndex(vector)+top-k · createFunction+call+assert.
- **Польза:** ★★★★☆ (серде-регрессия пройдёт юниты и тихо сломает фичу).
- **Похожесть:** ★★★☆☆ — тест-кампания (как наша TS-сессия), не feature.
- **Риск:** низкий.

---

## Созерцание — рекомендация

Пересечение «**похожее**» ∧ «**наибольшая польза**» чисто ложится на
**① Phase E (DDL-lifecycle)**:
- буквальное продолжение того, что строили в Phase D.3 (drop-guard);
- такая же многофазная структура (E.1–E.4);
- частично engine-ready (cascade на db/repo; guard-паттерн);
- закрывает ежедневную operability-боль (идемпотентные миграции + эволюция).

**Честный контрапункт:** если «наибольшая польза» перевешивает «похожее» — то
**③ A2** объективно ценнее (единственный P0, security), но это другая мышца,
тяжелее (L) и рискованнее.

**Если хочется ровно keyset-фид (чистый surface-ROI):** **② OQL surface wins**
(DESCRIBE — ближайший single-shot win).

---

## Развилка (ждёт решения пользователя)

| # | Кампания | Похожесть | Польза | Риск/скоуп |
|---|---|---|---|---|
| ① | **Phase E — DDL lifecycle** (if_exists+cascade+DropFn-guard+RENAME) | ★★★★★ | ★★★★★ operability | S-M/фаза |
| ② | **OQL surface wins** (RETURNING+DESCRIBE+EXPLAIN) | ★★★★★ keyset-twin | ★★★☆☆ DX | M |
| ③ | **A2 — access-дефолты** (P0 security) | ★★☆☆☆ | ★★★★★ P0 | L, риск |
| ④ | **C1 — e2e headline** (FTS/vector/call) | ★★★☆☆ | ★★★★☆ | низкий |

**Рекомендация:** ① Phase E.

---

## Решение: одна кампания «Phase E — Completeness & Operability»

Сливаем **① DDL-lifecycle + ② OQL-surface + ④ headline-e2e** в одну когерентную
многофазную кампанию (независимые, низкий риск, S-M/фаза, commit-per-stage).
**A2 (③) и E5-unify-uniqueness — отдельной кампанией следом** (другой класс
риска: A2 меняет дефолтное поведение всей системы → может замаскировать
регрессии остальных треков; нужна security-линза ревью и поэтапный rollout).

Заземлено чтением кода (file:line в описаниях тасков). Таски #241–249.

### Track A — DDL lifecycle

- **E.1 (#241) `if_exists` на всех drop-ops.** Дропы НЕпоследовательны:
  drop_db no-op-on-absent, drop_index — ошибка при отсутствии родителя. Добавить
  `if_exists` (зеркало `if_not_exists` на create) + унифицировать handlers →
  идемпотентные миграции/CI.
- **E.2 (#242, blockedBy E.1) table-level `cascade` на drop.** `cascade` есть
  только на db/repo (handle_drop_db — образец). Добавить `DropTableOp.cascade` →
  снять свои индексы/валидаторы/схему атомарно. Не обходит reverse-FK guard
  (Phase D.3) от чужих таблиц.
- **E.3 (#243) `DropFunction`-as-validator guard.** Закрывает остаток A3/G3.
  Прямой аналог Phase D.3 drop-guard + DropValidator(bound_in). `drop_refused_bound`.
- **E.4 (#244) RENAME table/repo/index** (+db/role/group/folder follow-on).
  Образец — `handle_rename_function`/`handle_rename_validator` (rekey by name,
  preserve id). Точка риска: каталог-rekey + reverse-index. Commit per-object.

### Track B — OQL surface (близнецы keyset: engine-ready, surface-absent)

- **E.5 (#245) RETURNING-симметрия INSERT/DELETE.** `UpdateSelect` есть для
  UPDATE; DeleteOp без returning, InsertOp без fields-projection. Движок при
  delete уже читает байты → returning дёшев.
- **E.6 (#246) DESCRIBE / SHOW CREATE.** Скомпоновать полную форму объекта
  (schema+indexes+validators+retention+buffer+owner/mode) из уже существующих
  reads (get_table_schema + index_manager + validator_bindings + access-meta).
- **E.7 (#247, опц.) EXPLAIN / dry-run plan.** QueryStats post-hoc есть; превью
  плана read_planner без материализации. Низкий приоритет.

### Track C — verification

- **E.8 (#248) e2e FTS / vector / call.** Подтверждено: НИ ОДИН TS e2e не создаёт
  fts/vector-индекс и не зовёт `call` через сервер. Headline-фичи с нулевым
  интеграционным покрытием (C1/P0). По e2e-кейсу на каждую через release-сервер.
- **E.9 (#249) unit Phase B/C constraints (C2) + doc-fixes F1–F5.** Дешёвая
  страховка билдер-слоя + гигиена отчётов.

**Порядок:** в основном независимы → берутся по ID (E.1→E.9), кроме E.2←E.1.
Стратегия: single-context, sequential, commit-per-phase. Старт — по слову
пользователя (таски заведены как pending).
