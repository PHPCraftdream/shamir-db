בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# e2e-fix — 6 pre-existing падений Node e2e (#421)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #421:
> `cd tests/e2e && npm test` даёт 122 passed / 6 failed. Все 6 — дрейф
> тестов/инфры после репликационной кампании (Jun 23 → Jul), НЕ векторные.
> Server-бинарь свежий: target/release/shamir-server.exe (Jul 5); харнесс
> tests/e2e/helpers/server.js его и спавнит.

## Падения и направления

1. **10-multi-db «drop A leaves B intact»** и **12-hmac-gate «drop_db with
   correct hmac succeeds»**: `db error [still_referenced]: cannot drop
   database: still has repositories: ["main"]`. Сначала проверь git log /
   код сервера: интенциональна ли новая integrity-проверка drop_db
   (похоже, да — из репликационной работы). Если интенциональна — обнови
   ОБА теста: дропать репозитории перед drop_db (найди wire-op drop_repo /
   drop_repository в серверном коде), и проверь, что негативный сценарий
   (drop непустой → still_referenced) тоже покрыт тестом (это новый
   контракт — закрепи его). Если НЕинтенциональна — красный тест + отчёт,
   НЕ чини сервер молча.
2. **16-replication «ReplHello as plain user → error / bad_role»**:
   `transport: read challenge: io: early eof` — клиент не дочитывает
   отказ/сервер рвёт соединение раньше. Разберись: это тест-хрупкость
   (нужно ждать/читать иначе) или серверный дефект вежливого отказа.
3. **17-replication-convergence setup**: `Database 'app' already exists` —
   cross-test утечка (`app` не изолирована между прогонами/файлами).
   Найди где создаётся `app` и изолируй (уникальное имя как fixtures.setupDb
   в других файлах, либо teardown). Два 30s-таймаута follower —
   каскад от setup; после фикса изоляции проверь, что convergence
   реально сходится.

## Гейт

- `cd tests/e2e && npm test` → 128/128 зелёный (полный suite, включая
  18-vectors), минимум 2 прогона подряд (изоляционные баги любят второй
  прогон).
- Rust НЕ менять без явной находки серверного дефекта (тогда красный тест
  + отчёт). Если правишь только JS-тесты — Rust-гейт не нужен.

## Дисциплина

- Менять только файлы tests/e2e/**. Каждый корень → отдельно назван в
  финале. stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

npm test 128/128 два прогона подряд; финал: корень каждого из 6 падений
(интенционально/дефект/хрупкость), изменённые файлы, вывод обоих прогонов.
