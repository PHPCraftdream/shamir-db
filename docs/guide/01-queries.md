בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 1 — Запросы: фильтры, батчи, индексы

KV-хранилище с этажа 0 — уже работает. Но данные растут: нужны выборки
по полям, диапазоны, сортировка, индексы. Этот этаж — именно об этом.

Примеры ниже — JSON, который ты отправляешь в `queries` батча (так же,
как на этаже 0). Мы продолжаем пользоваться `client.execute("default", batch)`.

## 1. Фильтры: `where`

Каждый `from`-запрос принимает необязательный `where` — фильтр в виде
JSON-объекта с полем `op`. Вот основные операции.

### Сравнения

```json
{ "from": "users",
  "where": { "op": "eq", "field": "status", "value": "active" } }
```

| `op` | Смысл | Пример значения |
|---|---|---|
| `eq` | равно | `"active"`, `42`, `true`, `null` |
| `ne` | не равно | `"inactive"` |
| `gt` | больше | `18` |
| `gte` | больше или равно | `50000` |
| `lt` | меньше | `65` |
| `lte` | меньше или равно | `100` |

Значение (`value`) — строка, число, булев или `null` — Shamir сам
приведёт тип.

### Проверка на null / существование поля

```json
{ "op": "is_null",     "field": "deleted_at" }
{ "op": "is_not_null", "field": "email" }
```

`is_null` — поле отсутствует или равно `null`. `is_not_null` — поле
присутствует и не null.

### `in` / `not_in` — список значений

```json
{ "op": "in",     "field": "status", "values": ["active", "pending"] }
{ "op": "not_in", "field": "status", "values": ["banned"] }
```

Обрати внимание: ключ — `values` (массив), а не `value`.

### `between` — диапазон включительно с обоих концов

```json
{ "op": "between", "field": "age", "from": 25, "to": 35 }
```

Эквивалентно `age >= 25 AND age <= 35`. Обе границы включительны.

### `like` — шаблон строки

```json
{ "op": "like",  "field": "email", "pattern": "%@example.com" }
{ "op": "ilike", "field": "email", "pattern": "%@EXAMPLE.COM" }
```

`%` — любая последовательность, `_` — один символ. `ilike` —
регистронезависимый вариант.

### `exists` / `not_exists` — наличие поля у записи

```json
{ "op": "exists",     "field": "email" }
{ "op": "not_exists", "field": "temp" }
```

### Комбинирование: `and`, `or`, `not`

```json
{
  "op": "and",
  "filters": [
    { "op": "gte", "field": "age", "value": 30 },
    { "op": "lte", "field": "age", "value": 50 },
    { "op": "or", "filters": [
        { "op": "eq", "field": "city", "value": "NYC" },
        { "op": "eq", "field": "city", "value": "LA" }
    ]}
  ]
}
```

```json
{ "op": "not", "filter": { "op": "eq", "field": "status", "value": "deleted" } }
```

Вложенность любая. `and`/`or` принимают `filters` (массив), `not` —
один `filter`.

### Путь к полю: строка или массив

`field` принимает **два формата**:

* **Строка** — верхнее поле: `"field": "id"` (частый случай).
* **Массив** — вложенный путь: `"field": ["address", "city"]` →
  `record.address.city`.

Для одноэлементного пути строка и массив — эквивалентны:
`"id"` === `["id"]`. Serializer всегда выдаёт канонический массив, но
в запросе можно писать строку — это чище.

```json
{ "op": "eq", "field": ["address", "city"], "value": "NY" }
```

## 2. Мульти-запросные батчи

Несколько операций — один round-trip. Ключи в `queries` — **алиасы**;
результаты вернутся в `resp.results[alias]`.

### Независимые операции

```json
{
  "id": "multi",
  "queries": {
    "users":  { "from": "users" },
    "orders": { "from": "orders" },
    "seed":   {
      "insert_into": "users",
      "values": [{ "name": "Alice", "score": 100 }]
    }
  }
}
```

Чтения и записи вперемешку. Порядок ключей в JSON не гарантируется,
но движок корректно упорядочит выполнение.

### Скрытие промежуточных результатов

`"return_result": false` — операция выполнится, но результат не вернётся.
Удобно для промежуточных записей:

```json
{
  "id": "setup-and-read",
  "queries": {
    "setup": {
      "insert_into": "users",
      "values": [{ "name": "Alice" }],
      "return_result": false
    },
    "read": { "from": "users" }
  }
}
```

### Зависимые запросы: `$query`-ссылки

Один запрос может ссылаться на результат другого через `{"$query": "<alias>", "path": "<path>"}`.

```json
{
  "id": "chained",
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "name", "value": "alice" }
    },
    "user_orders": {
      "from": "orders",
      "where": {
        "op": "eq",
        "field": "user_id",
        "value": { "$query": "user", "path": "[0].id" }
      }
    }
  }
}
```

`"path": "[0].id"` — взять поле `id` из первой записи результата
`user`. Планировщик автоматически выстроит этапы: `user` → `user_orders`.

Краткая запись `"path"` поддерживает навигацию по результату:
`[0].field`, `[].field` (все элементы), `.count` / `.length`.

## 3. Вторичные индексы

Без индекса каждый `where` сканирует таблицу целиком (O(n)). Индекс
ускоряет конкретные паттерны доступа. Создаётся — как и всё — батчем:

### Обычный (hash) индекс

Ускоряет `eq`-поиск по полю:

```json
{
  "id": 1,
  "queries": {
    "idx": {
      "create_index": "name_idx",
      "table": "users",
      "fields": [["name"]]
    }
  }
}
```

После этого `{ "op": "eq", "field": "name", "value": "Bob" }` пойдёт
через индекс — O(log n) вместо полного скана.

### Уникальный индекс

То же, что обычный, плюс constraint: дубль по полю → ошибка.

```json
{
  "id": 2,
  "queries": {
    "idx": {
      "create_index": "email_idx",
      "table": "users",
      "fields": [["email"]],
      "unique": true
    }
  }
}
```

### Sorted-индекс (для диапазонов, сортировки, MIN/MAX)

Ключевое отличие: **значения хранятся упорядоченно**. Это даёт O(log n)
для:

* диапазонов: `between`, `gt`/`gte`, `lt`/`lte` по одному полю;
* `order by` + `LIMIT K` — первые/последние K без полной сортировки;
* `MIN(field)`, `MAX(field)` — O(1) из начала/конца индекса.

```json
{
  "id": 1,
  "queries": {
    "idx": {
      "create_index": "score_idx",
      "table": "users",
      "fields": [["score"]],
      "sorted": true
    }
  }
}
```

> `unique: true` + `sorted: true` одновременно — **запрещено**.
> Sorted-индекс — только одно скалярное поле.

### Какой индекс для какого запроса

| Паттерн запроса | Нужный индекс |
|---|---|
| `{ "op": "eq", "field": "email", … }` | обычный или `unique` по `email` |
| `{ "op": "between", "field": "age", … }` | `sorted` по `age` |
| `order by score desc` + `LIMIT 10` | `sorted` по `score` |
| `MIN(score)`, `MAX(score)` | `sorted` по `score` |

## 4. Диапазоны, сортировка, лимиты

### BETWEEN + sorted-индекс

```json
{
  "from": "users",
  "where": { "op": "between", "field": ["age"], "from": 30, "to": 35 }
}
```

При наличии sorted-индекса по `age` — сканирование от 30 до 35,
O(log n + K), где K — число попавших записей.

### ORDER BY + LIMIT

```json
{
  "from": "users",
  "order_by": {
    "items": [
      { "field": ["score"], "direction": "desc" }
    ]
  },
  "pagination": { "mode": "LimitOffset", "limit": 10, "offset": 0 }
}
```

* `direction`: `"asc"` (по умолчанию) или `"desc"`.
* `pagination.mode: "LimitOffset"` — классический limit/offset.
* При sorted-индексе по `score` движок возьмёт 10 записей прямо
  из индекса — без сортировки всей таблицы.

### MIN / MAX

```json
{
  "from": "users",
  "select": {
    "items": [
      { "type": "aggregate", "func": "min", "field": ["score"], "alias": "lo" },
      { "type": "aggregate", "func": "max", "field": ["score"], "alias": "hi" }
    ]
  }
}
```

С sorted-индексом — O(1): берётся первая (min) и/или последняя (max)
запись.

### Постраничная навигация

Альтернатива LimitOffset — постраничный режим:

```json
{
  "from": "users",
  "pagination": { "mode": "Page", "page": 2, "page_size": 25 },
  "count_total": true
}
```

`count_total: true` — вернуть общее число записей в ответе (нужно для
пагинации UI).

## Что важно знать уже сейчас (дозированно)

* **Индекс — overlay.** Данные живут в MVCC-сторе; индекс лишь
  ускоряет доступ. При crash он восстанавливается из WAL.
* **`from` — это не только строка.** `"from": "users"` → таблица
  `users` в репозитории `main` (по умолчанию). Если нужна таблица
  из другого репозитория: `"from": ["hot", "sessions"]` → репо `hot`,
  таблица `sessions`. Подробности — [этаж 3](./03-storage.md).
* **`select` по умолчанию — `SELECT *`.** Опускай его, пока не нужны
  агрегаты или проекции.
* **FTS, vector, functional-индексы** — отдельный зоопарк, им посвящён
  [этаж 6](./06-search.md). Здесь мы не касаемся `op: "fts"`,
  `op: "vector_similarity"` и `op: "computed"`.

## Куда дальше

| Упёрся в… | Поднимайся на |
|---|---|
| «данные терять нельзя, нужны транзакции» | [Этаж 2 — Durability](./02-durability.md) |
| «несколько хранилищ, бэкап, миграции» | [Этаж 3 — Хранилища](./03-storage.md) |
| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
