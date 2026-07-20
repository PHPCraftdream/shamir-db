# ShamirDb — Top-Level Database Manager

## Обзор

`ShamirDb` — верхнеуровневый менеджер, объединяющий все компоненты:
управление базами данных, persistent metadata через SystemStore,
и точка входа для выполнения batch-запросов.

## Иерархия

```
ShamirDb
  ├── SystemStore (persistent metadata)
  │     └── databases, repositories, settings, users, roles
  │
  ├── "production" (DbInstance)
  │     └── "main" (RepoInstance)
  │           ├── "users" (TableManager)
  │           └── "orders" (TableManager)
  │
  └── "analytics" (DbInstance)
        └── "archive" (RepoInstance)
              └── "logs" (TableManager)
```

## ShamirDb

```rust
pub struct ShamirDb {
    dbs: Arc<DashMap<String, DbInstance>>,
    system_store: SystemStore,
}
```

### Инициализация

```rust
// Production — с persistent metadata на fjall
let db = ShamirDb::init(SystemStoreConfig::Fjall("./data/system".into())).await?;

// Тесты — in-memory
let db = ShamirDb::init_memory().await?;
```

При инициализации загружает существующие базы и репозитории из SystemStore.

### Ключевые методы

| Метод | Описание |
|-------|----------|
| `init(config)` | Создать с конфигурацией SystemStore |
| `init_memory()` | Создать с in-memory SystemStore |
| `create_db(name)` | Создать БД (+ persist в SystemStore) |
| `get_db(name)` | Получить DbInstance |
| `remove_db(name)` | Удалить БД |
| `list_dbs()` | Список всех БД |
| `add_repo(db, config)` | Добавить репозиторий |
| `remove_repo(db, repo)` | Удалить репозиторий |
| `get_table(db, repo, table)` | Прямой доступ к TableManager |
| `execute(db, request)` | Выполнить BatchRequest |

### execute() — точка входа

```rust
pub async fn execute(
    &self,
    db_name: &str,
    request: &BatchRequest,
) -> Result<BatchResponse, BatchError>
```

Создаёт `DbTableResolver` и `ShamirAdminExecutor`, передаёт в `execute_batch()`.
Поддерживает все операции: read, write, admin (DDL), auth.

## SystemStore

Persistent хранилище метаданных. Использует `DbInstance` + `RepoInstance`
с единственным repo `"system"`.

```rust
pub struct SystemStore {
    db: DbInstance,
}
```

### SystemStoreConfig

| Вариант | Описание |
|---------|----------|
| `InMemory` | Для тестов. Данные теряются при перезапуске |
| `Fjall(PathBuf)` | Production. Персистентное хранение на fjall |

### Системные таблицы

| Таблица | Содержимое |
|---------|-----------|
| `databases` | Метаданные БД (name, created_at) |
| `repositories` | Метаданные репозиториев (db_name, repo_name, engine, path) |
| `settings` | Настройки (key → value) |
| `users` | Пользователи (для auth) |
| `roles` | Роли и permissions (для auth) |

### Ключевые методы

| Метод | Описание |
|-------|----------|
| `init(config)` | Инициализация системного хранилища |
| `save_database(name, record)` | Сохранить метаданные БД |
| `remove_database(name)` | Удалить метаданные БД |
| `load_databases()` | Загрузить все БД |
| `save_repository(db, repo, engine, path)` | Сохранить метаданные репозитория |
| `load_repositories()` | Загрузить все репозитории |
| `save_setting(key, value)` | Сохранить настройку |
| `load_setting(key)` | Загрузить настройку |
| `users_table()` | TableManager для пользователей |
| `roles_table()` | TableManager для ролей |

## Файлы

| Файл | Описание |
|------|----------|
| `shamir_db.rs` | `ShamirDb` — управление БД |
| `system_store.rs` | `SystemStore`, `SystemStoreConfig` |
| `execute.rs` | `ShamirDb::execute()`, `ShamirAdminExecutor`, `DbTableResolver` |
| `mod.rs` | Re-exports |
