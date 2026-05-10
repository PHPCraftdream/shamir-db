# Admin (DDL) Operations

## Обзор

Модуль `admin` определяет типы для операций DDL (Data Definition Language) —
создание и удаление баз данных, репозиториев, таблиц и индексов, а также листинг сущностей.

Все DDL операции выполняются через `AdminExecutor` trait в batch executor.
Каждая операция — это отдельная структура, сериализуемая из JSON.

## Типы операций

| Структура | JSON-ключ | Описание |
|-----------|-----------|----------|
| `CreateDbOp` | `create_db` | Создать базу данных |
| `DropDbOp` | `drop_db` | Удалить базу данных |
| `CreateRepoOp` | `create_repo` | Создать репозиторий (engine + tables) |
| `DropRepoOp` | `drop_repo` | Удалить репозиторий |
| `CreateTableOp` | `create_table` | Создать таблицу в репозитории |
| `DropTableOp` | `drop_table` | Удалить таблицу |
| `CreateIndexOp` | `create_index` | Создать индекс (regular/unique) |
| `DropIndexOp` | `drop_index` | Удалить индекс |
| `ListOp` | `list` | Листинг (databases/repos/tables/indexes) |

## JSON примеры

### Базы данных

```json
{"create_db": "mydb"}
{"drop_db": "mydb"}
```

### Репозитории

```json
{"create_repo": "hot_cache", "engine": "in_memory", "tables": ["sessions", "tokens"]}
{"drop_repo": "hot_cache"}
```

`engine` по умолчанию `"in_memory"`. Для disk-движков требуется `path`.

### Таблицы

```json
{"create_table": "products", "repo": "main"}
{"drop_table": "products", "repo": "main"}
```

`repo` по умолчанию `"main"`.

### Индексы

```json
{"create_index": "email_idx", "table": "users", "fields": [["email"]], "unique": true}
{"drop_index": "email_idx", "table": "users"}
```

### Листинг

```json
{"list": "databases"}
{"list": "repos"}
{"list": "tables", "repo": "main"}
{"list": "indexes", "table": "users", "repo": "main"}
```

`ListOp` — tagged enum по полю `list` с вариантами: `Databases`, `Repos`, `Tables`, `Indexes`.

## Выполнение

DDL операции обрабатываются через `AdminExecutor` trait (определён в `batch`):

```rust
#[async_trait]
pub trait AdminExecutor {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError>;
}
```

Реализация `ShamirAdminExecutor` в модуле `shamir_db/execute.rs` маршрутизирует
каждую операцию к соответствующему методу `ShamirDb` / `DbInstance`.

## Файлы

| Файл | Описание |
|------|----------|
| `types.rs` | Все DDL структуры и `ListOp` enum |
| `mod.rs` | Публичные re-exports |

## Архитектура

```
BatchRequest → BatchOp::CreateDb/DropDb/... → AdminExecutor → ShamirDb/DbInstance
```

DDL операции не используют `$query` зависимости и всегда выполняются последовательно.
