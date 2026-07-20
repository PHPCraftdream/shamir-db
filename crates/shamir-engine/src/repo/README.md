# Repository Management

## Обзор

Модуль `repo` управляет репозиториями — контейнерами таблиц, привязанными
к конкретному storage engine. Один репозиторий = один движок хранения.
Поддерживается 2 движка: InMemory (всегда доступен, используется тестами) и
Fjall (durable production-backend), feature-gated.

## RepoInstance

Управляет таблицами в рамках одного storage backend.

```rust
pub struct RepoInstance {
    repo: BoxRepo,                                      // storage backend
    configs: Arc<DashMap<String, TableConfig>>,          // конфигурации таблиц
    tables: Arc<DashMap<String, OnceCell<TableManager>>>, // lazy-created таблицы
}
```

Таблицы создаются лениво через `OnceCell` — при первом обращении через `get_table()`.

### Ключевые методы

| Метод | Описание |
|-------|----------|
| `new(repo, configs)` | Создать из BoxRepo + конфигурации |
| `from_factory(factory, configs)` | Создать асинхронно через фабрику |
| `get_table(name)` | Получить TableManager (lazy create) |
| `add_table(config)` | Зарегистрировать таблицу |
| `remove_table(name)` | Удалить таблицу |
| `list_table_names()` | Список имён таблиц |
| `create_index(table, name, paths)` | Проксирование к TableManager |
| `create_unique_index(...)` | Проксирование к TableManager |

## BoxRepo

Enum-обёртка над 7 движками хранения, реализующая trait `Repo`:

```rust
pub enum BoxRepo {
    InMemory(Arc<InMemoryRepo>),
    Sled(Arc<SledRepo>),
    Redb(Arc<RedbRepo>),
    Fjall(Arc<FjallRepo>),
    Nebari(Arc<NebariRepo>),
    Persy(Arc<PersyRepo>),
    Canopy(Arc<CanopyRepo>),
}
```

Trait `Repo` предоставляет: `store_get(name)`, `store_delete(name)`, `stores_list()`.

## BoxRepoFactory

Фабрика для асинхронного создания репозиториев. Disk-движки используют
`spawn_blocking` для blocking I/O.

```rust
pub enum BoxRepoFactory {
    InMemory(InMemoryRepoFactory),
    Sled(SledRepoFactory),
    Redb(RedbRepoFactory),
    Fjall(FjallRepoFactory),
    Nebari(NebariRepoFactory),
    Persy(PersyRepoFactory),
    Canopy(CanopyRepoFactory),
}
```

Удобные конструкторы:

```rust
BoxRepoFactory::in_memory()
BoxRepoFactory::sled("./data/sled")
BoxRepoFactory::fjall("./data/fjall")
```

## RepoConfig

Конфигурация для создания репозитория:

```rust
pub struct RepoConfig {
    pub name: String,
    pub factory: BoxRepoFactory,
    pub tables: Vec<TableConfig>,
}
```

Fluent API:

```rust
RepoConfig::new("main", BoxRepoFactory::in_memory())
    .add_table(TableConfig::new("users"))
    .add_table(TableConfig::new("orders"))
```

## Файлы

| Файл | Описание |
|------|----------|
| `repo_instance.rs` | `RepoInstance` — управление таблицами |
| `repo_types.rs` | `BoxRepo`, `BoxRepoFactory`, `RepoFactory` trait, фабрики для 7 движков |
| `repo_config.rs` | `RepoConfig` |
| `mod.rs` | Re-exports |

## Архитектура

```
RepoConfig + BoxRepoFactory
       │
       ▼
  RepoInstance
       │
       ├── BoxRepo (один из 7 движков)
       │     └── store_get("__data__users") → Arc<dyn Store>
       │
       └── DashMap<String, OnceCell<TableManager>>
             └── get_table("users") → lazy init → TableManager
```
