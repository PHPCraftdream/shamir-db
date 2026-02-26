# Архитектура слоёв Engine (Layers Architecture)

**Обновлено:** 2026-02-26

## Иерархия управления

```
Dispatcher              → управляет несколькими репозиториями
  └── RepoInstance      → управляет таблицами одного репозитория
      └── TableManager  → управляет одной таблицей + интернер + индексы
          └── IndexManager → управление индексами (реализован)
```

## Ключевой принцип: Интернирование строго на уровне TableManager

**Правило:** Интернер живёт только в TableManager. Все уровни выше используют строковые пути.

**Причина:** Интернер привязан к конкретной таблице (своё пространство ID для каждой таблицы).

### Почему так?

```
Table "users"  → Interner: {"name" → 1, "email" → 2, ...}
Table "orders" → Interner: {"id" → 1, "user_id" → 2, ...}  // Другое пространство!
```

Если интернировать на уровне RepoInstance или Dispatcher — коллизии ID между таблицами.

## API для индексов

### Формат путей на каждом уровне

| Уровень | Входной формат | Преобразование |
|---------|----------------|----------------|
| Dispatcher/RepoInstance | `"email"`, `"user.name"` | Пробрасывает как есть |
| TableManager | `"email"`, `"user.name"` | Интернирует → `Vec<u64>` |
| IndexManager | `Vec<u64>` (уже интернировано) | Работает напрямую |

### Пример API (проектируемый)

```rust
// На уровне Dispatcher/RepoInstance
dispatcher.create_index("main", "users", "email_idx", &["email"]).await?;
dispatcher.create_index("main", "users", "name_city_idx", &["name", "address.city"]).await?;
dispatcher.create_unique_index("main", "users", "email_unique", &["email"]).await?;

// На уровне TableManager
table_manager.create_index("email_idx", &["email"]).await?;
table_manager.create_unique_index("email_unique", &["email"]).await?;

// На уровне IndexManager (текущий API)
let path = vec![IndexInfoItem::new(vec![42])]; // 42 = interned "email"
index_manager.create_index(IndexDefinition::new(name_id, path)).await?;
```

## Текущее состояние

| Компонент | CRUD | Index API | Статус |
|-----------|------|-----------|--------|
| IndexManager | — | ✅ Полный | Готов |
| TableManager | ✅ | ⚠️ Внутренний | Нужен публичный API |
| RepoInstance | — | ❌ | Нужен proxy |
| Dispatcher | — | ❌ | Нужна маршрутизация |

### Что уже работает в TableManager

```rust
// CRUD с автоматическим обновлением индексов
table.insert(&value).await?;  // → index_manager.on_record_created()
table.set(id, &value).await?; // → index_manager.on_record_updated()
table.delete(id).await?;      // → index_manager.on_record_deleted()

// Доступ к IndexManager
table.index_manager() → &IndexManager
```

### Чего не хватает

1. **TableManager** — публичные методы `create_index()`, `drop_index()` со строковыми путями
2. **RepoInstance** — proxy-методы с `(table_name, ...)`
3. **Dispatcher** — маршрутизация с `(repo_name, table_name, ...)`

## План реализации

### Этап 1: TableManager API

```rust
impl TableManager {
    /// Создать обычный индекс
    pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()>;

    /// Создать уникальный индекс
    pub async fn create_unique_index(&self, name: &str, paths: &[&str]) -> DbResult<()>;

    /// Удалить индекс
    pub async fn drop_index(&self, name: &str) -> DbResult<bool>;

    /// Удалить уникальный индекс
    pub async fn drop_unique_index(&self, name: &str) -> DbResult<bool>;

    /// Поиск по индексу (возвращает RecordId)
    pub async fn lookup_by_index(&self, name: &str, values: &[InnerValue]) -> DbResult<BTreeSet<RecordId>>;
}
```

### Этап 2: RepoInstance API

```rust
impl RepoInstance {
    pub async fn create_index(&self, table: &str, name: &str, paths: &[&str]) -> DbResult<()>;
    pub async fn create_unique_index(&self, table: &str, name: &str, paths: &[&str]) -> DbResult<()>;
    pub async fn drop_index(&self, table: &str, name: &str) -> DbResult<bool>;
    // ... etc
}
```

### Этап 3: Dispatcher API

```rust
impl Dispatcher {
    pub async fn create_index(&self, repo: &str, table: &str, name: &str, paths: &[&str]) -> DbResult<()>;
    // ... etc
}
```

## Выгода

- **Dispatcher** — единая точка управления всей БД
- **Единообразный API** — строковые пути на всех уровнях
- **Инкапсуляция** — детали интернирования скрыты
- **Тестируемость** — каждый уровень можно тестировать изолированно
