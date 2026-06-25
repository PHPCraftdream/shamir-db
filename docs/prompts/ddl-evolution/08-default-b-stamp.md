בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.4c — E2 DEFAULT: stamp-enforcement (insert)

Кампания **② DDL-эволюция**, этап ②.4 (E2 DEFAULT), под-этап **c — enforcement**.
Источник: `docs/research/DDL-EVOLUTION-PLAN.md` §②.4 (блок «✅ РЕШЕНО (②.4a)»).
Объём: M. Риск средний (write-пути, insert hot path). **②.4b surface уже в
дереве** (`default: Option<QueryValue>` есть в Constraints/DTO/builders). Пакет:
`shamir-engine`.

## Задача (одна строка)
На INSERT штамповать литерал-`default` в запись ДО валидации+хранения — для
каждого field-rule с `default`, ОТСУТСТВУЮЩЕГО во входящей записи; явное значение
(в т.ч. явный NULL) НЕ перетирать; заштампованное поле затем удовлетворяет
`required`.

## Решение ②.4a (инвариант — держать)
Литерал-DEFAULT replay-safe by-construction: штампуем ТОЛЬКО отсутствующее поле;
после первой записи поле present → reload/replay не пере-штампует. Никакого
mutating-фреймворка. Только INSERT (не UPDATE).

## Заземление — insert-путь (читай ПЕРВЫМ)
`crates/shamir-engine/src/table/write_exec.rs`, `execute_insert_tx`:
- **values-ветка** (основной путь, ~:136-187): на каждый `value` →
  `resolve_computed_record(value, interner)` (:137) → `query_value_to_storage_bytes_into`
  (:139, кодирует в `staged` — то, что ХРАНИТСЯ) + `resolved_values.push(resolved)`
  (:142, то, что ВАЛИДИРУЕТСЯ на :170-181). **ТОЧКА ШТАМПА — сразу после
  `resolve_computed_record`, ДО encode**: заштампуй defaults в `resolved`
  QueryValue → и stored-байты, и валидация увидят default единым местом.
- **id-keyed ветка** (`op.records_idmsgpack`, ~:206-247): raw msgpack-байты
  кладутся verbatim (lens-only, decode только если `has_validators`). Здесь
  defaults сложнее (decode→stamp→re-encode ломает lens-оптимизацию). **MVP-выбор
  (реши):** (i) штамповать и тут — но ТОЛЬКО когда таблица реально имеет defaults
  (decode-stamp-reencode под флагом `has_defaults`, иначе verbatim как сейчас);
  либо (ii) задокументировать, что id-keyed (client-pre-interned) путь
  предполагает полную запись и defaults на нём не применяются в этом MVP. Выбери
  (i), если ложится чисто; иначе (ii) с явной нотой. НЕ замедляй verbatim-путь
  когда defaults нет (флаг `has_defaults` — fast-skip).

## Доступ к defaults (зеркаль `collect_fk_refs`)
`SchemaValidator` уже отдаёт `collect_fk_refs()`
(`schema_validator.rs:39`); таблица проксирует
`table_manager_validators.rs:348`. Добавь по тому же образцу:
- `SchemaValidator::collect_defaults(&self) -> Vec<(Vec<String>, QueryValue)>`
  (пары `(field_path, default_value)` для правил с `constraints.default = Some`).
- `TableManager`-проксик (`table_manager_validators.rs`, рядом с `collect_fk_refs`):
  `pub fn schema_defaults(&self) -> Vec<(Vec<String>, QueryValue)>` —
  агрегирует по всем bound SchemaValidator-ам. (Sorted-index/registry-доступ —
  как в collect_fk_refs.)

## Stamp-хелпер
Локальная функция в write_exec.rs (или sibling), напр.:
`fn apply_defaults(rec: &mut QueryValue, defaults: &[(Vec<String>, QueryValue)])`:
- Только если `rec` — `QueryValue::Map`.
- Для каждой `(path, default)`:
  - **MVP: single-segment path** (`path.len() == 1`). Вложенные пути
    (`["address","zip"]`) — задокументируй как future, не строй (или сделай
    аккуратно, если тривиально; не усложняй).
  - **ОТСУТСТВИЕ = ключа нет в map** (`!m.contains_key(field)`). Present-as-Null
    — ЭТО явное значение, НЕ отсутствие → НЕ штамповать. (Ключевой кейс из ②.4a.)
  - Если отсутствует → `m.insert(field, default.clone())`.
- Идемпотентно: повторный вызов на уже-заштампованной записи — no-op (ключ есть).

## Интеграция
- В `execute_insert_tx` values-ветке: один раз за батч получи
  `let defaults = self.schema_defaults();` (fast-skip если пусто — НЕ трогай
  hot path когда defaults нет: `if !defaults.is_empty() { apply_defaults(&mut
  resolved, &defaults) }` ПЕРЕД encode). resolved уже `mut`? — сделай `mut`.
- id-keyed ветка по выбранному варианту (i)/(ii).
- Сверь все ПРОЧИЕ insert-под-пути в write_exec.rs (grep `run_validators_qv` —
  есть несколько; :234, :474, :856, :902): если какой-то — тоже INSERT-приём
  записи извне, штампуй и там. UPDATE-пути (:649 view, set/update) — НЕ трогай
  (default только на insert).

## Тесты (обязательно, shamir-engine; реши per-suite vs e2e по образцу
## declarative_schema_unique_e2e.rs / engine validator tests)
- **default на insert**: schema с `default` на отсутствующем поле → insert без
  поля → readback: поле = default.
- **не перетирает явное**: insert С явным значением поля → readback: явное
  значение (не default).
- **явный NULL не перетёрт**: insert с полем = NULL (явно) и nullable → readback:
  NULL (не default). ⚠ ключевой кейс.
- **required + default**: поле `required` + `default`, insert без поля → проходит
  (default удовлетворяет required), НЕ `field_required`-ошибка.
- **replay-идемпотентность**: insert с default → durable reopen (если есть
  reopen-харнес, как `unique_survives_durable_reopen`) → значение то же, не
  пере-штамповано / не задвоено.
- **нет default → как раньше**: правило без default → insert без поля →
  поведение прежнее (required → ошибка, иначе отсутствует).

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-engine -p shamir-db --full -- default`
  (+ существующие schema/insert тесты НЕ сломаны).
- `cargo fmt -p shamir-engine -p shamir-db -- --check` + `cargo clippy --workspace
  --all-targets -- -D warnings`.

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). НЕ коммить.
- ⛔ Default ТОЛЬКО на INSERT, ТОЛЬКО на отсутствующее поле, ТОЛЬКО литерал.
  Явный NULL — не отсутствие. UPDATE-пути не трогать. Computed-default не делать.
- Surgical. Fast-skip когда defaults пусто (не замедляй hot path). Импорты в шапку.
  Тесты — только через `./scripts/test.sh`, JSON-литералы многострочные с отступами.
- Заверши финальным текстом: изменённые/новые файлы (file:line) + выбор по
  id-keyed ветке (i/ii) + вывод гейта.
