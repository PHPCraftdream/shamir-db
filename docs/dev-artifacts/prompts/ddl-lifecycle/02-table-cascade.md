בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.2 (#242) — table-level `cascade` на drop (таблица + индексы/валидаторы/схема)

Кампания **Phase E — Completeness & Operability**, Track A (DDL-lifecycle).
Stage 2 из 9. Strategy: single-context, commit-per-phase. blockedBy E.1 (УЖЕ
СДЕЛАНА — общий drop-handler surface; E.1 добавила `if_exists` на DropTableOp).

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую рабочее дерево/индекс. Только редактируй файлы;
коммитит оркестратор. (2026-06-24 агент сделал `git reset --hard` и стёр часы
работы.)

## Контекст E.1 (только что закоммичена — НЕ ломай)
DropTableOp теперь имеет `#[serde(default)] pub if_exists: bool` и early-exit
в `handle_drop_table` (admin_table_index.rs ~106): при if_exists + отсутствие
таблицы/db → no-op existed:false, ДО auth-guard. Твой `cascade` добавляется
рядом, не конфликтуя.

## Цель
`cascade` сейчас есть ТОЛЬКО на db/repo (DropDbOp.cascade, DropRepoOp.cascade).
На DropTableOp нет — дроп таблицы с индексами/валидаторами требует ручной
зачистки (completeness-ddl G2). Прецедент — `handle_drop_db` (admin_db_repo.rs
~50): при cascade удаляет repos→tables→validators в одной op.

## Заземление (file:line, перепроверь)
- DropTableOp DTO — `crates/shamir-query-types/src/admin/types/table_ops.rs`
  (там же где if_exists из E.1).
- handle_drop_table — `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`
  (~106). Уже использует очистку валидаторов `bound_in` (см.
  `drop_table_cleaning_validators` / вызов на ~150 по плану) + index_manager.
- Прецедент cascade — `handle_drop_db` (admin_db_repo.rs ~50).
- Phase D.3 drop-guard `drop_refused_fk` — УЖЕ есть в handle_drop_table; cascade
  его НЕ обходит (см. ниже).
- Rust билдер — `crates/shamir-query-builder/src/ddl/drop_table.rs` (там же
  `.if_exists()` из E.1).
- TS билдер — `crates/shamir-client-ts/src/core/builders/ddl.ts` (dropTable).
- TS тип — `crates/shamir-client-ts/src/core/types/ddl.ts` (DropTableOp).

## Сделать
1. Добавить `#[serde(default)] pub cascade: bool` в DropTableOp.
2. handle_drop_table: при `cascade=true` — атомарно снять привязанные
   валидаторы (bound_in) + дропнуть СВОИ индексы + схему, ПОТОМ таблицу.
   Без cascade — текущее поведение.
   - ⚠️ ВАЖНО: cascade зачищает только СВОИ артефакты таблицы. Он НЕ обходит
     reverse-FK guard (`drop_refused_fk`, Phase D.3) от ЧУЖИХ таблиц,
     ссылающихся на эту. Если на таблицу ссылается чужой FK с RESTRICT —
     дроп должен по-прежнему отказывать, даже с cascade. (cascade здесь =
     «снеси мои индексы/валидаторы/схему», НЕ «снеси всё что на меня ссылается».)
   - Опирайся на существующую `drop_table_cleaning_validators` (уже зовётся при
     обычном дропе) + index_manager для индексов.
3. Билдеры: Rust `drop_table.rs` (`.cascade()`), TS `ddl.ts` (opts.cascade) +
   тип в `ddl.ts`. Паритет Rust↔TS.
4. Тесты (через ./scripts/test.sh):
   - integration (shamir-db, файл `crates/shamir-db/tests/ddl_wire_e2e/idempotency_cascade.rs`
     — там уже есть cascade-тесты для db/repo, добавь table-cascade рядом):
     таблица с индексом+валидатором+схемой → drop cascade снимает всё, одна op,
     existed:true; без cascade при наличии своих артефактов — текущее поведение.
   - Если есть чужой RESTRICT-FK на таблицу → drop cascade всё равно отказывает
     (guard не обойдён). Если воспроизвести FK-сценарий дорого — хотя бы
     коммент-обоснование, что guard перед cascade-зачисткой.
   - TS unit wire-shape (ddl.test.ts): поле cascade сериализуется.

## Дисциплина проекта (как в E.1)
- Запросы — ТОЛЬКО query-builder; `serde_json::Value` запрещён (док-исключения).
- `#[serde(default)]` на новом поле обязателен.
- Тесты ТОЛЬКО через `./scripts/test.sh` (raw cargo test заблокирован). Узко:
  `./scripts/test.sh -p shamir-db --full -- cascade` и
  `./scripts/test.sh -p shamir-query-types -p shamir-query-builder`.
  НЕ грепай вывод тестов inline — пиши в файл, грепай файл.
- Один файл = один основной export; импорты в шапке; mod.rs только реэкспорты.
- Платформа: Windows, shell = bash (Unix-синтаксис, forward slashes).

## Гейт перед сдачей (прогони сам)
```
cargo fmt -p <touched> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder
```

## Что вернуть оркестратору
Структурно: (1) изменённые файлы; (2) контракт cascade как реализовал
(особенно взаимодействие с drop_refused_fk guard); (3) результат гейта с
числами; (4) отклонения от брифа. НЕ КОММИТЬ. Твой финальный текст — отчёт
оркестратору.
