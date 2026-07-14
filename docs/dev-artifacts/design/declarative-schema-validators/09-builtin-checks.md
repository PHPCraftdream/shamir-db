בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Каталог встроенных проверок (типы, функции, внешние ключи)

## Прозрение

Встроенные проверки — **не новые примитивы**, а декларативные имена над тем, что движок УЖЕ
умеет:
- **funclib-скаляры** (`arrays`, `strings`, `compare`, `datetime`, `math`, `cast`) — значения;
- **`$in @ref` semi-join** (тот, что мы оптимизировали O(N²)→O(N) в перф-кампании:
  `resolve_query_ref_column` + материализация в `TSet`) — **foreign key**;
- **индексы** (functional/secondary) — `unique` и быстрый FK.

Declarative-схема — тонкий по-полевой table-owned **фасад** над scalar+index плоскостями.

## Две оси проверок

```
ЧИСТЫЕ (in-process, без БД)          ┆   DB-TOUCHING (валидатор трогает БД)
type, numeric, string, collection,   ┆   foreign_key (semi-join + индекс)
const/one_of, cross-field            ┆   unique (table-level, уникальный индекс)
```

Чистые `SchemaValidator` считает по `RecordFields` (`08-/01-…`). DB-touching требуют, чтобы
`ValidatorCtx` нёс **db-handle** (см. ниже) и был **индекс** на ссылаемом поле.

## Каталог

**1. Тип/форма** (поверх `TypeTag`): `string/int/f64/dec/bool/bin/list/map/set`; `list`
(= «поле массив») + `array_of: <тип>` (типизированный массив); `non_empty`.

**2. Числовые** (funclib `math`/`compare`): `min`, `max`, `range`, `multiple_of`,
`unsigned` (Int≥0).

**3. Строковые** (funclib `strings`/`datetime`): `min_len`/`max_len`/`len`; `pattern` (regex —
уже есть в фильтрах); `format`: `email`/`url`/`uuid`/`date`/`ip` (через готовые скаляры);
`one_of`/`enum`; `const`.

**4. Коллекции** (funclib `arrays`): `min_items`/`max_items`/`len`, `unique_items` (без дублей в
массиве), `element_type`.

**5. Значение/наличие**: `required`, `nullable`, `default`, `const`, `one_of`.

**6. Кросс-полевые** (та же запись, funclib `compare`): `field A < field B` (напр. `start <
end`), условный `required` (если X задан → Y обязателен), взаимное исключение.

**7. Реляционные** (главное, Phase C):
- **`foreign_key`** — значение нашего пути обязано существовать как значение пути `P` в таблице
  `T`. Это **ровно `our_field $in (SELECT P FROM T)`** — semi-join. С **индексом на `T.P`** —
  O(log N) на проверку.
- **`unique`** (table-level) — нет двух строк с одинаковым значением пути → **уникальный индекс**.

**8. Escape-hatch** (Phase B): любой зарегистрированный скаляр (built-in funclib ИЛИ user) как
предикат — `[["email"], "@valid_email"]`. Покрывает всё, чего нет в built-in.

## Foreign key — детально (продумано)

Форма правила в каталоге (DTO, не конструирование запроса — строится билдером `02-/06-…`):
```
{ "path": ["author_id"], "foreign_key": { "table": "users", "field": ["_id"], "repo": "main" } }
```

> **Phase C — АСПИРАЦИОННА: требует НЕПОСТРОЕННЫХ примитивов** (честно). Ниже — что есть и чего
> нет.

1. **Только прямая проверка на write** (наше значение должно найтись). *Referential actions*
   (cascade/restrict/set-null при удалении ссылаемой строки) — отдельная фича (триггеры/каскады),
   НЕ часть field-валидации, Phase D+.
2. **Индекс обязателен.** FK без индекса на `T.field` = O(N)-скан на вставку → declarative FK
   **требует индекс** на ссылаемом поле (или авто-создаёт, как реляц. СУБД). Связывает с
   index-слоем.
3. **`$in @ref` даёт лишь СТРУКТУРУ мембершипа, не механизм.** В фильтрах `resolve_query_ref_column`
   материализует `TSet` из `ctx.resolved_refs` — это **пред-выбранный результат ПРЕДЫДУЩЕЙ
   batch-Read-операции**, НЕ живой DB-lookup. На write-path у валидатора нет пред-резолвнутой
   колонки → он обязан САМ выполнить membership-запрос к `T.field`. Переиспользуется максимум
   `TSet`-проба, а источник (живой read) — непостроен.
4. **DB-handle валидатора — НОВЫЙ примитив, его сейчас НЕТ.** Цикл строит `FnCtx` БЕЗ db-gateway
   (`table_manager_validators.rs:233`) — у валидатора доступа к БД нет вообще. Единственный
   существующий handle — `DbGateway` (`FnCtx.db`) — **autocommit-per-op, без объемлющей tx, без
   RYOW/SSI, а ре-ентрантный вызов из-внутри `execute` ДЕДЛОКнет** на batch-planner-lock
   (`db_gateway.rs:7-37`). Поэтому Phase C требует НОВОГО: (а) проброс `TxContext` в `ValidatorCtx`;
   (б) **tx-scoped read-only снапшот** текущей транзакции (НЕ `DbGateway`); (в) **ре-ентрант-
   безопасный** read-путь (НЕ через `ShamirDb::execute`); (г) индекс-lookup как источник пробы.
5. **Tx-видимость — НЕ «зафиксирована», а целевая.** Ссылаемая строка должна быть видна в снапшоте
   той же tx; это и есть смысл пункта 4(б). Сегодня этого нет — реализуется в Phase C.

`null`-семантика FK: если поле `null` (и `nullable`) — FK не проверяется (как реляц. SQL: NULL
FK допустим). `unsigned`/тип ссылаемого должны совпадать.

## Где живёт в типах

`Constraints` (`01-…`) расширяется в фазах:
```rust
pub struct Constraints {
    // Phase A (чистые): required, nullable, min, max, len, unsigned, one_of, array_of, unique_items, …
    // Phase B: pub scalar: Option<ScalarRef>,        // escape-hatch + format
    // Phase C: pub foreign_key: Option<ForeignKey>,  // { table, field, repo }  (DB-touching, индекс)
    //          pub unique: bool,                      // table-level (уникальный индекс)
}
```
`ValidatorCtx` (`08-…`): `actor`, `interner`, **`Option<DbHandle>`** (read-only, для Phase C).

## Фазировка каталога

- **Phase A** — чистые: type (+`list`/`array_of`/`non_empty`), numeric, string (len/pattern/
  one_of/const), collection (items/unique_items). In-process.
- **Phase B** — scalar-reference (escape-hatch) + `format` через скаляры + кросс-полевые.
- **Phase C** — реляционные: `foreign_key` (semi-join + индекс, DB-touching) и `unique`
  (уникальный индекс). Требуют index-поддержки + db-handle в `ValidatorCtx`.
- **Phase D+** — referential actions (cascade/restrict) — отдельно.

## Тесты

**Unit** (Phase A): каждый built-in чек (numeric/string/collection/const/one_of/array_of/
unique_items) — accept/reject + код.

**Rust e2e:**
- `foreign_key` (Phase C, durable): значение есть в `T.field` → принято; нет → отвергнуто
  (`fk_violation`); `null` FK при `nullable` → принято; проверка использует индекс (без O(N));
  tx-видимость (ссылаемая строка из той же tx).
- `unique` (Phase C): дубль значения пути → отвергнут; уникальный индекс задействован.
- scalar-reference (Phase B): `@valid_email` принимает/отвергает.

**ts/js e2e:** FK/unique/format через билдер (`field(["author_id"]).foreign_key("main","users",
["_id"])`, `field(["sku"]).unique()`, `field(["email"]).format("email")`).
