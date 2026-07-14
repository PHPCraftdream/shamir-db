בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.4 (#244) — RENAME для table / repo / index

Кампания **Phase E**, Track A (DDL-lifecycle). Самая крупная фаза — скоуп
ОГРАНИЧЕН тремя объектами (table, repo, index). db/role/group/folder — follow-on.
Commit-per-object (оркестратор коммитит каждый объект отдельно).

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую дерево/индекс. Только редактируй файлы. НЕ коммить.

## Проблема (заземлено)
rename есть только у function (RenameFunctionOp function_ops.rs:41,
handle_rename_function admin_function.rs:142) и validator (handle_rename_validator
admin_validator.rs:118). Нет для table/repo/index (completeness-ddl G6) →
переименование требует dump/recreate.

## ОБРАЗЕЦ (повторить структуру)
- `handle_rename_function` (admin_function.rs:142) и `handle_rename_validator`
  (admin_validator.rs:118): rekey by name, preserve id. Изучи их ПЕРВЫМИ.
- Каталог-rekey: system_store table record (table_name) — см. как хранится/читается.
- reverse-index: token_names register_token/remove (repo_instance.rs) — при
  переименовании нужно перерегистрировать токен имени.
- access-meta: ResourcePath::Table move (старый путь → новый).

## Сделать — ПО ОДНОМУ ОБЪЕКТУ, в порядке ценности
Каждый объект = полный вертикальный срез + свой integration-тест. Между объектами
прогоняй гейт. (Оркестратор закоммитит каждый объект отдельным коммитом — в отчёте
ЧЁТКО раздели, какие файлы относятся к какому объекту.)

### Объект 1 — RenameTableOp (самый ценный)
- DTO `RenameTableOp { rename_table, to, repo }` (admin/types/table_ops.rs) +
  dispatch (admin_dispatch.rs) + `handle_rename_table`.
- Логика: каталог rekey (system_store table record table_name) + reverse-index
  (token_names register/remove) + access-meta move + СОХРАНИТЬ schema/indexes/
  validators (привязки по table id, не по имени — проверь!). Old имя → пусто,
  new → резолвится, данные/схема/индексы целы.
- Билдер Rust ddl (rename_table) + TS builders/ddl.ts.
- integration-тест: создать таблицу со схемой+данными+индексом → rename →
  старое имя не резолвится, новое резолвится, записи и схема целы, индекс работает.

### Объект 2 — RenameRepoOp
- Аналогично для repo. Учесть, что repo содержит таблицы — rekey repo, таблицы
  внутри сохраняют связь.

### Объект 3 — RenameIndexOp
- Переименование индекса в рамках таблицы. index_manager rekey.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; serde_json::Value запрещён (док-исключения).
- `#[serde(default)]` где уместно для обратной совместимости.
- Тесты ТОЛЬКО через ./scripts/test.sh (bash; raw cargo test заблокирован).
  Узко: ./scripts/test.sh -p shamir-db --full -- rename. Вывод тестов в файл, грепай файл.
- Один файл = один основной export; импорты в шапке; mod.rs только реэкспорты.
- НЕ используй tool под-агентов — пиши/правь сам.
- Платформа Windows, shell bash (Unix-синтаксис, forward slashes, /dev/null).

## Файл-сет
admin/types/{table_ops,repo_ops,index_ops}.rs, admin_dispatch.rs,
admin_table_index.rs / admin_db_repo.rs (handlers), system_store.rs,
repo_instance.rs (reverse-index, если нужно), query-builder ddl/*, TS ddl.ts +
тип, integration-тесты shamir-db. НЕ трогай write.*, docs/research, e2e TS,
ddl.test.ts (там может работать другой агент).

## Гейт (после каждого объекта)
```
cargo fmt -p <touched> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder
```

## Что вернуть
(1) изменённые файлы С РАЗБИВКОЙ по объектам (table/repo/index); (2) контракт
rename (rekey каталога + reverse-index + сохранение schema/indexes); (3) гейт с
числами; (4) что НЕ сделал (если разрослось — какой объект остался). НЕ КОММИТЬ.
Если успел только RenameTable — это ОК, отчитайся, остальное доделаем отдельно.
