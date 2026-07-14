בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.8 (#248) — e2e для FTS / vector / call (headline-фичи)

Кампания **Phase E — Completeness & Operability**, Track C (headline-e2e).
Независима. Закрывает C1 (coverage-ts-tests P0).

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую рабочее дерево/индекс. Только редактируй файлы;
коммитит оркестратор. (2026-06-24 агент сделал `git reset --hard` и стёр часы.)

## Твой файл-сет (НЕ выходи за него — параллельно работают другие агенты)
- НОВЫЕ e2e тест-файлы в `crates/shamir-client-ts/src/__tests__/`
- при необходимости — расширения `crates/shamir-client-ts/src/__tests__/e2e-harness.ts`
  (только аддитивные хелперы, не ломай существующее)
НЕ трогай: Rust-крейты, ddl/write билдеры (.ts core/builders), DTO — там другие
агенты. Тебе нужны ТОЛЬКО e2e-тесты через клиентский билдер + сервер.

## Проблема (заземлено)
НИ ОДИН TS e2e не создаёт FTS- или vector-индекс и не зовёт stored-функцию через
call() через сервер. grep по `__tests__/*.ts` на createIndex(fts)/vectorSimilarity/
.fts(/.call( — пусто. Engine-уровень покрыт (Rust), но клиент→сервер путь — нет.
Серде-регрессия в Fts/VectorSimilarity/CallOp пройдёт юниты и тихо сломает фичу.

## Сделать (по e2e-кейсу на каждую, через свежесобранный release-сервер)
1. **FTS**: createIndex(fts) на текстовом поле + insert + fts-query (filter.fts) +
   assert совпадения/токенов.
2. **Vector**: createIndex(vector) + insert векторов + top-k similarity query +
   assert порядок.
3. **call**: createFunction + call() (CallOp) + assert result.

Использовать `e2e-harness.ts` (startServer/connectAdmin/setupDb — изучи
существующие e2e-файлы как образец).

## Сервер (важно — из прошлой сессии)
- Сборка: `cargo build --release -p shamir-server`. Бинарь окажется в
  `CARGO_TARGET_DIR` (профиль шелла ставит `D:\dev\rust\.cargo-target`), т.е.
  `.cargo-target/release/shamir-server.exe`. harness (`serverBinPath()`) берёт
  его первым. Сборка release ~15-20 мин — наберись терпения, не прерывай.
- createIndex signature = `createIndex(name, table, [['field']], opts)`.
- `dropTable(signer, db, repo, table)` требует HMAC-signer.
- ТОЛЬКО builder (никакого raw `{from,where} as any`).

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО клиентский query-builder. Никакого raw JSON / `as any`.
- Тесты гоняй через vitest: `cd crates/shamir-client-ts && npx vitest run <file>`.
  НЕ грепай вывод тестов inline бесконтрольно — при нужде пиши в файл, грепай файл.
- Если сервер уже собран в `.cargo-target/release/shamir-server.exe` и свежий —
  не пересобирай зря.
- Если фича на сервере отсутствует/сломана (не серде, а реальный баг движка) —
  НЕ чини движок (это вне скоупа), а зафиксируй тест как точную репро-точку и
  отчитайся оркестратору с деталями.

## Гейт перед сдачей
- Каждый новый e2e-файл проходит `npx vitest run <file>` зелёным против
  свежесобранного release-сервера. Приложи вывод (pass-счётчики).
- Если какой-то e2e падает из-за реального серверного бага — отчитайся отдельным
  разделом «BLOCKED: <фича> — <деталь>», тест оставь (skipIf или as-is с пояснением).

## Что вернуть
(1) новые/изменённые файлы; (2) по каждой фиче — green/blocked + деталь;
(3) вывод vitest с числами; (4) отклонения. НЕ КОММИТЬ — коммитит оркестратор.
Заверши финальным assistant-сообщением с этим отчётом.
