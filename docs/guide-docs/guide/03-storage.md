בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 3 — Хранилища: репозитории, бэкап, интроспекция

**Когда подниматься:** несколько хранилищ / много данных.

До этого этажа всё живёт в одной базе (`default`) и одном репозитории
(`main`). Этого хватает для KV и простых выборок. Но данные растут:
нужны отдельные базы, холодные репозитории, контроль буферов и
интроспекция.

## 1. Базы данных

Каждая база — полностью изолированное пространство имён. Таблицы из
одной базы не видны в другой.

### Создание

```ts
import { ddl, Batch } from '@shamir/client';

await Batch.create('mk-analytics')
  .add('mk', ddl.createDb('analytics'))
  .execute(client, 'default');
```

Ответ: `results.mk.records[0].created_db === "analytics"`.

Создаём против `"default"` (базы живут на уровне сервера):

### Интроспекция: `listDatabases`

```ts
const resp = await Batch.create('ls')
  .add('l', ddl.listDatabases())
  .execute(client, 'default');

resp.results.l.records[0].databases; // ['default', 'analytics']
```

(wire form; clients build this via the query builder)

### Удаление (требует HMAC)

```ts
await Batch.create('drop-db')
  .add('d', ddl.dropDb(client, 'analytics', { cascade: true }))
  .execute(client, 'default');
```

Ответ: `results.d.records[0].dropped === "analytics"`.

## 2. Репозитории

Репозиторий (repo) — единица хранения внутри базы. Каждая БД начинается
с репозитория `main`. Можно создавать дополнительные — для горячих/холодных
данных, изоляции или разных движков.

### Создание репозитория

```ts
const db = client.db('analytics');

await db.run(ddl.createRepo('cold'));
```

Ответ: `results.r.records[0].created_repo === "cold"`.

**Durable по умолчанию.** Если сервер запущен с `data_dir` (продакшен),
wire-созданные репозитории — durable (fjall). Если сервер in-memory
(тесты) — репозиторий тоже in-memory. Явный opt-out:

```ts
// wire form: { "create_repo": "scratch", "engine": "in_memory" }
await db.run(ddl.createRepo('scratch', { engine: 'in_memory' }));
```

`in_memory`-репозиторий — эфемерный scratch-пространство: данные не
переживают рестарт. Полезно для кешей, сессий, временных расчётов.

### Интроспекция

```ts
const resp = await Batch.create('ls-repos')
  .add('l', ddl.listRepos())
  .execute(client, 'analytics');

resp.results.l.records[0].repos; // ['main', 'cold']
```

### Удаление (требует HMAC)

```ts
await db.dropRepo('cold');
```

## 3. Таблицы в разных репозиториях

Таблица принадлежит репозиторию. По умолчанию — `main`. Адресация
через опцию `repo`:

```ts
await Batch.create('mk-tables')
  .add('t1', ddl.createTable('users',   { repo: 'main' }))
  .add('t2', ddl.createTable('archive', { repo: 'cold' }))
  .execute(client, 'analytics');
```

Интроспекция таблиц — repo-scoped:

```ts
const resp = await Batch.create('ls-main')
  .add('l', ddl.listTables({ repo: 'main' }))
  .execute(client, 'analytics');

resp.results.l.records[0].tables; // ['users']
resp.results.l.records[0].repo;   // 'main'
```

Чтение из таблицы другого репо — через `Query.withRepo`:

```ts
import { Query } from '@shamir/client';

const rows = await Batch.create('read-cold')
  .add('r', Query.withRepo('cold', 'archive'))
  .execute(client, 'analytics');
```

## 4. Конфигурация буферов (per-table)

Каждая таблица имеет in-memory буфер (MemBuffer), через который проходят
записи перед сбросом на диск. Можно настроить его параметры.

### Просмотр

```ts
const resp = await Batch.create('get-buf')
  .add('g', ddl.getBufferConfig('items', { repo: 'main' }))
  .execute(client, 'analytics');

resp.results.g.records[0]; // { table: 'items', repo: 'main', config: null }
```

### Установка

```ts
await Batch.create('set-buf')
  .add('s', ddl.setBufferConfig('items', {
    max_bytes:        1048576,
    max_entries:      500,
    ttl_ms:           7000,
    flush_interval_ms: 333,
    flush_batch_size:  48,
  }, { repo: 'main' }))
  .execute(client, 'analytics');
```

Ответ: `records: [{ set_buffer_config: "items", repo: "main", config: {…} }]`.

### Частичное обновление

```ts
await Batch.create('alter-buf')
  .add('a', ddl.alterBufferConfig('items', {
    flush_interval_ms: 1000,
    max_entries:       9999,
  }, { repo: 'main' }))
  .execute(client, 'analytics');
```

Патч меняет только указанные поля. Остальные — без изменений.
Особый контракт `ttl_ms`:

|| Значение в `patch` | Смысл |
|---|---|
| ключ опущен | оставить как есть |
| `null` | очистить TTL |
| число (ms) | установить TTL |

## 5. Индексы: создание и удаление

Подробно — на этаже 1. Здесь — полная картина DDL:

```ts
await Batch.create('idx-ddl')
  .add('mk', ddl.createIndex('by_email', 'users', [['email']]))
  .execute(client, 'analytics');
```

Интроспекция:

```ts
const resp = await Batch.create('ls-idx')
  .add('l', ddl.listIndexes('users', { repo: 'main' }))
  .execute(client, 'analytics');

resp.results.l.records[0].indexes; // [{ name: 'by_email' }]
```

Удаление индекса — HMAC-gated:

```ts
await Batch.create('rm-idx')
  .add('d', ddl.dropIndex(client, 'analytics', 'main', 'users', 'by_email'))
  .execute(client, 'analytics');
```

Для уникальных индексов — свой flavour тега:

```ts
await Batch.create('rm-unique-idx')
  .add('d', ddl.dropIndex(client, 'analytics', 'main', 'users', 'by_email', { unique: true }))
  .execute(client, 'analytics');
```

## 6. Access-tree: интроспекция прав

`accessTree` — read-only операция, которая возвращает дерево ресурсов
с владельцами, группами и режимами. Полная картина — кто и что может.

```ts
import { admin } from '@shamir/client';

const resp = await Batch.create('tree')
  .add('t', admin.accessTree())
  .execute(client, 'default');
```

С фильтром по базе и ограничением глубины:

```ts
const resp = await Batch.create('tree-db')
  .add('t', admin.accessTree({ db: 'analytics', depth: 2 }))
  .execute(client, 'default');
```

Ответ содержит секции `resources`, `principals` и `functions` (структура ответа — MessagePack на проводе):

```
{
  "results": {
    "t": {
      "records": [{
        "access_tree": {
          "resources": {
            "kind": "root",
            "children": [
              { "name": "default", "kind": "database", "children": ["…"] }
            ]
          },
          "principals": { "users": ["…"], "groups": ["…"] },
          "functions": [{ "name": "argon2id" }, "…"]
        }
      }]
    }
  }
}
```

Подробнее о правах и владельцах — [этаж 4](./04-access.md).

## Что важно знать уже сейчас (дозированно)

* **`ddl.createDb` / `ddl.createRepo` / `ddl.createTable` — не требуют HMAC.**
  Это созидательные операции. Удаление (`drop_*`) — требует.
* **Durability репозитория = durability дома.** Если сервер запущен с
  `data_dir`, wire-созданные репозитории — durable. Если in-memory
  (тесты) — in-memory. Когерентно, без отдельного флага.
* **База — единица изоляции.** `client.db('db-a')` и
  `client.db('db-b')` видят только свои данные. Удаление базы A
  не влияет на базу B.
* **Транзакция = один репозиторий.** Нельзя в одной транзакции записать в
  `main` и `cold` — см. [этаж 2](./02-durability.md).

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «пользователей много, нужны права и группы» | [Этаж 4 — Доступ](./04-access.md) |
| «логика переезжает в БД» | [Этаж 5 — Функции](./05-functions.md) |
