# Архитектура слоёв Engine (Layers Architecture)

**Обновлено:** 2026-02-26

## Иерархия управления

```
DbInstance              → управляет несколькими репозиториями
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

Если интернировать на уровне RepoInstance или DbInstance — коллизии ID между таблицами.

## API для индексов

### Формат путей на каждом уровне

| Уровень | Входной формат | Преобразование |
|---------|----------------|----------------|
| DbInstance/RepoInstance | `"email"`, `"user.name"` | Пробрасывает как есть |
| TableManager | `"email"`, `"user.name"` | Интернирует → `Vec<u64>` |
| IndexManager | `Vec<u64>` (уже интернировано) | Работает напрямую |

### Пример API

```rust
// На уровне DbInstance/RepoInstance
db.create_index("main", "users", "email_idx", &["email"]).await?;
db.create_index("main", "users", "name_city_idx", &["name", "address.city"]).await?;
db.create_unique_index("main", "users", "email_unique", &["email"]).await?;

// На уровне TableManager
table_manager.create_index("email_idx", &["email"]).await?;
table_manager.create_unique_index("email_unique", &["email"]).await?;

// На уровне IndexManager (низкоуровневый API)
let path = vec![IndexInfoItem::new(vec![42])]; // 42 = interned "email"
index_manager.create_index(IndexDefinition::new(name_id, path)).await?;
```

## Текущее состояние

| Компонент | CRUD | Index API | Статус |
|-----------|------|-----------|--------|
| IndexManager | — | ✅ Полный | Готов |
| TableManager | ✅ | ✅ Публичный | Готов |
| RepoInstance | — | ✅ Proxy | Готов |
| DbInstance | — | ✅ Маршрутизация | Готов |

### Что работает

```rust
// CRUD с автоматическим обновлением индексов (TableManager)
table.insert(&value).await?;  // → index_manager.on_record_created()
table.set(id, &value).await?; // → index_manager.on_record_updated()
table.delete(id).await?;      // → index_manager.on_record_deleted()

// Index API на всех уровнях
table_manager.create_index("email_idx", &["email"]).await?;
repo_instance.create_index("users", "email_idx", &["email"]).await?;
db.create_index("main", "users", "email_idx", &["email"]).await?;
```

## Архитектура

### Файловая структура

```
src/
├── db_instance/
│   └── db_instance.rs    → DbInstance
├── repo/
│   └── repo_instance.rs → RepoInstance
├── table/
│   └── table_manager.rs → TableManager
└── index/
    └── index_manager.rs → IndexManager
```

### Поток вызовов

```
DbInstance::create_index(repo, table, name, paths)
    ↓
RepoInstance::create_index(table, name, paths)
    ↓
TableManager::create_index(name, paths)
    ↓ (интернирование)
IndexManager::create_index(definition)
```
