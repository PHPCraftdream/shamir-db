בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.7 (#247) — EXPLAIN / dry-run plan

Кампания **Phase E**, Track B (OQL-surface). Финальная фаза. Цель — превью
плана чтения БЕЗ материализации строк.

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую дерево/индекс. Только редактируй файлы. НЕ коммить.

## Проблема (заземлено)
QueryStats post-hoc уже есть (index_used / records_scanned / execution_time_us —
в BatchResponse ПОСЛЕ исполнения). Нет op «показать план ДО/БЕЗ исполнения»
(completeness-oql M5). Трудно тюнить запросы на проде заранее.

## Заземление (перепроверь чтением)
- `read_planner.rs` (shamir-engine, query/read/ или подобное) — выбирает путь:
  try_plan_keyset_seek / try_plan_order_limit_fast_path /
  try_plan_and_range_index_scan / full scan. Изучи, КАК он выбирает и какие
  данные о решении доступны (выбранный индекс, тип плана, оценка scanned).
- QueryStats — структура, как она наполняется и сериализуется в BatchResponse.
- ReadQuery / Query DTO — куда добавить флаг explain.

## Сделать (ПРОСТО — не переусложняй)
1. Флаг `#[serde(default)] explain: bool` на ReadQuery (предпочтительно — меньше
   surface, чем отдельная op). При explain=true — прогнать ТОЛЬКО планировщик
   (выбор пути + индекс + оценка), вернуть план-preview, БЕЗ материализации строк
   (не читать/не возвращать records).
2. Форма ответа: QueryStats-подобный preview — plan_type (keyset_seek /
   order_limit_fast / and_range_index_scan / full_scan), index_used (Option),
   estimated/records_scanned если планировщик это знает до исполнения. Если
   точная оценка недоступна без исполнения — верни тип плана + индекс (это уже
   ценно), пометь оценку как недоступную.
3. Билдеры: Rust (query .explain()) + TS (опционально, если время есть — иначе
   отметь в отчёте как follow-on, TS не обязателен для этой опц. фазы).
4. Тесты: integration — запрос с индексом + explain → план показывает индекс/тип,
   records НЕ материализованы (пустой результат-набор или явный preview-only).

## ВАЖНО — соблюдай простоту
Это ОПЦИОНАЛЬНАЯ фаза. Если EXPLAIN требует глубокой хирургии планировщика
(рефакторинг executor, разделение plan/execute фаз там, где они слиты) — НЕ
делай большой рефактор. Достань минимально: прогони существующий путь
планирования, захвати решение, верни preview, не материализуя/не возвращая
строки. Если даже это требует серьёзного вторжения — реализуй частично и честно
отметь в отчёте, что осталось. Лучше маленький честный EXPLAIN, чем большой
рискованный рефактор.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; serde_json::Value запрещён (док-исключения).
- scc len() запрещён (clippy disallowed-methods).
- `#[serde(default)]` на новом флаге.
- Тесты ТОЛЬКО через ./scripts/test.sh (bash; raw cargo test заблокирован).
  Узко: ./scripts/test.sh -p shamir-db --full -- explain и -p shamir-engine -- explain.
  НЕ грепай вывод тестов inline — пиши в файл, грепай файл.
- Один файл = один основной export; импорты в шапке.
- НЕ используй tool под-агентов — пиши/правь сам.
- Платформа Windows, shell bash (Unix-синтаксис, forward slashes, /dev/null).

## Гейт (прогони сам)
```
cargo fmt -p <touched> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-query-types -p shamir-query-builder
```

## Что вернуть
(1) изменённые файлы; (2) форма explain-preview + как планировщик прогоняется
без материализации; (3) гейт с числами; (4) что осталось (TS? оценки?). НЕ
КОММИТЬ. Финальный текст — отчёт оркестратору.
