# Authorization & Role-Based Access Control

## Overview

ShamirDB поддерживает row-level security через систему ролей и permissions.
Каждый permission определяет: эффект (allow/deny), действия, ресурс, и опционально
фильтр на уровне записей (row-level security).

## Resource — на что распространяется право

Иерархия ресурсов (от общего к частному):

```json
{"scope": "global"}
{"scope": "database", "database": "mydb"}
{"scope": "repo",     "database": "mydb", "repo": "main"}
{"scope": "table",    "database": "mydb", "repo": "main", "table": "users"}
```

Более общий scope покрывает все вложенные. `global` покрывает всё.
`database: mydb` покрывает все repo и таблицы внутри `mydb`.

### Специфичность

| Scope | Уровень |
|-------|---------|
| global | 0 |
| database | 1 |
| repo | 2 |
| table | 3 |

## Action — какое действие разрешено/запрещено

| Action | Покрывает |
|--------|-----------|
| `read` | SELECT (from) |
| `insert` | INSERT (insert_into) |
| `update` | UPDATE, SET (update, set) |
| `delete` | DELETE (delete_from) |
| `create` | create_repo, create_table, create_index |
| `drop` | drop_repo, drop_table, drop_index |
| `manage_users` | create_user, drop_user, grant_role, revoke_role |
| `manage_roles` | create_role, drop_role |
| `all` | все вышеперечисленные |

## Permission — единица права

```json
{
  "effect": "allow",
  "actions": ["read", "insert"],
  "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "orders"},
  "where": {"op": "eq", "field": ["region"], "value": "europe"}
}
```

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `effect` | `"allow"` / `"deny"` | да | Разрешить или запретить |
| `actions` | [Action] | да | Список действий |
| `resource` | Resource | да | На какой ресурс распространяется |
| `where` | Filter | нет | Row-level security фильтр |

### where (Row-Level Security)

Если указан `where`, право действует только на записи, проходящие фильтр.
Используется тот же синтаксис фильтров, что и в запросах.

### $user — ссылка на поля текущего пользователя

Пользователь — это документ-объект с произвольными полями:

```json
{
  "name": "alice",
  "office": {"id": "eu-west-1", "region": "europe"},
  "department": "sales",
  "level": 3
}
```

В `where` фильтрах permissions можно ссылаться на поля пользователя через `$user`:

```json
{"$user": ["office", "id"]}
{"$user": ["department"]}
{"$user": ["level"]}
```

Путь — массив строк (тот же формат что FieldPath). Резолвится в рантайме из документа текущего пользователя.

#### Примеры

Менеджер видит только заказы своего офиса:

```json
{
  "effect": "allow",
  "actions": ["read", "update"],
  "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "orders"},
  "where": {"op": "eq", "field": ["office_id"], "value": {"$user": ["office", "id"]}}
}
```

Сотрудник видит только свои записи:

```json
{
  "where": {"op": "eq", "field": ["owner"], "value": {"$user": ["name"]}}
}
```

Доступ по уровню: видит записи с level ≤ своего:

```json
{
  "where": {"op": "lte", "field": ["required_level"], "value": {"$user": ["level"]}}
}
```

Комбинация с литералами:

```json
{
  "where": {
    "op": "and",
    "filters": [
      {"op": "eq", "field": ["department"], "value": {"$user": ["department"]}},
      {"op": "ne", "field": ["status"], "value": "archived"}
    ]
  }
}
```

Поведение по типу операции:

| Операция | Поведение `where` |
|----------|-------------------|
| read | Автоматически добавляется к WHERE запроса (AND) |
| update | Добавляется к WHERE + обновляются только matching записи |
| delete | Добавляется к WHERE + удаляются только matching записи |
| insert | Вставляемая запись валидируется — должна проходить фильтр |

## Role — именованный набор permissions

```json
{
  "name": "regional_manager_eu",
  "permissions": [
    {
      "effect": "allow",
      "actions": ["read"],
      "resource": {"scope": "global"}
    },
    {
      "effect": "allow",
      "actions": ["read", "update", "insert"],
      "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "orders"},
      "where": {"op": "eq", "field": ["region"], "value": "europe"}
    },
    {
      "effect": "deny",
      "actions": ["delete", "drop"],
      "resource": {"scope": "global"}
    }
  ]
}
```

## Встроенные роли

| Роль | Permissions |
|------|------------|
| `superadmin` | allow all on global |
| `readonly` | allow read on global |
| `readwrite` | allow read, insert, update, delete on global |

## Разрешение конфликтов

Пользователь имеет несколько ролей → все permissions объединяются.

### Алгоритм `check_permission(permissions, action, resource)`

1. **Собрать** все permissions из всех ролей пользователя
2. **Отфильтровать** по action: permission.actions содержит запрошенный action (или `all`)
3. **Отфильтровать** по resource: permission.resource покрывает запрошенный resource
4. **Выбрать** самый специфичный resource match (table > repo > database > global)
5. **При одинаковой специфичности**: deny побеждает allow
6. **Нет match** → deny (secure by default)

### Merge where-фильтров из нескольких ролей

Если несколько ролей дают allow на один ресурс с разными where-фильтрами:

```
role_a: allow read on orders WHERE region = "europe"
role_b: allow read on orders WHERE region = "asia"
```

Результат: `OR(region = "europe", region = "asia")` — пользователь видит оба региона.

Если одна роль без where, а другая с where:

```
role_a: allow read on orders (без where)
role_b: allow read on orders WHERE region = "europe"
```

Результат: без ограничений (role_a даёт полный доступ к таблице).

## Управление через JSON API

### Users

```json
{"create_user": "alice", "password": "...", "roles": ["readonly"]}
{"drop_user": "alice"}
{"grant_role": "analyst", "user": "alice"}
{"revoke_role": "analyst", "user": "alice"}
{"list": "users"}
```

### Roles

```json
{
  "create_role": "analyst",
  "permissions": [
    {"effect": "allow", "actions": ["read"], "resource": {"scope": "global"}},
    {"effect": "allow", "actions": ["insert"], "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "reports"}}
  ]
}
{"drop_role": "analyst"}
{"list": "roles"}
```

## Примеры

### Аналитик: читает всё, пишет только в reports

```json
{
  "create_role": "analyst",
  "permissions": [
    {"effect": "allow", "actions": ["read"], "resource": {"scope": "global"}},
    {"effect": "allow", "actions": ["insert", "update"], "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "reports"}}
  ]
}
```

### Региональный менеджер: CRUD только по своему региону (через $user)

```json
{
  "create_role": "regional_manager",
  "permissions": [
    {"effect": "allow", "actions": ["read"], "resource": {"scope": "global"}},
    {
      "effect": "allow",
      "actions": ["read", "insert", "update", "delete"],
      "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "orders"},
      "where": {"op": "eq", "field": ["region"], "value": {"$user": ["office", "region"]}}
    }
  ]
}
```

Одна роль для всех менеджеров. Каждый видит свой регион на основе своего профиля.

### Аудитор: только чтение, только архив

```json
{
  "create_role": "auditor",
  "permissions": [
    {"effect": "allow", "actions": ["read"], "resource": {"scope": "repo", "database": "mydb", "repo": "archive"}}
  ]
}
```

### Запрет удаления для всех кроме admin

```json
{
  "create_role": "no_delete",
  "permissions": [
    {"effect": "deny", "actions": ["delete", "drop"], "resource": {"scope": "global"}}
  ]
}
```
Назначить всем пользователям роль `no_delete`. Admin с ролью `superadmin` перекроет deny
более специфичным allow (если настроен на конкретную таблицу).
