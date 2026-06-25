בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.6 (#246) — DESCRIBE / SHOW CREATE (полная форма объекта)

Кампания **Phase E**, Track B (OQL-surface). Близнец keyset — собрать из уже
существующих reads.

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую дерево/индекс. Только редактируй файлы. НЕ коммить.

## Проблема (заземлено)
list_* отдаёт только имена (handle_list admin_list.rs:14). get_table_schema —
ТОЛЬКО схему (handle_get_table_schema admin_schema.rs:559). Нет одной op,
возвращающей полную форму таблицы: schema + indexes + validators + retention +
buffer + owner/mode (completeness-ddl G5). Машинерия есть — нужно скомпоновать.

## Сделать
1. Новый `DescribeTableOp { describe_table, repo }` (admin/types/table_ops.rs) +
   dispatch (admin_dispatch.rs) + `handle_describe_table` — скомпоновать в один
   Map-ответ:
   - **schema**: как get_table_schema (admin_schema.rs:559, serialise_rules_flat).
   - **indexes**: index_manager list (см. как handle_list_indexes / index_manager
     перечисляет индексы).
   - **validators**: validator_bindings + registry (как list validators для таблицы).
   - **retention / buffer**: config таблицы (найди, где читается retention/buffer).
   - **owner / mode**: access meta (ResourcePath::Table — как читается owner/mode).
   Все источники уже читаются в других handlers — переиспользуй, не дублируй логику.
2. Гейт доступа: Action::Read на таблицу (как get_table_schema).
3. Билдеры: Rust ddl (describe_table) + TS builders/ddl.ts.
4. integration-тест: создать таблицу со схемой+индексом+валидатором+retention →
   describe → все секции присутствуют и верны.

## Примечание про builder-only
Ответ DESCRIBE — admin-introspection (reference-форма объекта), не
query-builder query. Это документированное исключение из builder-only (как
client-server-protocol-spec — форма, которую builder и так производит). Запрос
DescribeTableOp всё равно строится через query-builder (b.describe_table(...)).

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; serde_json::Value запрещён (док-исключения с комментом).
- Тесты ТОЛЬКО через ./scripts/test.sh (bash; raw cargo test заблокирован).
  Узко: ./scripts/test.sh -p shamir-db --full -- describe. Вывод тестов в файл, грепай файл.
- Один файл = один основной export; импорты в шапке; mod.rs только реэкспорты.
- НЕ используй tool под-агентов — пиши/правь сам.
- Платформа Windows, shell bash (Unix-синтаксис, forward slashes, /dev/null).

## Файл-сет
admin/types/table_ops.rs (DescribeTableOp), admin_dispatch.rs, новый
handle_describe_table (admin_schema.rs или новый admin-файл — следуй
«один-файл-один-export»), query-builder ddl/*, TS ddl.ts + тип, integration-тест.
НЕ трогай write.*, docs/research, e2e TS, ddl.test.ts (другой агент).

## Гейт
```
cargo fmt -p <touched> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder
```

## Что вернуть
(1) изменённые файлы; (2) форма DescribeTable-ответа (какие секции, из каких
источников); (3) гейт с числами; (4) отклонения. НЕ КОММИТЬ. Финальный текст —
отчёт оркестратору.
