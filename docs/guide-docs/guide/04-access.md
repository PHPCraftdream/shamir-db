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

Пользователь создаётся через `client.createScramUser`:

```ts
const created = await client.createScramUser('alice', 'alice-password', []);
created.name;    // 'alice'
created.user_id; // Uint8Array(16)
```

Пользователь получает стабильный числовой ID (hash от имени). Этот ID
используется в `chown`, `chgrp`, группах.

### Подключение

Каждый пользователь подключается отдельно — SCRAM-аутентификация,
TLS 1.3. Сессия привязана к principal:

```ts
import { connect } from '@shamir/client';

const alice = await connect({
  host: '127.0.0.1',
  port: 13760,
  username: 'alice',
  password: 'alice-password',
  tls: { rejectUnauthorized: false },
  origin: 'https://127.0.0.1',
});
```

### Роли и разрешения (RBAC-основа)

Роль — именованный набор разрешений:

```ts
import { admin, Batch } from '@shamir/client';

await Batch.create('mk-role')
  .add('r', admin.createRole('analyst', [
    admin.permission('allow', ['read'], admin.scopeDatabase('analytics')),
  ]))
  .execute(client, 'default');
```

Каждое разрешение:

|| Поле | Описание |
|---|---|---|
| `effect` | `"allow"` или `"deny"` |
| `actions` | массив: `"read"`, `"insert"`, `"update"`, `"delete"`, `"create"`, `"drop"`, `"alter"`, `"manage_users"`, `"manage_roles"`, `"all"` |
| `resource` | область действия: `admin.scopeGlobal()` / `admin.scopeDatabase(db)` / `admin.scopeRepo(db, repo)` / `admin.scopeTable(db, repo, table)` |

Чем выше `specificity` (table > repo > database > global), тем приоритетнее.
Разрешение на `database` покрывает все repo и table внутри.

Назначение роли пользователю:

```ts
await Batch.create('grant')
  .add('g', admin.grantRole('analyst', 'alice'))
  .execute(client, 'default');
```

Отзыв:

```ts
await Batch.create('revoke')
  .add('r', admin.revokeRole('analyst', 'alice'))
  .execute(client, 'default');
```

Удаление роли (HMAC-gated):

```ts
await Batch.create('drop-role')
  .add('d', admin.dropRole(client, 'analyst'))
  .execute(client, 'default');
```

### Интроспекция

```ts
import { ddl } from '@shamir/client';

const resp = await Batch.create('ls')
  .add('u', ddl.listUsers())
  .add('r', ddl.listRoles())
  .execute(client, 'default');
```

## 2. POSIX-режимы: `chmod`, `chown`, `chgrp`

Каждый ресурс в дереве Shomer (database, store, table, function) несёт
метаданные: **owner** (id пользователя), **group** (id группы), **mode**
(9 бит `rwx|rwx|rwx`).

### Ресурсы и адресация

Ресурс строится через `admin.ref*`:

|| Ресурс | Вызов |
|---|---|---|
| База данных | `admin.refDatabase('mydb')` |
| Репозиторий | `admin.refStore('mydb', 'main')` |
| Таблица | `admin.refTable('mydb', 'main', 'users')` |
| Функция | `admin.refFunction('my_fn')` |
| Папка функций | `admin.refFunctionFolder(['reports', 'daily'])` |
| Пространство имён функций | `admin.refFunctionNamespace()` |

### chmod — сменить режим

```ts
import { admin, Batch } from '@shamir/client';

await Batch.create('chmod-tbl')
  .add('cm', admin.chmod(admin.refTable('ptest', 'main', 'secret'), 0o700))
  .execute(client, 'ptest');
```

`0o700` = `448` = `rwx------` (только владелец).

|| Примеры mode | Октал | Смысл |
|---|---|---|---|
| `rwxrwxrwx` | `0o777` (493) | полный доступ всем (дефолт) |
| `rwx------` | `0o700` (448) | только владелец |
| `rwxr-x---` | `0o750` (488) | владелец — всё, группа — чтение и traversal |
| `r--r--r--` | `0o444` (292) | все только читают |

### chown — сменить владельца

```ts
await Batch.create('chown-db')
  .add('co', admin.chown(admin.refDatabase('analytics'), 7))
  .execute(client, 'default');
```

`owner` — числовой ID пользователя.

### chgrp — назначить группу

```ts
await Batch.create('chgrp-tbl')
  .add('cg', admin.chgrp(admin.refTable('gtest', 'main', 'shared'), 3))
  .execute(client, 'gtest');

// null — снять группу:
await Batch.create('chgrp-remove')
  .add('cg', admin.chgrp(admin.refDatabase('mydb'), null))
  .execute(client, 'default');
```

`group` — числовой ID группы. `null` — снять группу.

## 3. Группы

Группа — множество пользователей. Создаётся администратором:

```ts
const resp = await Batch.create('mkgrp')
  .add('mk', admin.createGroup('devs'))
  .execute(client, 'gtest');

const groupId = resp.results.mk.records[0].group_id;
```

### Добавление и удаление участников

```ts
await Batch.create('add-bob')
  .add('am', admin.addGroupMember(admin.groupName('devs'), bobPrincipalId))
  .execute(client, 'gtest');

await Batch.create('rm-bob')
  .add('rm', admin.removeGroupMember(admin.groupId(1), bobPrincipalId))
  .execute(client, 'gtest');
```

Группа адресуется по имени или ID:

|| Форма | Вызов |
|---|---|
| по имени | `admin.groupName('devs')` |
| по ID | `admin.groupId(1)` |

### Удаление группы

```ts
await Batch.create('dropgrp')
  .add('dg', admin.dropGroup(admin.groupName('devs')))
  .execute(client, 'gtest');
```

## 4. Как это работает вместе: типичный сценарий

**Задача:** дать разработчикам (bob) доступ к таблице `shared`,
запретить остальным (carol).

```ts
import { ddl, admin, Batch, connect } from '@shamir/client';

// 1. Создать базу и таблицу (от имени admin)
await Batch.create('setup-db')
  .add('mk', ddl.createDb('gtest'))
  .execute(client, 'default');

const gtest = client.db('gtest');
await gtest.run(ddl.createRepo('main'));
await gtest.run(ddl.createTable('shared', { repo: 'main' }));

// 2. Создать группу
const grpResp = await Batch.create('mkgrp')
  .add('mk', admin.createGroup('devs'))
  .execute(client, 'gtest');
const groupId = grpResp.results.mk.records[0].group_id as number;

// 3. Добавить bob в группу (bob's principal_id = hash(username))
await Batch.create('add-bob')
  .add('am', admin.addGroupMember(admin.groupName('devs'), bobPrincipalId))
  .execute(client, 'gtest');

// 4. Назначить группе таблицу
await Batch.create('chgrp')
  .add('cg', admin.chgrp(admin.refTable('gtest', 'main', 'shared'), groupId))
  .execute(client, 'gtest');

// 5. Установить режим 0o750: owner rwx + group r-x + other ---
await Batch.create('chmod')
  .add('cm', admin.chmod(admin.refTable('gtest', 'main', 'shared'), 0o750))
  .execute(client, 'gtest');
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

`admin.accessTree()` (этаж 3) показывает актуальное дерево с владельцами
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
* **`admin.accessTree()` — read-only.** Не меняет права, только показывает.
* **Удаление пользователя (`admin.dropUser(client, username)`) — HMAC-gated**,
  как и все деструктивные операции (этаж 2).

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «логика переезжает в БД, нужны WASM-функции» | [Этаж 5 — Функции](./05-functions.md) |
| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
