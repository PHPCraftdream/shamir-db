בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Кампания ③ — Завершающая досборка (тесты · mutating-валидаторы · мелочи)

> **Статус:** ✅ **РЕАЛИЗОВАНА ЦЕЛИКОМ** (③.1a/b, ③.2a/b/c/d/e, ③.3a/b, ③.h1).
> Преемник `DDL-EVOLUTION-PLAN.md` (кампания ②). Источник — пофайловая сверка
> `docs/research/` после кампаний ①/② (и Phase D/E/G). Реализация — делегирование
> агентам `sh` (prompt-first → zero-trust → коммит per-stage); дизайн-проходы
> (③.2a) и решение ③.3b — оркестратор сам. Факт (коммиты, тесты, развилки, уловы
> zero-trust) — в `DONE.md` (раздел «Кампания ③»). Запушено в `master`.
>
> **Главный итог:** ②.4 литерал-`DEFAULT` вырос в полноценный **декларативный
> transform-фреймворк** (computed-`DEFAULT` через выражения + server-stamping
> `created_at`/`updated_at`) с доказанной durable-reopen replay-безопасностью.
> Закрыта последняя «осознанно отложенная (A)-мини-кампания».
>
> **Phase H (репликация) — ОТЛОЖЕН** как отдельная большая кампания, см. §«Отложено».

---

## 0. Рамка

После кампаний ①/② read/DDL-поверхность зрелая. Пофайловый аудит `docs/research`
оставил три когерентных остатка, объединённых темой **«довести до 100%, закрыть
последний engine-хвост»**:

1. **#1 TS-клиент тест-покрытие** (`coverage-ts-tests.md`) — низкий риск.
2. **#2 Mutating/transform-валидаторы → computed-DEFAULT + server-stamping**
   (`completeness-ddl.md` G9-остаток, `VALIDATORS.md` future, `DDL-EVOLUTION-PLAN.md`
   (A)-развилка) — средний риск, единственный нетривиальный engine-кусок.
3. **#4 Мелкая builder-досборка** (`coverage-rust/ts-query-builder.md` остаток) —
   низкий риск / engine-gated.

Порядок — по возрастанию риска: ③.1 (тесты) → ③.3 (мелочи) → ③.2 (фреймворк).
③.2 — содержательный центр кампании.

---

## ③.1 — TS-клиент тест-покрытие · S-M · риск низкий

**Источник:** `coverage-ts-tests.md` §3.4, §6. Деление по actionability:

- **③.1a — unit-тесты 6 FieldBuilder Phase B/C сеттеров (server-НЕзависимо).**
  `scalar`/`oneOf`/`format`/`compare`/`foreignKey`/`unique` имеют **ноль
  unit-тестов** — покрыты только server-gated e2e (`e2e-schema-validators.test.ts`).
  При отсутствии release-бинаря — нулевое покрытие билдер-слоя. Добавить wire-shape
  unit в `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts` по
  образцу соседних (`toEqual({...})`). Берётся ПЕРВЫМ — дёшево, без сервера.
- **③.1b — e2e-добивка (server-gated, нужен release `shamir-server`).** По
  `coverage-ts-tests.md` §6: P0 — FTS, vector, `call`; P1 — `like/ilike/regex`,
  existence/containment (`isNull`/`exists`/`contains*`), `aggregateFn`, `func`
  (scalar-projection), `history`-range, `page`-mode, `distinct`; P2/P3 — `resume()`,
  `commitMigration`-success, `dropUser`/`dropRole`. Добавить e2e-кейсы в
  `e2e-ddl`/`e2e-data`/новый `e2e-call.test.ts` под `skipIf(!SERVER_AVAILABLE)`.
  ⚠ НЕ параллелить rust `--full` с e2e (Windows file-lock на `shamir-server.exe`).

**Тесты = сам объект этапа.** Гейт: `cd crates/shamir-client-ts && npx vitest run`
зелёный; при e2e — с поднятым release-сервером.

---

## ③.2 — Mutating/transform-валидаторы + computed-DEFAULT + server-stamping · M · риск средний (центр кампании)

**Источник:** `VALIDATORS.md:156-160,201-225` + `DDL-EVOLUTION-PLAN.md §②.4a`
((A)-развилка) + `completeness-ddl.md` G9-остаток.

**Ключевое упрощение (созерцание ②.4a).** Главный страх «replay must not
re-transform» **уже решён by-construction**: `VALIDATORS.md:126-131` — валидаторы
НЕ перезапускаются на WAL-replay (admission gate, не replay-step). Transform
мутирует запись на оригинальной записи ДО encode → хранится трансформированное →
replay восстанавливает байты без повторного transform. Точка интеграции — та же,
что у ②.4c DEFAULT-штампа (`write_exec.rs`, после `resolve_computed_record`, до
encode). Валидаторы сейчас ЧИСТЫЕ (`table_manager_validators.rs:291`
`run_validators_loop` — `new` по ссылке, возвращают только ошибки).

**Под-этапы:**
- **③.2a дизайн (оркестратор сам). ✅ РЕШЕНО — см. §«③.2a — boundary» ниже.**
  Контракт: ДЕКЛАРАТИВНЫЕ transform-правила (не общий mutating-трейт), агрегируемые
  через `SchemaValidator`, применяемые ДО encode в той же точке, что `apply_defaults`
  (②.4c, `write_exec.rs:152`); порядок — transform ПЕРЕД CHECK; replay-safety
  бесплатна (admission-time, вмороженo в байты).
- **③.2b framework импл** (`/crush`). Провести mutable-запись через BEFORE-trigger
  в `run_validators_loop`; применить мутацию до encode в `write_exec.rs`
  (переиспользовать точку ②.4c). Surgical, не сломать чистые CHECK-валидаторы.
- **③.2c computed-DEFAULT.** Расширить ②.4 `default: Option<QueryValue>` до
  computed (default-через-scalar / `$fn` — напр. `default` может быть выражением,
  вычисляемым на insert). Едет поверх ③.2b.
- **③.2d server-stamping.** `created_at` (insert-only) + `updated_at` (каждый
  write) как встроенные transform-стампы (schema-флаг или встроенный валидатор).
- **③.2e тесты.** replay-идемпотентность e2e (durable reopen — значение не
  пере-штамповано); порядок transform-vs-CHECK; computed-default; stamping
  insert/update.

**Риск:** write-path; порядок/детерминизм. Но replay-проблема снята → tractable.

### ③.2a — boundary (РЕШЕНО оркестратором, 2026-06-27)

**Прочитанная реальность движка (а не предположения):**
- `RecordValidator::validate(new, old, ctx) → Validation` (`record_validator.rs:145-189`)
  сейчас ЧИСТЫЙ: `new`/`old` — read-only `&dyn RecordFields`, возврат — только ошибки.
  Уже есть прецедент агрегации деклараций: `defaults()` (`:186`) собирает
  `(field_path, QueryValue)` литеральных дефолтов для ②.4c-штампа.
- **Критический порядок: encode ПРЕДШЕСТВУЕТ валидации.** `write_exec.rs:106-160`
  строит `staged`-байты из `resolved` (после `resolve_computed_record` + `apply_defaults`,
  `:146`/`:152`), и ТОЛЬКО ПОТОМ `run_validators_qv` (`:185`) бежит на read-only
  `resolved_values`. Индексы строятся из `staged` в `insert_tx_many_bytes` (`:202`).
  ⇒ чтобы мутация transform-валидатора дошла до хранилища И индексов, она обязана
  лечь в `resolved` ДО encode.
- `apply_defaults(rec.to_mut(), &defaults)` (`write_helpers.rs:161`, вызов
  `write_exec.rs:152`) — это и есть готовый pre-encode mutate-hook на `&mut QueryValue`.

**Решение (вариант «декларативный transform-проход, зеркало ②.4c»):**

1. **Декларативно, НЕ общий mutating-трейт.** Никакого `transform()` на
   `RecordValidator` (async-mutating-dyn по `&dyn RecordFields` + WASM-гость — это
   G13 AFTER-trigger, future, ВНЕ скоупа ③). Вместо этого — новый агрегатор
   `SchemaValidator::transforms() → Vec<(Vec<String>, TransformSpec)>` (близнец
   `defaults()`), и новый `apply_transforms(rec: &mut QueryValue, &transforms, ctx)`
   в `write_helpers.rs`, вызываемый в `write_exec.rs` СРАЗУ ПОСЛЕ `apply_defaults`
   (`:152`), ДО encode. Fast-skip когда список пуст (горячий путь не платит).

2. **`TransformSpec` (закрытый перечень, не open-ended):**
   - `ComputedDefault(expr)` — ③.2c: расширение `default` с литерала до выражения
     (`$fn`/scalar). Применяется ТОЛЬКО к ОТСУТСТВУЮЩЕМУ полю (тот же keystone, что
     ②.4c: `!rec.contains(field)`), вычисляется через `ctx.scalars()`/resolver.
   - `AutoNowAdd` — ③.2d `created_at`: штамп текущего времени на INSERT, только если
     поле отсутствует (insert-only).
   - `AutoNow` — ③.2d `updated_at`: штамп текущего времени на КАЖДЫЙ write
     (INSERT и UPDATE), перезаписывает.
   Время — серверные wall-clock-часы, прокинутые в ctx (НЕ `Date.now()` в скрипте).

3. **Порядок: `resolve_computed_record` → `apply_defaults` (литералы) →
   `apply_transforms` (computed-default + stamping) → encode → CHECK-валидаторы.**
   Transform ПЕРЕД CHECK: CHECK валидирует то, что реально хранится (напр. `format`
   проверяет уже-проштампованное значение). Индексируется трансформированное
   значение (берётся из `staged`) ⇒ store == index == CHECK-вход. Детерминизм есть.

4. **Replay-safety — БЕСПЛАТНА by-construction (`VALIDATORS.md:126-131`).**
   Валидаторы/трансформы бегут на admission, НЕ на WAL-replay. Transform мутирует
   `resolved` ДО encode ⇒ в WAL ложатся уже-трансформированные байты ⇒ replay
   восстанавливает байты verbatim, БЕЗ повторного штампа. `updated_at`, взятый из
   часов на момент write, вморожен в байты — replay не переписывает его новым
   значением часов. Это инвариант, подлежащий тесту ③.2e (durable reopen → значение
   стабильно).

5. **`ctx` для apply_transforms.** Нужен `ScalarResolver` (для `ComputedDefault`) и
   часы (для stamping). Resolver уже грузится в `run_validators_qv` (`:175`
   `self.scalar_resolver.load_full()`); для transform-прохода его надо поднять в
   `write_exec` ДО encode (или прокинуть в helper). Часы — добавить узкий
   clock-источник в ctx (тестируемо подменяемый; off-replay).

**Развилки, закрытые в стору совершенства:**
- *Декларативный vs общий mutating-трейт* → **декларативный** (скоуп ③ = literal +
  computed-default + created/updated_at; общий side-effecting AFTER-trigger = G13 future).
- *Мутировать в `run_validators_loop` (как гласил черновик ③.2b) vs отдельный
  pre-encode проход* → **отдельный проход у `apply_defaults`**. Причина: loop бежит
  ПОСЛЕ encode и на read-only `&dyn RecordFields` — мутация там не дошла бы до байтов.
  Черновик ③.2b («mutable-запись через BEFORE-trigger в run_validators_loop»)
  пере-нацеливается на `apply_transforms` в write-helpers. CHECK-валидаторы
  (`run_validators_loop`) остаются ЧИСТЫМИ и нетронутыми.
- *③.2c: новый тип поля vs расширение `default`* → **расширение `default`**: семантика
  «значение для отсутствующего поля» уже точна; меняется лишь литерал→выражение.

**Точки интеграции (для ③.2b–③.2d):**
- `crates/shamir-engine/src/table/write_helpers.rs` — новый `apply_transforms`
  (рядом с `apply_defaults:161`).
- `crates/shamir-engine/src/table/write_exec.rs:152` — вызов после `apply_defaults`
  (INSERT-путь); UPDATE/UPSERT — аналогичные точки (`:355`/`:774` resolve_computed_record).
- `crates/shamir-engine/src/validator/record_validator.rs` — `transforms()` агрегатор
  (близнец `defaults():186`).
- `crates/shamir-engine/src/validator/schema/schema_validator.rs` — реализация
  `transforms()` из schema-правил; surface для `auto_now`/`auto_now_add` +
  expression-`default` в `ConstraintsDto`/`Constraints` (query-types + builder, как ②.4a).

---

## ③.3 — Мелкая builder-досборка · S · риск низкий / engine-gated

**Источник:** `coverage-rust-query-builder.md` / `coverage-ts-query-builder.md`
остаток (P2-косметика).

- **③.3a `lit_u64` / `bin()` TS-хелперы.** `coverage-ts-query-builder.md` G9/G10:
  TS `FilterValue` уже включает `Uint8Array`, но нет `bin()`-конструктора; `lit_u64`
  для полного u64 (через `bigint`). Тривиальный сахар + unit wire-shape.
- **③.3b `SelectItem::Expression` / `SelectExpr` — ENGINE-GATED. ✅ РЕШЕНО →
  развилка (B) «задокументировать-отложить», билдер НЕ строим.**

  **Факт (сверено с движком, не предположение, 2026-06-27):** `SelectExpr`
  парсится (`query/read/parser.rs:190` → `SelectItem::Expression{expr, alias}`,
  `query/common/parser.rs:82` `expr_from_value`), НО НЕ исполняется в проекции:
  - `table/read_exec.rs:83` — `SelectItem::Expression { .. } => return false`
    (fast-path не умеет, отвергает);
  - `query/read/aggregate.rs:663` — `SelectItem::All | SelectItem::Expression { .. }
    => {}` (no-op, игнор в агрегат-проекции);
  - `read/select.rs:108` сам тип помечен `/// Expression (future: computed fields)`.
  Арифметика `Add/Sub/Mul/Div` определена в `read/select_expr.rs`, но evaluation
  её в read-пути НЕТ.

  **Решение (B):** билдер не строим. Билдер для невыполняемого движком типа
  породил бы запросы, которые движок молча игнорирует/отвергает — хуже, чем
  отсутствие билдера. Исполнение `SelectExpr` в проекции — самостоятельная
  engine-фича (M-объём: вычислитель в read_exec/projection), а не «мелочь ③».
  Write-path computed-нужды уже закрывает ③.2 (transform/computed-DEFAULT).
  **Отложено** до реального спроса на computed read-проекцию; при появлении —
  СНАЧАЛА engine-evaluation (read_exec), ПОТОМ билдер (Rust+TS).

> **M2/M3 (roadmap-hardening) — отдельные, НЕ «мелочь»:** M2 generated/computed
> columns (`completeness-oql.md`) пересекается с ③.2 (mutating-валидаторы — это и
> есть механизм computed-полей); M3 FTS ranking/score/highlight — самостоятельная
> engine-фича. Брать ПОСЛЕ ③.2 (M2 ляжет поверх transform-фреймворка) либо вынести
> отдельно. В кампанию ③ по умолчанию НЕ включены (engine-объём).

---

## Порядок, зависимости, стратегия

```
③.1 тесты (a server-free → b e2e)  →  ③.3 мелочи (a helpers → b SelectExpr engine-gated)  →  ③.2 mutating-валидаторы (a-design → b → c → d → e)
   низкий риск, server-free старт         низкий / engine-gated                                   средний риск, центр кампании
```
- В основном независимы. ③.1a — дешёвый server-free старт. ③.2 — содержательный
  центр; M2 (generated-columns) естественно ляжет поверх ③.2b после.
- Дизайн-проход ③.2a — оркестратор сам (прецедент ①.4/②.3a/②.4a). Импл — `/crush`.
- Стратегия: prompt-first (бриф в `docs/prompts/campaign-3/` ДО агента, запрет
  git-мутаций) → zero-trust verify (дифф + `./scripts/test.sh` + `clippy
  --workspace` + TS `vitest`/`tsc`, оркестратор сам) → коммит per-stage.

## Скоуп-границы (не раздуваем)
- ③.1b e2e — под `skipIf(!SERVER_AVAILABLE)`; не блокировать на отсутствии бинаря.
- ③.2 — литерал + computed-DEFAULT + created_at/updated_at; общий «AFTER-trigger /
  side-effecting» (`completeness-ddl.md` G13) — НЕ в скоупе (future).
- ③.3b SelectExpr — engine-gated; не строить билдер без движковой поддержки.
- OQL-ширина (JOIN/CTE/window/set-ops) — intentionally out of scope, не трогаем.

---

## Отложено (отдельная кампания, НЕ в ③)

### Phase H — Leader-Follower репликация (Movement C · «I») — ОТЛОЖЕННЫЙ ВАРИАНТ
**План готов:** `PHASE-H-PLAN.md` (H.0 `REPLICATION.md` design → H.1 `ReplicaApplier`
in-process → H.2 `ReplicateFrom`-поток → H.3 `ReplicaFollower`+e2e → H.4
bootstrap/snapshot; H.5 failover/election — отложен внутри). Объём L,
**front-loaded** (вся correctness в H.1, тестируема in-process без сети).
Рекомендация самих доков — ★★★★★ (единственный незакрытый пилон самого имени
S.H.A.M.I.R.). **Это НЕ часть кампании ③** — отдельная большая кампания,
**ждёт явного выбора направления**. Если берётся — кампания ③ может идти до или
после, либо ③ пропускается ради H.

**Почему отложено в ③:** read/DDL-досборка (③) — низко-рисковая «дочистка»;
репликация — новый пилон с распределёнными свойствами, заслуживает своей кампании
с дизайн-докой (H.0) первым. Развязаны: ③ не блокирует H, H не блокирует ③.

---

## Решение (ждёт пользователя)
Старт ③.1a по слову (дёшево, server-free). ③.2 (mutating-валидаторы) — центр;
③.2a-дизайн делает оркестратор. Phase H — отдельный выбор направления
(`PHASE-H-PLAN.md`). Таски заведутся при «делай ③».
