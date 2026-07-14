# Checkpoint — 2026-07-08 [audit-remediation-a4-inflight]

## Session summary
Продолжаем кампанию по устранению находок из 5-агентного аудита `docs/dev-artifacts/audits/2026-07-06-*.md`, по инструкции пользователя: последовательно, через `/crush` для реализации и `@sh` (Agent) для ревью, коммит после ревью и правок. Все 8 исходных CRITICAL + 1 обнаруженный по ходу работы (CRIT-9, resume() позиционный/именованный формат в TS-клиенте) — закрыты и закоммичены. Первые пункты всех 4 исходных HIGH-кластеров (#443 durability, #444 security, #445 perf, #446 client) реализованы, остаток каждого вынесен в отдельную задачу (#476–478, #480) с более низким приоритетом.

Сейчас в работе задача **#447 HIGH-concurrency-isolation** (MVCC/SSI кластер). Уже закрыты и закоммичены:
- **A2** (`publish_cell`/`seed_version` немонотонная версия) — коммит `9afc1957`, отревьюено `@sh` (SHIP IT).
- **A3** (SSI read-set фиксирует «текущую» версию вместо прочитанной) — коммит `dc73c824`, отревьюено `@sh` (SHIP IT, один некритичный комментарий-нит).

Бриф для **A4** (Level-3 Pessimistic: lost update — ранний release локов ДО публикации + чтение под локом по устаревшему snapshot) написан и закоммичен (`docs/dev-artifacts/prompts/audit/17-pessimistic-lost-update-early-release-and-snapshot-stale-lock-read.md`, коммит `a1cf6e7e`). Реализация запущена через `/crush` (сессия `pessimistic-lost-update-a4`) — после нескольких прерываний (peak-hours у провайдера zai, гонки сессионных локов) агент реально отредактировал файлы: `table_manager_streaming.rs`, `tx/commit.rs`, `tx/group_commit.rs`, `tx/tests/mod.rs`, новый `tx/tests/pessimistic_lost_update_tests.rs`. **Этот диф ЕЩЁ НЕ проверен оркестратором (git diff не читан построчно), НЕ отправлен на ревью `@sh`, гейты (fmt/clippy/test) ЕЩЁ НЕ перепрогнаны самостоятельно, НЕ закоммичен.**

Heartbeat `/babysit` переустановлен (cron `8ec51d05`, `7,27,47 * * * *`, каждые ~20 мин) после того как предыдущий cron истёк (7-дневный лимит / сессионный, слетел).

По просьбе пользователя этот чекпоинт **намеренно не фиксирует** детали возни с самим `/crush` (пиковые часы, гонки локов, повторные запуски) — только состояние работы по shamir-db.

## Active goal
none (используется `/babygoal`, не `/goal`)

## TaskList
### in_progress
- #447 HIGH-concurrency-isolation: MVCC/SSI кластер (A2/A3/A4/A6/A7 + A8-A14) — A2, A3 закрыты; A4 в реализации (диф есть, не проверен/не закоммичен); A6, A7(уже частично закрыт как CRIT-3/#437) и A8-A14 ещё не начаты.

### pending
- #448 CLEANUP: устаревшие/лживые doc-комментарии + мёртвый код (durability §3, concurrency §2-3)
- #449 COMPLIANCE-1: cargo-deny/cargo-audit CI-гейт + SECURITY.md + captrack path-pin
- #450 COMPLIANCE-2: plaintext username в auth-логах + wasmtime advisory-политика
- #451 COMPLIANCE-3: at-rest шифрование + PII retention/erasure политика
- #452–456 PERF-RADICAL-1..5 (fjall zero-copy, CachedStore read-after-write, posting-list Arc, funclib distinct() O(N²), CREATE INDEX материализация)
- #457 PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → Key128(u128) (архитектурная, высокая сложность)
- #458 FLAKE: vr5_cofilter_sees_staged_and_filters_residual intermittent failure
- #459 Migrate remaining 41 criterion benches (возможно уже перекрыто прошлой bench-нормализацией — не проверено)
- #476 MEDIUM-durability residual (1.5-1.9, 2.2-2.6)
- #477 HIGH-security residual (ticket channel-binding, subscription fanout, WASM fuel/SSRF)
- #478 HIGH-perf residual (keyset O(N²), unbounded ts_index/cells, MemBuffer drain amplification, SQ8 SIMD)
- #480 HIGH-client residual (error-code typing, timeouts, wire-type drift, executeWithTouch parity, e2e/parity gaps)

### recently completed
- #479 CRIT-9: resume() позиционный/именованный decode баг
- #446 HIGH-client (query_version staleness + resume downgrade, часть)
- #445 HIGH-perf (UPDATE/upsert-merge dead de-interning, часть)
- #444 HIGH-security (accept timeout+per-IP cap, Argon2 gating, часть)
- #443 HIGH-durability (WAL quarantine + GroupCommit leader RAII, часть)
- #442 CRIT-8, #441 CRIT-7, #440 CRIT-6, #439 CRIT-5, #438 CRIT-4

## Decisions
- Кластерные задачи (#443-446) разбиваются: топ-находка фиксится сразу, остаток уходит в отдельную задачу с более низким приоритетом (не блокирует переход к следующему кластеру).
- Новые CRITICAL находки (как CRIT-9) обрабатываются вне очереди, раньше номерных HIGH-задач.
- Каждый фикс проходит: бриф → коммит брифа отдельно → `/crush` → независимая проверка diff+тестов оркестратором → ревью `@sh` → повторная независимая проверка гейтов → коммит с подробным сообщением.
- A2/A3 в MVCC — оставлены раздельными функциями (`publish_cell`/`seed_version`) несмотря на идентичное поведение после фикса — разное семантическое намерение вызова, риск ре-аудита при слиянии.

## Open questions
- A4 diff (в работе прямо сейчас): нужно ли что-то менять в wound-wait/lock-timeout логике из-за более позднего release локов? — ещё не проверено оркестратором.
- #459 (миграция бенчей) — возможно полностью перекрыта прошлой сессией нормализации бенчей, не проверено.
- Стрей-файлы `*.log` в корне репозитория (артефакты гейт-прогонов агентов) продолжают накапливаться — не критично, но требует уборки при удобном случае.

## Repo state
```
 M crates/shamir-engine/src/table/table_manager_streaming.rs
 M crates/shamir-engine/src/tx/commit.rs
 M crates/shamir-engine/src/tx/group_commit.rs
 M crates/shamir-engine/src/tx/tests/mod.rs
?? crates/shamir-engine/src/tx/tests/pessimistic_lost_update_tests.rs
?? docs/dev-artifacts/checkpoints/2026-07-08-audit-remediation-babygoal.md
?? docs/dev-artifacts/prompts/audit/crush-launch-command.md
(+ множество стрей *.log файлов в корне — артефакты гейт-прогонов агентов, не отслеживаются git)
```
```
a1cf6e7e docs(prompts): brief for HIGH-concurrency A4 pessimistic lost update
dc73c824 fix(engine): clamp SSI read-set version to min(current, snapshot) (audit A3)
54eaf794 docs(prompts): brief for HIGH-concurrency A3 SSI read-set stale version recording
9afc1957 fix(tx): make publish_cell/seed_version max-monotonic (audit A2)
e2a7c35a docs(prompts): brief for HIGH-concurrency A2 publish_cell/seed_version monotonicity
cced2851 fix(client-ts): CRIT-9 resume() decoded positional wire array as a named map
253f1c34 docs(prompts): brief for CRIT-9 TS resume() positional array decode fix
f2a41d39 fix(client-ts): stop hardcoding query_version:1 + read server_query_version on resume
```
