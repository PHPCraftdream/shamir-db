בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 1 — Запросы: фильтры, батчи, индексы

KV-хранилище с этажа 0 — уже работает. Но данные растут: нужны выборки
по полям, диапазоны, сортировка, индексы. Этот этаж — именно об этом.

Примеры ниже используют TS-клиент `@shamir/client`. Подключение — как
на этаже 0: `const db = client.db('default')`.

## 1. Фильтры: `where`

Каждый `db.query(table)` принимает необязательный `.where(filter.*)` —
фильтр, построенный через билдер. Вот основные операции.

### Сравнения

```ts
import { filter } from '@shamir/client';

// равно
const rows = await db.query('users').where(filter.eq('status', 'active')).rows();
```

| Метод | Смысл | Пример |
|---|---|---|
| `filter.eq(f, v)` | равно | `filter.eq('status', 'active')` |
| `filter.ne(f, v)` | не равно | `filter.ne('status', 'inactive')` |
| `filter.gt(f, v)` | больше | `filter.gt('age', 18)` |
| `filter.gte(f, v)` | больше или равно | `filter.gte('salary', 50000)` |
| `filter.lt(f, v)` | меньше | `filter.lt('age', 65)` |
| `filter.lte(f, v)` | меньше или равно | `filter.lte('score', 100)` |

Значение — строка, число, булев или `null` — Shamir сам приведёт тип.

### Проверка на null / существование поля

```ts
// поле отсутствует или равно null
db.query('users').where(filter.isNull('deleted_at'))

// поле присутствует и не null
db.query('users').where(filter.isNotNull('email'))
```

`filter.isNull` — поле отсутствует или равно `null`. `filter.isNotNull` — поле
присутствует и не null.

### `in_` / `notIn` — список значений

```ts
// поле входит в список
db.query('users').where(filter.in_('status', ['active', 'pending']))

// поле не входит в список
db.query('users').where(filter.notIn('status', ['banned']))
```

### `between` — диапазон включительно с обоих концов

```ts
db.query('users').where(filter.between('age', 25, 35))
```

Эквивалентно `age >= 25 AND age <= 35`. Обе границы включительны.

### `like` / `ilike` — шаблон строки

```ts
// LIKE
db.query('users').where(filter.like('email', '%@example.com'))

// Case-insensitive LIKE
db.query('users').where(filter.ilike('email', '%@EXAMPLE.COM'))
```

`%` — любая последовательность, `_` — один символ. `ilike` —
регистронезависимый вариант.

### `exists` / `notExists` — наличие поля у записи

```ts
db.query('users').where(filter.exists('email'))
db.query('users').where(filter.notExists('temp'))
```

### Комбинирование: `and`, `or`, `not`

```ts
// AND через andWhere
db.query('users')
  .where(filter.gte('age', 30))
  .andWhere(filter.lte('age', 50))

// AND + OR — вложенные комбинаторы
db.query('users').where(
  filter.and([
    filter.gte('age', 30),
    filter.lte('age', 50),
    filter.or([
      filter.eq('city', 'NYC'),
      filter.eq('city', 'LA'),
    ]),
  ])
)

// NOT
db.query('users').where(filter.not(filter.eq('status', 'deleted')))
```

Вложенность любая. `filter.and([...])` / `filter.or([...])` принимают массив,
`filter.not(f)` — один фильтр.

### Путь к полю: строка или массив

`field` принимает **два формата**:

* **Строка** — верхнее поле: `filter.eq('id', ...)` (частый случай).
* **Массив** — вложенный путь: `filter.eq(['address', 'city'], 'NY')` →
  `record.address.city`.

Для одноэлементного пути строка и массив — эквивалентны:
`'id'` === `['id']`.

```ts
db.query('users').where(filter.eq(['address', 'city'], 'NY'))
```

## 2. Мульти-запросные батчи

Несколько операций — один round-trip. Алиасы — ключи в `.add(alias, ...)`;
результаты вернутся в `resp.results[alias]`.

### Независимые операции

```ts
import { Query, write } from '@shamir/client';

const resp = await db.batch()
  .add('users',  db.query('users'))
  .add('orders', db.query('orders'))
  .add('seed',   write.insert('users', [{ name: 'Alice', score: 100 }]))
  .run();

resp.results.users.records;
resp.results.orders.records;
```

Чтения и записи вперемешку. Движок корректно упорядочит выполнение.

### Скрытие промежуточных результатов

`.add(alias, op, { returnResult: false })` — операция выполнится, но результат
не вернётся. Удобно для промежуточных записей:

```ts
const resp = await db.batch()
  .add('setup', write.insert('users', [{ name: 'Alice' }]), { returnResult: false })
  .add('read',  db.query('users'))
  .run();

resp.results.read.records; // ['Alice']
// resp.results.setup — отсутствует
```

### Зависимые запросы: `filter.queryRef`

Один запрос может ссылаться на результат другого через `filter.queryRef('@alias', path)`.

```ts
const resp = await db.batch()
  .add('user', db.query('users').where(filter.eq('name', 'alice')))
  .add('user_orders', db.query('orders').where(
    filter.eq('user_id', filter.queryRef('@user', '[0].id'))
  ))
  .run();

resp.results.user_orders.records; // заказы alice
resp.execution_plan;               // [['user'], ['user_orders']] — два этапа
```

`'[0].id'` — взять поле `id` из первой записи результата `user`. Планировщик
автоматически выстроит этапы: `user` → `user_orders`.

Краткая запись `path` поддерживает навигацию по результату:
`[0].field`, `[].field` (все элементы), `.count` / `.length`.

## 3. Вторичные индексы

Без индекса каждый `where` сканирует таблицу целиком (O(n)). Индекс
ускоряет конкретные паттерны доступа. Создаётся через `ddl.createIndex`:

### Обычный (hash) индекс

Ускоряет `filter.eq`-поиск по полю:

```ts
import { ddl } from '@shamir/client';

await db.run(ddl.createIndex('name_idx', 'users', [['name']]));
```

После этого `filter.eq('name', 'Bob')` пойдёт через индекс — O(log n)
вместо полного скана.

### Уникальный индекс

То же, что обычный, плюс constraint: дубль по полю → ошибка.

```ts
await db.run(ddl.createIndex('email_idx', 'users', [['email']], { unique: true }));
```

### Sorted-индекс (для диапазонов, сортировки, MIN/MAX)

Ключевое отличие: **значения хранятся упорядоченно**. Это даёт O(log n)
для:

* диапазонов: `between`, `gt`/`gte`, `lt`/`lte` по одному полю;
* `orderByAsc` / `orderByDesc` + `limit` — первые/последние K без полной сортировки;
* `select.min` / `select.max` — O(1) из начала/конца индекса.

```ts
await db.run(ddl.createIndex('score_idx', 'users', [['score']], { sorted: true }));
```

> `unique: true` + `sorted: true` одновременно — **запрещено**.
> Sorted-индекс — только одно скалярное поле.

### Какой индекс для какого запроса

| Паттерн запроса | Нужный индекс |
|---|---|
| `filter.eq('email', …)` | обычный или `unique` по `email` |
| `filter.between('age', …)` | `sorted` по `age` |
| `orderByDesc('score')` + `limit(10)` | `sorted` по `score` |
| `select.min('score')`, `select.max('score')` | `sorted` по `score` |

## 4. Диапазоны, сортировка, лимиты

### BETWEEN + sorted-индекс

```ts
const rows = await db.query('users')
  .where(filter.between('age', 30, 35))
  .rows();
```

При наличии sorted-индекса по `age` — сканирование от 30 до 35,
O(log n + K), где K — число попавших записей.

### ORDER BY + LIMIT

```ts
const rows = await db.query('users')
  .orderByDesc('score')
  .limit(10)
  .offset(0)
  .rows();
```

* `orderByAsc(field)` / `orderByDesc(field)` — сортировка по одному полю.
* При sorted-индексе по `score` движок возьмёт 10 записей прямо
  из индекса — без сортировки всей таблицы.

### MIN / MAX

```ts
import { select } from '@shamir/client';

const qr = await db.query('users')
  .select([
    select.min('score', { alias: 'lo' }),
    select.max('score', { alias: 'hi' }),
  ])
  .ex();

const { lo, hi } = qr.records[0];
```

С sorted-индексом — O(1): берётся первая (min) и/или последняя (max)
запись.

### Постраничная навигация

```ts
// LimitOffset
const rows = await db.query('users')
  .limit(25)
  .offset(25) // вторая страница
  .rows();

// 1-based page helper
const rows2 = await db.query('users').page(2, 25).rows();

// С подсчётом общего числа записей
const qr = await db.query('users')
  .where(filter.gte('score', 50))
  .limit(25)
  .offset(0)
  .countTotal()
  .ex();

const total = qr.pagination?.total_count; // нужно для пагинации UI
```

`countTotal()` — вернуть общее число записей в ответе (нужно для
пагинации UI). Результат — в `qr.pagination.total_count`.

## Что важно знать уже сейчас (дозированно)

* **Индекс — overlay.** Данные живут в MVCC-сторе; индекс лишь
  ускоряет доступ. При crash он восстанавливается из WAL.
* **`Query.from` и `Query.withRepo`.** `db.query('users')` → таблица
  `users` в репозитории `main` (по умолчанию). Если нужна таблица
  из другого репозитория: `Query.withRepo('hot', 'sessions')`.
  Подробности — [этаж 3](./03-storage.md).
* **`select` по умолчанию — `SELECT *`.** Опускай его, пока не нужны
  агрегаты или проекции.
* **FTS, vector, functional-индексы** — отдельный зоопарк, им посвящён
  [этаж 6](./06-search.md). Здесь мы не касаемся `filter.fts`,
  `filter.vectorSimilarity` и `filter.computed`.

## Куда дальше

| Упёрся в… | Поднимайся на |
|---|---|
| «данные терять нельзя, нужны транзакции» | [Этаж 2 — Durability](./02-durability.md) |
| «несколько хранилищ, бэкап, миграции» | [Этаж 3 — Хранилища](./03-storage.md) |
| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
