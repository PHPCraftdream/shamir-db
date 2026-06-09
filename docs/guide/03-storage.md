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

```json
{
  "id": "mk-analytics",
  "queries": {
    "mk": { "create_db": "analytics" }
  }
}
```

Ответ: `results.mk.records[0].created_db === "analytics"`.

Создаём против `"default"` (базы живут на уровне сервера):

```javascript
await client.execute('default', {
  id: 'mk',
  queries: { mk: { create_db: 'analytics' } },
});
```

### Интроспекция: `list`

```json
{
  "id": "ls",
  "queries": { l: { "list": "databases" } }
}
```

Ответ:

```json
{
  "results": {
    "l": { "records": [{ "databases": ["default", "analytics"] }] }
  }
}
```

### Удаление (требует HMAC)

```javascript
await client.execute('default', {
  id: 1,
  queries: { d: hmac.drop_db_op(client, 'analytics') },
});
```

Ответ: `results.d.records[0].dropped === "analytics"`.

## 2. Репозитории

Репозиторий (repo) — единица хранения внутри базы. Каждая БД начинается
с репозитория `main`. Можно создавать дополнительные — для горячих/холодных
данных, изоляции или разных движков.

### Создание репозитория

```json
{
  "id": "mk-cold",
  "queries": {
    "r": { "create_repo": "cold" }
  }
}
```

Запускаем против целевой базы:

```javascript
await client.execute('analytics', {
  id: 'mk-cold',
  queries: { r: { create_repo: 'cold' } },
});
```

Ответ: `results.r.records[0].created_repo === "cold"`.

**Durable по умолчанию.** Если сервер запущен с `data_dir` (продакшен),
wire-созданные репозитории — durable (redb). Если сервер in-memory
(тесты) — репозиторий тоже in-memory. Явный opt-out:

```json
{ "create_repo": "scratch", "engine": "in_memory" }
```

`in_memory`-репозиторий — эфемерный scratch-пространство: данные не
переживают рестарт. Полезно для кешей, сессий, временных расчётов.

<!-- TODO: verify that wire-created durable repo engine=redb path is auto-derived from data_root per DURABLE_BY_DEFAULT.md D2 — currently CreateRepoOp.engine is Option<String> with None default -->

### Интроспекция

```json
{
  "id": "ls-repos",
  "queries": { l: { "list": "repos" } }
}
```

Ответ:

```json
{
  "results": {
    "l": { "records": [{ "repos": ["main", "cold"] }] }
  }
}
```

### Удаление (требует HMAC)

```javascript
await client.execute(dbName, {
  id: 1,
  queries: { d: hmac.drop_repo_op(client, dbName, 'cold') },
});
```

## 3. Таблицы в разных репозиториях

Таблица принадлежит репозиторию. По умолчанию — `main`. Адресация
через поле `repo`:

```json
{
  "id": "mk-tables",
  "queries": {
    "t1": { "create_table": "users", "repo": "main" },
    "t2": { "create_table": "archive", "repo": "cold" }
  }
}
```

Интроспекция таблиц — repo-scoped:

```json
{
  "id": "ls-main",
  "queries": { l: { "list": "tables", "repo": "main" } }
}
```

Ответ:

```json
{
  "results": {
    "l": { "records": [{ "tables": ["users"], "repo": "main" }] }
  }
}
```

Чтение из таблицы другого репо — через `"from": ["repo", "table"]`
(подробнее на этаже 1).

## 4. Конфигурация буферов (per-table)

Каждая таблица имеет in-memory буфер (MemBuffer), через который проходят
записи перед сбросом на диск. Можно настроить его параметры.

### Просмотр

```json
{
  "id": "get-buf",
  "queries": {
    "g": { "get_buffer_config": "items", "repo": "main" }
  }
}
```

Ответ (если не настроен): `records: [{ table: "items", repo: "main", config: null }]`.

### Установка

```json
{
  "id": "set-buf",
  "queries": {
    "s": {
      "set_buffer_config": "items",
      "repo": "main",
      "config": {
        "max_bytes": 1048576,
        "max_entries": 500,
        "ttl_ms": 7000,
        "flush_interval_ms": 333,
        "flush_batch_size": 48
      }
    }
  }
}
```

Ответ: `records: [{ set_buffer_config: "items", repo: "main", config: {…} }]`.

### Частичное обновление

```json
{
  "id": "alter-buf",
  "queries": {
    "a": {
      "alter_buffer_config": "items",
      "repo": "main",
      "patch": { "flush_interval_ms": 1000, "max_entries": 9999 }
    }
  }
}
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

```json
{
  "id": "idx-ddl",
  "queries": {
    "mk": {
      "create_index": "by_email",
      "table": "users",
      "fields": [["email"]]
    }
  }
}
```

Интроспекция:

```json
{
  "id": "ls-idx",
  "queries": { l: { "list": "indexes", "repo": "main", "table": "users" } }
}
```

Ответ содержит rich-entries (не только имена):

```json
{
  "results": {
    "l": { "records": [{ "indexes": [{ "name": "by_email" }], "repo": "main", "table": "users" }] }
  }
}
```

Удаление индекса — HMAC-gated:

```javascript
await client.execute(dbName, {
  id: 1,
  queries: { d: hmac.drop_index_op(client, dbName, 'main', 'users', 'by_email') },
});
```

Для уникальных индексов — свой flavour тега:

```javascript
hmac.drop_index_op(client, dbName, 'main', 'users', 'by_email', { unique: true });
```

## 6. Access-tree: интроспекция прав

`access_tree` — read-only операция, которая возвращает дерево ресурсов
с владельцами, группами и режимами. Полная картина — кто и что может.

```json
{
  "id": "tree",
  "queries": {
    "t": { "access_tree": true }
  }
}
```

С фильтром по базе и ограничением глубины:

```json
{
  "id": "tree-db",
  "queries": {
    "t": { "access_tree": true, "db": "analytics", "depth": 2 }
  }
}
```

Ответ содержит секции `resources`, `principals` и `functions`:

```json
{
  "results": {
    "t": {
      "records": [{
        "access_tree": {
          "resources": {
            "kind": "root",
            "children": [
              { "name": "default", "kind": "database", "children": […] }
            ]
          },
          "principals": { "users": […], "groups": […] },
          "functions": [{ "name": "argon2id" }, …]
        }
      }]
    }
  }
}
```

Подробнее о правах и владельцах — [этаж 4](./04-access.md).

## Что важно знать уже сейчас (дозированно)

* **`create_db` / `create_repo` / `create_table` — не требуют HMAC.**
  Это созидательные операции. Удаление (`drop_*`) — требует.
* **Durability репозитория = durability дома.** Если сервер запущен с
  `data_dir`, wire-созданные репозитории — durable. Если in-memory
  (тесты) — in-memory. Когерентно, без отдельного флага.
* **База — единица изоляции.** `client.execute("db-a", …)` и
  `client.execute("db-b", …)` видят только свои данные. Удаление базы A
  не влияет на базу B.
* **Транзакция = один репозиторий.** Нельзя в одной транзакции записать в
  `main` и `cold` — см. [этаж 2](./02-durability.md).

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «пользователей много, нужны права и группы» | [Этаж 4 — Доступ](./04-access.md) |
| «логика переезжает в БД» | [Этаж 5 — Функции](./05-functions.md) |
