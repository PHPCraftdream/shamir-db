בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 4 — Доступ: Shomer (юзеры, группы, права)

**Когда подниматься:** >1 типа пользователей.

До этого этажа работал один bootstrap-админ — он может всё. Но продукт
растёт: появляются обычные пользователи, аналитики, read-only боты.
Нужно разграничить доступ.

**Shomer** (שׁוֹמֵר, «страж») — подсистема управления доступом ShamirDB.
Модель — POSIX-style DAC: владелец, группа, mode-биты (`rwx`) на ресурсах
в иерархическом дереве. Знакомая семантика `chmod`/`chown`/`chgrp`.

## 1. Пользователи

### Создание

Пользователь создаётся через отдельный wire-запрос (не батч):

```javascript
await client.createScramUser('alice', 'alice-password', []);
```

Под капотом — `DbRequest::CreateScramUser`:

```json
{
  "op": "create_scram_user",
  "name": "alice",
  "password": "alice-password",
  "roles": []
}
```

Ответ: `DbResponse::UserCreated { name: "alice", user_id: <16 байт> }`.

Пользователь получает стабильный числовой ID (hash от имени). Этот ID
используется в `chown`, `chgrp`, группах.

### Подключение

Каждый пользователь подключается отдельно — SCRAM-аутентификация,
TLS 1.3. Сессия привязана к principal:

```javascript
const alice = await Client.connect({
  addr: '127.0.0.1:7000',
  server_name: 'localhost',
  username: 'alice',
  password: Zeroizing::new(b'alice-password'.to_vec()),
  accept_new_host: true,
  trusted_pin: None,
});
```

### Роли и разрешения (RBAC-основа)

Роль — именованный набор разрешений:

```json
{
  "id": "mk-role",
  "queries": {
    "r": {
      "create_role": "analyst",
      "permissions": [
        {
          "effect": "allow",
          "actions": ["read"],
          "resource": { "scope": "database", "database": "analytics" }
        }
      ]
    }
  }
}
```

Каждое разрешение:

|| Поле | Описание |
|---|---|---|
| `effect` | `"allow"` или `"deny"` |
| `actions` | массив: `"read"`, `"insert"`, `"update"`, `"delete"`, `"create"`, `"drop"`, `"alter"`, `"manage_users"`, `"manage_roles"`, `"all"` |
| `resource` | область действия: `{ "scope": "global" }` / `{ "scope": "database", "database": "…" }` / `{ "scope": "repo", "database": "…", "repo": "…" }` / `{ "scope": "table", "database": "…", "repo": "…", "table": "…" }` |

Чем выше `specificity` (table > repo > database > global), тем приоритетнее.
Разрешение на `database` покрывает все repo и table внутри.

Назначение роли пользователю:

```json
{
  "id": "grant",
  "queries": {
    "g": { "grant_role": "analyst", "user": "alice" }
  }
}
```

Отзыв:

```json
{
  "id": "revoke",
  "queries": {
    "r": { "revoke_role": "analyst", "user": "alice" }
  }
}
```

Удаление роли (HMAC-gated):

```json
{
  "id": "drop-role",
  "queries": {
    "d": { "drop_role": "analyst", "hmac": "<tag>" }
  }
}
```

### Интроспекция

```json
{
  "id": "ls",
  "queries": {
    "u": { "list": "users" },
    "r": { "list": "roles" }
  }
}
```

## 2. POSIX-режимы: `chmod`, `chown`, `chgrp`

Каждый ресурс в дереве Shomer (database, store, table, function) несёт
метаданные: **owner** (id пользователя), **group** (id группы), **mode**
(9 бит `rwx|rwx|rwx`).

### Ресурсы и адресация

Ресурс указывается объектом (`ResourceRef`):

|| Ресурс | Объект в JSON |
|---|---|---|
| База данных | `{ "database": "mydb" }` |
| Репозиторий | `{ "store": ["mydb", "main"] }` |
| Таблица | `{ "table": ["mydb", "main", "users"] }` |
| Функция | `{ "function": "my_fn" }` |
| Папка функций | `{ "function_folder": ["reports", "daily"] }` |
| Пространство имён функций | `{ "function_namespace": true }` |

### chmod — сменить режим

```json
{
  "id": "chmod-tbl",
  "queries": {
    "cm": {
      "chmod": { "table": ["ptest", "main", "secret"] },
      "mode": 448
    }
  }
}
```

`448` = `0o700` = `rwx------` (только владелец).

|| Примеры mode | Октал | Смысл |
|---|---|---|---|
| `rwxrwxrwx` | `0o777` (493) | полный доступ всем (дефолт) |
| `rwx------` | `0o700` (448) | только владелец |
| `rwxr-x---` | `0o750` (488) | владелец — всё, группа — чтение и traversal |
| `r--r--r--` | `0o444` (292) | все только читают |

### chown — сменить владельца

```json
{
  "id": "chown-db",
  "queries": {
    "co": {
      "chown": { "database": "analytics" },
      "owner": 7
    }
  }
}
```

`owner` — числовой ID пользователя.

### chgrp — назначить группу

```json
{
  "id": "chgrp-tbl",
  "queries": {
    "cg": {
      "chgrp": { "table": ["gtest", "main", "shared"] },
      "group": 3
    }
  }
}
```

`group` — числовой ID группы. `null` — снять группу:

```json
{
  "chgrp": { "database": "mydb" },
  "group": null
}
```

## 3. Группы

Группа — множество пользователей. Создаётся администратором:

```json
{
  "id": "mkgrp",
  "queries": {
    "mk": { "create_group": "devs" }
  }
}
```

Ответ содержит `group_id`:

```json
{ "results": { "mk": { "records": [{ "create_group": "devs", "group_id": 1 }] } } }
```

### Добавление и удаление участников

```json
{
  "id": "add-bob",
  "queries": {
    "am": { "add_group_member": { "name": "devs" }, "user": 42 }
  }
}
```

```json
{
  "id": "rm-bob",
  "queries": {
    "rm": { "remove_group_member": { "id": 1 }, "user": 42 }
  }
}
```

Группа адресуется по имени или ID:

|| Форма | JSON |
|---|---|
| по имени | `{ "name": "devs" }` |
| по ID | `{ "id": 1 }` |

### Удаление группы

```json
{
  "id": "dropgrp",
  "queries": {
    "dg": { "drop_group": { "name": "devs" } }
  }
}
```

## 4. Как это работает вместе: типичный сценарий

**Задача:** дать разработчикам (bob) доступ к таблице `shared`,
запретить остальным (carol).

```javascript
// 1. Создать базу и таблицу (от имени admin)
await client.execute('default', { id: 1, queries: { mk: { create_db: 'gtest' } } });
await client.execute('gtest', {
  id: 2,
  queries: {
    mr: { create_repo: 'main' },
    tb: { create_table: 'shared' },
  },
});

// 2. Создать группу
const grpResp = await client.execute('gtest', {
  id: 3,
  queries: { mk: { create_group: 'devs' } },
});
const groupId = grpResp.results.mk.records[0].group_id;

// 3. Добавить bob в группу (bob's principal_id = hash(username))
await client.execute('gtest', {
  id: 4,
  queries: { am: { add_group_member: { name: 'devs' }, user: bobPrincipalId } },
});

// 4. Назначить группе таблицу
await client.execute('gtest', {
  id: 5,
  queries: { cg: { chgrp: { table: ['gtest', 'main', 'shared'] }, group: groupId } },
});

// 5. Установить режим 0o750: owner rwx + group r-x + other ---
await client.execute('gtest', {
  id: 6,
  queries: { cm: { chmod: { table: ['gtest', 'main', 'shared'] }, mode: 488 } },
});
```

Результат:

|| Пользователь | Доступ | Почему |
|---|---|---|
| admin (owner) | ✅ разрешён | owner → bits `rwx` (0o700 portion) |
| bob (member of `devs`) | ✅ разрешён | group → bits `r-x` (0o050 portion) |
| carol (не в группе) | ❌ `access_denied` | other → bits `---` (0o000 portion) |

### Дефолт — открытый

Новые ресурсы получают `mode: 0o777` (полный доступ всем). Пока не
выполнишь `chmod` — доступ есть у любого аутентифицированного
пользователя. Это позволяет постепенно ограничивать: сначала работает
всё, потом точечно закрываем.

## 5. Иерархия и traversal

Ресурсы образуют дерево:

```
/                              ← root
├── databases/
│   └── <db>/                  ← Database
│       └── <store>/           ← Store (repo)
│           └── <table>/       ← Table
│               ├── records    ← наследует mode таблицы
│               └── indexes    ← наследует mode таблицы
├── functions/                 ← FunctionNamespace
│   └── <function>             ← Function
├── users/
└── groups/
```

Для доступа к глубокому ресурсу нужна `x` (execute/traverse) на каждом
предке — как `x` на каталогах в POSIX.

`access_tree` (этаж 3) показывает актуальное дерево с владельцами
и режимами.

## 6. Бизнес-доступ через процедуры

Функции (WASM-процедуры, [этаж 5](./05-functions.md)) могут выполняться
с правами **definer** (setuid): вызывается от имени владельца функции,
а не вызывающего. Это позволяет:

* дать пользователю доступ к данным **только** через процедуру;
* скрыть реализацию (private function → `other` не имеет `x`);
* делегировать ограниченные права без раскрытия таблицы.

<!-- TODO: verify setuid function enforcement surface — see ACCESS_FABRIC.md P5 -->

## Что важно знать уже сейчас (дозированно)

* **Shomer — DAC, не RBAC.** Нет графа grant'ов — есть owner/group/mode
  на ресурсах. Для «дать доступ нескольким людям» — группы, не роли.
* **Роли — для разрешений (permissions).** Группы — для POSIX-прав.
  Это две ортогональные оси: роль описывает *что* можно делать
  (`read`, `insert`…), группа — *с кем* делится доступ к ресурсу.
* **Admin обходит все проверки.** Bootstrap-админ (и любой
  `Actor::System`) минует gate Shomer.
* **`access_tree` — read-only.** Не меняет права, только показывает.
* **Удаление пользователя (`drop_user`) — HMAC-gated**, как и все
  деструктивные операции (этаж 2).

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «логика переезжает в БД, нужны WASM-функции» | [Этаж 5 — Функции](./05-functions.md) |
| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
