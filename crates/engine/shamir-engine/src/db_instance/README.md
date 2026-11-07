# DbInstance

## Обзор

`DbInstance` управляет набором репозиториев в рамках одной логической базы данных.
Предоставляет доступ к таблицам, управление индексами и DDL операции
(create/drop table). Является промежуточным слоем между `ShamirDb` и `RepoInstance`.

## Иерархия

```
ShamirDb
  └── DbInstance (одна БД)
        ├── RepoInstance "main"
        │     ├── TableManager "users"
        │     └── TableManager "orders"
        └── RepoInstance "archive"
              └── TableManager "logs"
```

## Структура

```rust
pub struct DbInstance {
    repos: Arc<DashMap<String, RepoInstance>>,
}
```

Thread-safe через `DashMap`. Клонирование дешёвое (Arc).

## Ключевые методы

### Управление репозиториями

| Метод | Описание |
|-------|----------|
| `new()` | Пустой экземпляр |
| `with_repos(configs)` | Создание с набором RepoConfig (async) |
| `add_repo(config)` | Добавить репозиторий |
| `remove_repo(name)` | Удалить репозиторий |
| `get_repo(name)` | Получить RepoInstance |
| `list_repos()` | Список имён репозиториев |
| `has_repo(name)` | Проверка существования |

### Доступ к таблицам

| Метод | Описание |
|-------|----------|
| `get_table(repo, table)` | Получить TableManager |
| `create_table(repo, table)` | DDL: создать таблицу |
| `drop_table(repo, table)` | DDL: удалить таблицу |
| `list_tables(repo)` | Список таблиц в репозитории |
| `has_table(repo, table)` | Проверка существования |

### Управление индексами

| Метод | Описание |
|-------|----------|
| `create_index(repo, table, name, paths)` | Создать обычный индекс |
| `create_unique_index(...)` | Создать уникальный индекс |
| `drop_index(repo, table, name)` | Удалить обычный индекс |
| `drop_unique_index(...)` | Удалить уникальный индекс |
| `index_exists(...)` | Проверить наличие обычного индекса |
| `unique_index_exists(...)` | Проверить наличие уникального индекса |
| `lookup_by_index(...)` | Поиск записей по значению индекса |

## Использование

```rust
let db = DbInstance::new();
db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory())
    .add_table(TableConfig::new("users"))
).await?;

let table = db.get_table("main", "users").await?;
```

## Файлы

| Файл | Описание |
|------|----------|
| `db_instance.rs` | Основная реализация `DbInstance` |
| `mod.rs` | Re-export |
