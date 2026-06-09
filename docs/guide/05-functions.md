בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 5 — Функции: WASM, the "M" (Modular)

**Когда подниматься:** логика переезжает в БД.

До этого этажа вся бизнес-логика жила в приложении — ShamirDB была
«глупым» хранилищем с фильтрами и индексами. Но чем сложнее продукт,
тем больше инвариантов хочется держать рядом с данными: валидация
записей, вычисляемые поля, инкапсуляция доступа. Этот этаж — о том,
как логика переезжает **в** базу.

## 1. Четыре вида функций

ShamirDB различает четыре макроса SDK:

|| Макрос | Сигнатура | Контекст | Для чего |
||---|---|---|---|
|| `#[shamir::function]` | `(ctx, batch, params) → Value` | полный (db, call, http, globals) | хранимая процедура с доступом к данным |
|| `#[shamir::procedure]` | `(ctx, params) → Value` | полный (db, call, http, globals) | процедура без batch-скрэтчпада |
|| `#[shamir::scalar]` | `(params) → Value` | **нет Ctx** — чистая | inline-функция в запросах (встроенный funclib) |
|| `#[shamir::validator]` | `(record, old_record, ctx) → Validation` | read-only ctx | валидация записи «до коммита» |

### `#[shamir::function]` — хранимая процедура

Пишешь обычный async Rust; макрос прячет WASM-ABI:

```rust
use shamir_sdk::prelude::*;

#[shamir_sdk::function]
pub async fn identity(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n = params.i64("n")?;
    Ok(Value::Int(n))
}
```

* `Ctx` — шлюз ко всему: `ctx.db()`, `ctx.call()`, `ctx.http_fetch()`,
  `ctx.global_get()` / `ctx.global_set()`.
* `Batch` — per-batch скрэтчпад: функции внутри одного батча обмениваются
  данными через `batch.put()` / `batch.get()`.
* `Params` — именованные параметры вызова.

<!-- TODO: verify batch scratchpad API surface matches shamir-sdk Batch docs — see examples/wasm-baseline -->

### `#[shamir::procedure]` — процедура с доступом к БД

```rust
use shamir_sdk::prelude::*;

#[shamir_sdk::procedure]
pub async fn list_all(ctx: Ctx, params: Params) -> Result<Value> {
    let table_name = params.str("table")?;
    let rows = ctx.db().table(table_name).query(None)?;
    Ok(Value::List(rows))
}
```

`procedure` = function без batch-скрэтчпада. Основной вид для «сделай
что-то с БД и верни результат».

### `#[shamir::scalar]` — чистая inline-функция

```rust
use shamir_sdk::prelude::*;

#[shamir_sdk::scalar]
pub async fn double(params: Params) -> Result<Value> {
    let n = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
```

Нет `Ctx` — нет побочных эффектов. Вызывается движком в `$fn`-выражениях
при записи и в `WHERE`-фильтрах. Встроенная библиотека (`shamir-funclib`)
содержит ~120 таких скаляров: `math/abs`, `strings/lower`, `arrays/length`
и т.д.

### `#[shamir::validator]` — защита от плохих данных

```rust
use shamir_sdk::prelude::*;

#[shamir_sdk::validator]
pub async fn require_name(record: Value, _old: Option<Value>, _ctx: Ctx) -> Validation {
    match &record {
        Value::Map(entries) => {
            let name = entries.iter().find(|(k, _)| k == "name");
            match name {
                Some((_, Value::Str(s))) if !s.is_empty() => Validation::accept(),
                Some((_, Value::Str(_))) => Validation::reject("name", "name_empty"),
                _ => Validation::reject("name", "name_required"),
            }
        }
        _ => Validation::record_error("expected_map"),
    }
}
```

Валидатор запускается **до** записи — если он вернул ошибки, insert/update
отклоняется, строка не попадает в таблицу.

## 2. Ctx — контекст выполнения

`ctx` даёт доступ к трём мирам:

|| Метод | Возвращает | Для чего |
||---|---|---|
|| `ctx.db()` | `Db` | Чтение/запись таблиц: `db.table("users").query(None)`, `.get(key)`, `.insert(doc)` |
|| `ctx.call("fn", params)` | `Value` | Вызов другой зарегистрированной функции (function-calls-function) |
|| `ctx.http_fetch(req)` | `HttpResponse` | HTTP-эгресс (GET/POST, subject to allowlist) |
|| `ctx.http_get(url)` | `HttpResponse` | Удобная обёртка для GET |
|| `ctx.http_post(url, body)` | `HttpResponse` | Удобная обёртка для POST |
|| `ctx.global_get("key")` | `Option<Value>` | Процесс-level глобальная переменная |
|| `ctx.global_set("key", v)` | `()` | Записать глобальную переменную |

`Db`-хэндл привязан к дефолтному репозиторию; `Table` предоставляет
`.get(key)`, `.query(filter)`, `.insert(doc)`.

## 3. Компиляция и деплой

### Из исходника (Rust → WASM)

ShamirDB умеет компилировать Rust-исходник в WASM на сервере:

```ts
import { ddl, Batch } from '@shamir/client';

await Batch.create('deploy')
  .add('fn', ddl.createFunction('list_users', {
    source: `use shamir_sdk::prelude::*;\n#[shamir_sdk::procedure]\npub async fn list_users(ctx: Ctx, _p: Params) -> Result<Value> {\n    let rows = ctx.db().table("users").query(None)?;\n    Ok(Value::List(rows))\n}`,
  }))
  .execute(client, 'my_app');
```

Движок scaffolds временный крейт, подставляет `shamir-sdk` по абсолютному
пути и запускает `cargo build --target wasm32-unknown-unknown --release`.
Нужен toolchain (`cargo` + target `wasm32-unknown-unknown`). Если его нет —
ошибка `ToolchainUnavailable`.

<!-- TODO: verify wasm-opt post-processing is always available or gracefully skipped — see function/compile.rs -->

### Из бинарника (pre-compiled WASM)

Собираешь локально, отправляешь base64:

```bash
cargo build --release --target wasm32-unknown-unknown -p my-function
```

```ts
await Batch.create('deploy-bin')
  .add('fn', ddl.createFunction('my_fn', { wasm: '<base64-encoded .wasm>' }))
  .execute(client, 'my_app');
```

Ответ: `results.fn.records[0].created_function === 'my_fn'`.

Toolchain на сервере **не нужен** — WASM уже скомпилирован.

### Замена (`replace: true`)

```ts
await Batch.create('replace-fn')
  .add('fn', ddl.createFunction('my_fn', { wasm: '<new-base64>', replace: true }))
  .execute(client, 'my_app');
```

Атомарная замена: старый WASM выгружается, новый регистрируется.
In-flight вызовы дорабатывают на старой версии.

## 4. Вызов: `call`

Зарегистрированная функция вызывается через `call(name, params)`:

```ts
import { call, Batch } from '@shamir/client';

const resp = await Batch.create('call-add')
  .add('sum', call('add', [3, 5]))
  .execute(client, 'my_app');

resp.results.sum.value; // { sum: 8 }
```

`value` — произвольный JSON (object, array, scalar, null), который
вернула функция. `records` — пустой массив (call не читает таблицу).

### Позиционные параметры

`params` — массив. Внутри функции доступен по ключам `"0"`, `"1"`, …
Также передаётся `"args"` — весь массив целиком.

### Зависимые вызовы (`filter.queryRef`)

`call` поддерживает `filter.queryRef`-ссылки, как и другие операции:

```ts
import { call, filter, Batch } from '@shamir/client';

const resp = await Batch.create('chained')
  .add('user', db.query('users').where(filter.eq('name', 'alice')))
  .add('enrich', call('enrich_user', [filter.queryRef('@user', '[0].id')]))
  .run();
```

Планировщик выстроит этапы: `user` → `enrich`.

## 5. Валидаторы

Валидатор — функция с особым контрактом: вызывается **до** записи в
таблицу, может отклонить операцию.

### Создание

```ts
await Batch.create('mk-val')
  .add('v', ddl.createValidator('check_age', { wasm: '<base64>' }))
  .execute(client, 'my_app');
```

Ответ: `{ "created_validator": "check_age", "id": "…" }`.

### Привязка к таблице

```ts
await Batch.create('bind')
  .add('b', ddl.bindValidator('check_age', 'users', ['Insert', 'Update'], 100, {
    db: 'mydb',
    repo: 'main',
  }))
  .execute(client, 'mydb');
```

* `ops` — на какие операции реагировать: `Insert`, `Update`, `Delete`.
* `priority` — порядок срабатывания (меньше = раньше). Валидатор с
  `stop: true` блокирует дальнейшие.

### Механика валидации

1. Запись попадает в батч (insert/update/delete).
2. Движок находит все bound-валидаторы для данной таблицы + op.
3. Вызывает их по возрастанию `priority`.
4. Если валидатор вернул ошибки — запись **отклонена**, в ответе —
  field-bound error codes.
5. Если валидатор установил `stop: true` — последующие валидаторы не
  вызываются.

Пример ответа при ошибке валидации (wire form):

```json
{
  "error": {
    "code": "validation_failed",
    "details": {
      "errors": [
        { "field": ["age"], "code": "too_young" },
        { "field": ["name"], "code": "name_required" }
      ]
    }
  }
}
```

### Отвязка

```ts
await Batch.create('unbind')
  .add('u', ddl.unbindValidator('check_age', { db: 'mydb', repo: 'main', table: 'users' }))
  .execute(client, 'mydb');
```

### Удаление валидатора

Валидатор, который привязан к таблице, удалить нельзя — сначала `unbind`.
Свободный — можно:

```ts
await Batch.create('drop-val')
  .add('d', ddl.dropValidator('check_age'))
  .execute(client, 'mydb');
```

## 6. Setuid (SECURITY DEFINER)

Функция может выполняться с правами **владельца**, а не вызывающего:

```ts
import { admin, Batch } from '@shamir/client';

await Batch.create('chmod-fn')
  .add('cm', admin.chmod(admin.refFunction('get_secrets'), 0o4750))
  .execute(client, 'mydb');
```

`0o4750` = setuid-бит + `rwxr-x---`. Когда пользователь `bob` вызывает
`get_secrets`, движок резолвит **effective actor** как владельца функции
(например, `admin`), и чтение таблицы идёт с правами admin — даже если у
bob нет прямого read на `secrets`.

Это «data firewall»: доступ к данным — **только** через процедуру.
Процедура контролирует, что именно возвращается.

| Сценарий | Как |
|---|---|
| bob вызывает setuid-функцию | effective actor = владелец функции |
| bob вызывает обычную функцию | effective actor = bob |
| Валидация внутри setuid | ctx.db() работает от имени владельца |

<!-- TODO: verify setuid bit position in mode integer — see ACCESS_FABRIC.md P5 and getter_only_e2e.rs -->

## 7. Папки функций

Функции можно организовывать в папки (аналог POSIX-директорий):

```ts
await Batch.create('mk-folder')
  .add('f', ddl.createFunctionFolder(['reports', 'daily']))
  .execute(client, 'mydb');
```

Создаёт путь `reports/daily`. Функция `reports/daily/summary` — полное
имя с папкой. На папки действуют `chmod`/`chown` (как на каталоги) —
см. [этаж 4](./04-access.md).

## 8. Встроенная библиотека (funclib)

ShamirDB поставляется с ~120 встроенными скалярами и агрегатами:

|| Категория | Примеры |
||---|---|
|| `math/` | `abs`, `ceil`, `floor`, `round`, `sqrt`, `min`, `max` |
|| `strings/` | `lower`, `upper`, `trim`, `length`, `contains`, `replace` |
|| `arrays/` | `length`, `contains`, `flatten`, `sort` |
|| `cast/` | `to_int`, `to_str`, `to_float`, `to_bool` |
|| `datetime/` | `now`, `format`, `parse` |
|| `json/` | `parse`, `stringify`, `get_path` |
|| `text/` | `slug`, `camel`, `snake` |
|| `crypto/` | `hash_sha256`, `hmac_sha256` |
|| `validate/` | `is_email`, `is_uuid` |
|| `compare/` | `gt`, `lt`, `eq`, `between` |

Используются в запросах через `$fn` (вычисляемые поля и фильтры), в
`GROUP BY` (агрегаты: `median`, `percentile`), в `SELECT` (проекции).

## 9. Жизненный цикл

```
ddl.createFunction  →  call(...)  →  ddl.renameFunction  →  call(...)  →  ddl.dropFunction
ddl.createValidator →  ddl.bindValidator → (validates inserts) → ddl.unbindValidator → ddl.dropValidator
```

* `ddl.renameFunction` / `ddl.renameValidator` — атомарное переименование.
* `ddl.listFunctions()` / `ddl.listValidators_()` — интроспекция через `access_tree`.
* Персистентность: функции и валидаторы хранятся в каталоге (catalogue),
  переживают рестарт сервера.

## Что важно знать уже сейчас (дозированно)

* **WASM — sandboxed.** Функция не может выйти за пределы хост-импортов.
  Побочные эффекты — только через `ctx`: БД, HTTP-эгресс, globals.
* **Toolchain на сервере — опционален.** Компиляция из исходника удобна
  для dev; в проде отправляй pre-compiled `.wasm`.
* **Функция — ресурс Shomer.** На неё действуют `chmod`/`chown`/`chgrp`
  (этаж 4). Setuid — это setuid-бит в mode.
* **Валидатор — fail-closed.** Если bound-валидатор не найден в registry,
  insert/update abort с ошибкой `ValidatorInvalid`.
* **`call` — в графе зависимостей.** Можно строить цепочки:
  read → call → read, с `filter.queryRef`-ссылками между этапами.

## Куда дальше

||| Упёрся в… | Поднимайся на |
||---|---|---|
|| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
|| «выкатываю в прод, нужны метрики и сервис» | [Этаж 7 — Эксплуатация](./07-operations.md) |
