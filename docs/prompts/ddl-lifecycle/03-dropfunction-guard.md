בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.3 (#243) — DropFunction-as-validator guard (referential lifecycle)

Кампания **Phase E**, Track A (DDL-lifecycle). Независима (отдельный handler).
Закрывает остаток A3/G3. Прямой аналог Phase D.3 drop-guard.

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую дерево/индекс. Только редактируй файлы; коммитит
оркестратор. НЕ коммить.

## Проблема (заземлено)
DropFunction НЕ отказывает, если функция привязана как валидатор → silent
dangling reference. DropValidator уже отказывает при bound_in≠∅
(admin_validator.rs ~78). Нужен симметричный guard на функции.

## Заземление (file:line, перепроверь)
- handle_drop_function — `crates/shamir-db/src/shamir_db/execute/admin_function.rs` (~77).
  Уже имеет `if_exists` (из E.1) — guard добавляется ПОСЛЕ if_exists-early-exit,
  но ПЕРЕД фактическим удалением.
- handle_drop_validator (admin_validator.rs ~78) — образец поиска bound_in:
  как он находит, куда привязан валидатор. Переиспользуй тот же registry-обход.
- Связь function→validator: функция-как-валидатор регистрируется через
  bind_validator с function-backed validator. Изучи, как validator_id связан с
  function (по имени/id), чтобы найти все привязки данной функции.

## Сделать
1. handle_drop_function: перед удалением проверить, не привязана ли функция как
   валидатор где-либо (обход validator registry bound_in по аналогии с
   DropValidator). Если привязана — `Err` с кодом "drop_refused_bound" и
   информативным сообщением (имя функции + где привязана: table/db/repo).
2. `if_exists` (из E.1) НЕ обходит guard: если функции нет — if_exists даёт no-op;
   если функция есть и привязана — guard отказывает даже при if_exists.
3. Билдер: DropFunction уже есть (E.1 перевёл на builder-паттерн с .if_exists()) —
   guard серверный, билдер менять не нужно, если только не требуется флаг force
   (НЕ добавляй force без явной необходимости — отказ должен быть дефолтным).
4. Тесты: integration (через ./scripts/test.sh) — создать функцию, привязать как
   валидатор (bind_validator с function-backed), drop function → drop_refused_bound;
   unbind → drop проходит. Образец паттерна — declarative_schema/функц.-тесты или
   ddl_wire_e2e. Изучи существующие function/validator тесты как шаблон.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; `serde_json::Value` запрещён (док-исключения).
- Тесты ТОЛЬКО через `./scripts/test.sh` (bash; raw cargo test заблокирован).
  Узко: `./scripts/test.sh -p shamir-db --full -- drop_refused` или по имени теста.
  НЕ грепай вывод тестов inline — пиши в файл, грепай файл.
- Один файл = один основной export; импорты в шапке.
- НЕ используй tool под-агентов — пиши/правь сам.
- Платформа Windows, shell bash (Unix-синтаксис, forward slashes).

## Файл-сет (параллельно работает другой агент над ddl.test.ts + доками — НЕ трогай их)
admin_function.rs (+ возможно admin_validator.rs для переиспользования хелпера) +
Rust integration-тест (новый или существующий function/validator e2e). НЕ трогай
ddl.test.ts, docs/research/*.

## Гейт (прогони сам)
```
cargo fmt -p shamir-db -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db
```

## Что вернуть
(1) изменённые файлы; (2) контракт guard (как находит bound_in, взаимодействие с
if_exists); (3) гейт с числами; (4) отклонения. НЕ КОММИТЬ. Финальный текст —
отчёт оркестратору.
