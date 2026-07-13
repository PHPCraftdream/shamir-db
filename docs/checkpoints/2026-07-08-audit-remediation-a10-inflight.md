# Checkpoint — 2026-07-08 [audit-remediation-a10-inflight]

## Session summary
Продолжаем кампанию по устранению находок 5-агентного аудита `docs/audits/2026-07-06-*.md`, по инструкции пользователя: последовательно, через `/crush` для реализации и `@sh` (Agent) для адверсариального ревью, коммит после ревью и правок. Все CRITICAL закрыты ранее. Внутри задачи HIGH-concurrency-isolation (MVCC/SSI кластер) в этой сессии закрыты и закоммичены: **A6** (lock-upgrade Shared→Exclusive игнорировал других держателей, коммит `9e770df5`), **A8** (interner-delta терялся при аборте первого toucher'а, коммит `f9a2bba2`), **A9** (online CREATE/RENAME INDEX терял запись в окне backfill→registration, коммит `8ee0f6b6`). Все три прошли полный цикл: бриф → коммит брифа → `/crush` → независимая проверка диффа/тестов/гейтов оркестратором → `@sh` ревью → коммит.

**A10 (vacuum TOCTOU vs open_snapshot) — НЕ закрыт, в работе прямо сейчас, потребовал 3 раунда правок.** Бриф закоммичен (`16fe21dd`). Раунд 1 (register-then-verify-then-reconcile на стороне читателя + refcount) был отклонён `@sh` ревью — не закрывал основную гонку (окно между чтением `last_committed()` и завершением регистрации остаётся уязвимым на реальном многопоточном рантайме). Раунд 2 (vacuum-side anchor deferral — всегда хранить одну предыдущую версию как якорь) тоже отклонён — закрывает гонку только для ОДНОГО цикла записи; читатель, застрявший на двух и более записях подряд, всё ещё ловит баг. Раунд 3 — отправлен crush в ту же сессию `vacuum-toctou-a10` с точным заданием реализовать **настоящий in-flight barrier**: атомарный счётчик `active_snapshots_opening`, инкрементируемый ДО чтения floor и декрементируемый только ПОСЛЕ завершения регистрации (RAII, cancel-safe), и `vacuum_key`'s fast path должен требовать `active_snapshots_opening == 0` в дополнение к `active_snapshots_empty()` перед любым физическим удалением — это закрывает гонку безусловно, независимо от количества циклов записи. Сессия ещё выполняется (видны свежие лог-файлы `a10_concurrent_proof.log`, `tx_barrier.log` и т.д. в корне репо) — результат ещё не проверен оркестратором.

**Аномалия сессии:** инструмент `TaskList` сейчас возвращает "No tasks found", хотя ранее в этой сессии список содержал 17 активных задач (#481-497, пересозданных по просьбе пользователя из старых #447-480 для решения проблемы с отображением в UI-панели) плюс ~30 завершённых. Причина неясна — возможно, тот же класс рассинхронизации с UI-панелью, о котором сообщал пользователь ранее. `CronList` подтверждает, что babysit-cron (`8ec51d05`, `7,27,47 * * * *`, session-only) по-прежнему активен и его тик недавно репортил "still running #481", то есть до этого расхождения задача #481 существовала и была in_progress.

Правило "никогда не терять контроль завершения" (записанное ранее в `CLAUDE.md` в этой же сессии) и правило "не запускать `crush run` в фон через `&`/`disown`" соблюдались: все запуски делались как голая команда с `run_in_background: true` на самом Bash-вызове, без обёрточного shell.

## Active goal
none (используется `/babygoal`, не `/goal`)

## TaskList
**Инструмент TaskList вернул "No tasks found" на момент записи чекпоинта — расхождение с ожидаемым состоянием, не проверено до конца.** Последнее известное валидное состояние (до аномалии):

### in_progress (последнее известное)
- #481 HIGH-concurrency-isolation: MVCC/SSI кластер (A2/A3/A4/A6/A7/A8/A9 закрыты; A10 в работе, 3-й раунд правок; A11-A14 не начаты)

### pending (последнее известное, 16 шт.)
- #482 CLEANUP, #483-485 COMPLIANCE-1/2/3, #486-490 PERF-RADICAL-1..5, #491 PERF-RADICAL-STRUCTURAL, #492 FLAKE, #493 bench migration, #494-497 residual-кластеры (durability/security/perf/client)

## Decisions
- Каждый фикс проходит: бриф → коммит брифа отдельно → `/crush` → независимая проверка diff+тестов оркестратором → `@sh` ревью → коммит с подробным сообщением.
- Для A10 принято решение НЕ коммитить после первых двух раундов `@sh` вынес "DO NOT SHIP" — вместо того чтобы принять частичное закрытие гонки, оркестратор отправлял точные корректирующие задания в ТУ ЖЕ crush-сессию (не новую), явно называя, что именно не так и что нужно реализовать взамен.
- `crush run` теперь ВСЕГДА запускается голой командой с `run_in_background: true` на самом Bash-вызове — без `&`/`disown` в конце команды (это давало ложные "completed" уведомления раньше срока реального завершения).
- При транзиентном сетевом сбое crush-сессии (например, timeout к sourcegraph.com) — перезапуск в ТУ ЖЕ `--session`, а не откат на fallback-агента (это не исчерпание лимита).

## Open questions
- **A10 раунд 3 (in-flight barrier)**: ещё выполняется на момент чекпоинта — результат не проверен. При возобновлении: проверить `crush sessions locks vacuum-toctou-a10` / `crush sessions last vacuum-toctou-a10 --n 1`, прочитать диф самостоятельно, перегнать гейты/тесты, отправить на `@sh` (акцент на: закрывает ли счётчик `active_snapshots_opening` гонку БЕЗУСЛОВНО для произвольного числа циклов записи, корректны ли Acquire/Release ordering, не течёт ли счётчик при отмене future).
- **Аномалия TaskList "No tasks found"**: не диагностирована. Требуется либо ручное пересоздание задач заново (как уже делалось раз в этой сессии по просьбе пользователя), либо выяснение первопричины расхождения с UI.
- A11-A14 (`docs/audits/2026-07-06-concurrency-engine.md`) ещё не начаты: A11 (recovery `wal.commit` без A5-гейта/персиста интернера), A12 (`apply_replicated` без `VersionGuard` клинит watermark), A13 (`remove_table` не чистит `per_table_mvcc`), A14 (конкурентные `drain_all` затирают commit-time ts).
- Стрей-файлы `*.log` в корне репозитория продолжают накапливаться (в этой сессии добавились `a10_*.log`, `tx_barrier.log`, `engine_gc*.log` и т.д.) — не критично, но требует уборки при удобном случае (уже отражено в задаче #482 CLEANUP из прошлого списка).

## Repo state
```
 M CLAUDE.md
 M crates/shamir-tx/src/mvcc_store/mod.rs
 M crates/shamir-tx/src/mvcc_store/mvcc_gc.rs
 M crates/shamir-tx/src/mvcc_store/mvcc_history.rs
 M crates/shamir-tx/src/repo_tx_gate.rs
 M crates/shamir-tx/src/tests/mvcc_store_tests/gc_tests.rs
 M crates/shamir-tx/src/tests/mvcc_store_tests/mod.rs
 M crates/shamir-tx/src/tests/mvcc_store_tests/retention_tests.rs
 M crates/shamir-tx/src/tests/mvcc_store_tests/vacuum_targeted_tests.rs
?? crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs
?? docs/checkpoints/2026-07-08-audit-remediation-a4-inflight.md
?? docs/checkpoints/2026-07-08-audit-remediation-babygoal.md
?? docs/prompts/audit/crush-launch-command.md
(+ множество стрей *.log файлов в корне — артефакты гейт-прогонов агентов, не отслеживаются git)
```
```
16fe21dd docs(prompts): brief for HIGH-concurrency A10 vacuum TOCTOU vs open_snapshot
8ee0f6b6 fix(index): close CREATE/RENAME INDEX backfill-registration write race (audit A9)
f05a29bf docs(prompts): brief for HIGH-concurrency A9 index create/rename backfill race
f9a2bba2 fix(tx): record referenced interner ids above persisted hwm (audit A8)
d78787e3 docs(prompts): brief for HIGH-concurrency A8 interner-delta lost on first-toucher abort
```
