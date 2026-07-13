# Checkpoint — 2026-07-09 08:15 [audit-remediation-perf-radical]

## Session summary
Продолжаем длинную кампанию по устранению находок 5-агентного аудита `docs/audits/2026-07-06-*.md`, инструкция пользователя: последовательно, через `/crush` для реализации и `@sh` (Agent) для адверсариального ревью, коммит после ревью и правок; при исчерпании лимитов crush — переход на `@sh`-агента как fallback.

Полностью закрыт кластер **HIGH-concurrency-isolation** (задача #481, ранее #447): все находки A2–A14 из `docs/audits/2026-07-06-concurrency-engine.md` реализованы, проверены (диф прочитан построчно, гейты/тесты перегнаны самостоятельно оркестратором) и отревьюены `@sh` с вердиктом SHIP IT / SHIP IT WITH NITS, закоммичены отдельными коммитами. Ключевые: A6 (lock-upgrade race), A8 (interner-delta race), A9 (index create/rename backfill race), A10 (vacuum TOCTOU — самый сложный фикс, hybrid barrier+anchor-deferral дизайн), A11 (recovery A5-гейт), A12 (apply_replicated VersionGuard), A13 (per_table_mvcc leak), A14 (pending_ts race).

Далее прошли: #482 CLEANUP (только doc-комментарии, без логики), #483/#484/#485 все три COMPLIANCE-задачи (cargo-deny/cargo-audit гейт + SECURITY.md + captrack pin; убрали plaintext username из auth-логов + wasmtime-policy; задокументировали at-rest encryption model + PII erasure procedure в новом `docs/security/data-protection.md`).

Сейчас в работе **PERF-RADICAL-кластер** (задачи из `docs/audits/2026-07-06-perf-radical-o-notation.md`):
- **#486** (fjall zero-copy Bytes + double lookup) — ЗАКРЫТА, коммит `e1206eee`. Честно репортирован флэт-результат для get/insert (I/O доминирует), но ×1.59 для scan_prefix.
- **#487** (CachedStore invalidate→populate + eager stream→incremental cursor) — ЗАКРЫТА, коммит `959d36f7`. ×1.77 read-after-write (in-RAM inner; аудит предсказывал ×10-100 для disk), ×1000+ для early-terminated stream consumption.
- **#488** (posting-cache Arc вместо clone-on-hit) — ЗАКРЫТА, коммит `2fbb7cdd`. ВАЖНЫЙ нюанс: crush-сессия упала с ошибкой ДО написания финального отчёта — я (оркестратор) самостоятельно верифицировал состояние кода напрямую (диф, сборка, тесты), нашёл и исправил fmt-нарушение (сессия не успела прогнать `cargo fmt`), и по итогам `@sh`-ревью обнаружил и разрешил серьёзный нюанс: engine-уровневый wrapper (`TableManager::lookup_by_index`) всё ещё клонирует `BTreeSet` на границе публичного API — проверил grep'ом по всем крейтам (server/client/sdk/db/connect), что этот путь СЕЙЧАС не используется вообще ни одним внешним вызывающим, задокументировал это как явный, обоснованный trade-off прямо в коде. 3.2 (structural sorted-slice representation) сознательно отложена — заведена как отдельная задача **#499**.
- **#489** (funclib distinct() O(N²) + WAL segment-open replay + interner reverse-vec clone) — В РАБОТЕ ПРЯМО СЕЙЧАС. Crush-сессия `perf-radical4-489` реально работает (подтверждено `crush sessions locks` — alive), видны новые калибровочные записи бенча `distinct_arrays::distinct_*` в `bench-iters.txt` (naive vs новая hash-based реализация) — похоже, находка 1.6 (funclib distinct) уже в процессе фикса. Диф ЕЩЁ НЕ проверен оркестратором (git diff не читан построчно), гейты/тесты ЕЩЁ НЕ перепрогнаны самостоятельно, `@sh`-ревью ЕЩЁ НЕ отправлено, НЕ закоммичено.

Каждый фикс в этой кампании проходит строгий пайплайн: бриф в `docs/prompts/audit/NN-*.md` → коммит брифа отдельно → `/crush` (background, БЕЗ trailing `&` — bare command + `run_in_background:true`, следуя недавно закреплённому в CLAUDE.md правилу) → независимая проверка диффа+тестов+гейтов оркестратором (никогда не доверять отчёту агента на слово) → `@sh` (Agent tool) адверсариальное ревью с конкретными пунктами скрутини → повторная независимая проверка → коммит с подробным сообщением, кредитующим что именно было проверено.

Heartbeat `/babysit` активен (cron `4046f5ff`, `7,27,47 * * * *`, каждые ~20 мин).

## Active goal
`/goal` был установлен пользователем: **"выполним все таски, друг"** (session-scoped Stop hook, блокирует остановку сессии до выполнения условия — цель ещё активна, НЕ достигнута).

## TaskList
### in_progress
- #489 PERF-RADICAL-4: funclib distinct() O(N²) + WAL segment-open replay + interner reverse-vec clone (crush-сессия `perf-radical4-489` реально работает, диф не проверен)

### pending
- #490 PERF-RADICAL-5: CREATE INDEX полная материализация таблицы + fjall spawn_blocking-per-op + TCP framing memcpy
- #491 PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → инлайн Key128(u128) сквозной ключ (высокая сложность, возможно требует отдельного дизайн-прохода)
- #492 FLAKE: vr5_cofilter_sees_staged_and_filters_residual intermittent failure
- #493 Migrate remaining 41 criterion benches to bench-scale-tool fixed-iteration harness
- #494 MEDIUM-durability residual (1.5-1.9, 2.2-2.6 из durability-storage-wal-tx.md)
- #495 HIGH-security residual (ticket channel-binding + subscription fanout limits + WASM fuel/SSRF)
- #496 HIGH-perf residual (keyset O(N²) pagination + unbounded ts_index/cells + MemBuffer drain amplification + SQ8 SIMD fusion)
- #497 HIGH-client residual (error-code typing, timeouts, wire-type drift, executeWithTouch parity, e2e/parity gaps)
- #498 Triage 13 RUSTSEC advisories surfaced by the new cargo-deny gate (найдены при живом прогоне `cargo deny check` в рамках #483 — реальные vulnerability/unsound/unmaintained находки, требуют security-триажа, не механической правки)
- #499 PERF-RADICAL-3.2: sorted-slice posting-list representation (structural, деривативная от #488, сознательно отложена)

### recently completed
- #488 PERF-RADICAL-3 (posting-cache Arc, коммит 2fbb7cdd)
- #487 PERF-RADICAL-2 (CachedStore, коммит 959d36f7)
- #486 PERF-RADICAL-1 (fjall zero-copy, коммит e1206eee)
- #485 COMPLIANCE-3 (data-protection.md, коммит 507b6105)
- #484 COMPLIANCE-2 (plaintext username, коммит dec93a20)
- #483 COMPLIANCE-1 (cargo-deny/audit gate, коммит 1a6a67a0)
- #482 CLEANUP (stale comments, коммит b25c0b08)
- #481 HIGH-concurrency-isolation (A2-A14, множество коммитов, завершён)

## Decisions
- Кластерные PERF-задачи разбиваются на под-находки; там, где аудит сам помечает пункт «структурное»/высокая сложность, брифы явно разрешают crush-агенту scope-down: задокументировать находку + предложенный дизайн как follow-up задачу вместо рискованной половинчатой миграции (сработало для #488's 3.2 → #499).
- Честная репортировка перф-результатов — приоритет над красивыми цифрами: #486/#487 оба честно репортировали случаи, где ожидаемый выигрыш не материализовался (get/insert flat в #486), и объясняли почему, а не подгоняли числа.
- При обрыве crush-сессии до финального отчёта (как в #488) — оркестратор верифицирует состояние кода НАПРЯМУЮ (build/test/diff), а не считает задачу проваленной; если код фактически корректен и протестирован, продолжает пайплайн (гейты → @sh → коммит) от своего имени.
- Не поднимать фоновые процессы через `&` внутри команды — только `run_in_background: true` на голой команде (закреплено в CLAUDE.md после инцидентов с ложными "completed"-уведомлениями от shell-обёртки).

## Open questions
- #489: реальный статус фикса (какие из трёх находок — 1.6/2.1/2.3 — фактически реализованы, какие отложены) станет известен только после завершения crush-сессии и независимой проверки оркестратором — пока неизвестно.
- #491 (PERF-RADICAL-STRUCTURAL, RecordKey→Key128) явно помечена в аудите как «высокая сложность, структурное» — возможно потребует отдельного дизайн-обсуждения с пользователем перед тем, как поручать её crush в одном шаге, аналогично тому, как #499 была выделена из #488.
- Стрей-файлы `*.log` в корне репозитория продолжают накапливаться (артефакты гейт-прогонов) — не критично, но стоит убрать при удобном случае (возможно, кандидат для отдельной задачи очистки, отличной от #482, который был чисто doc-комментарии).

## Repo state
```
 M CLAUDE.md
 M Cargo.lock
 M bench-iters.txt
 M crates/shamir-funclib/Cargo.toml
 M crates/shamir-funclib/src/arrays.rs
 M crates/shamir-funclib/src/arrays/tests/arrays_tests.rs
?? crates/shamir-funclib/benches/
(+ множество стрей *.log файлов в корне — не отслеживаются git, артефакты гейт-прогонов)
```
```
9f8b108d docs(prompts): brief for PERF-RADICAL-4 #489 distinct/WAL-open/interner-clone
2fbb7cdd perf(index): posting-cache Arc<BTreeSet> instead of clone-on-hit (audit 1.5)
891cd22f docs(prompts): brief for PERF-RADICAL-3 #488 posting-cache Arc + sorted-slice
959d36f7 perf(storage): CachedStore populates on Set + incremental streams (audit 1.3+1.4)
2793c675 docs(prompts): brief for PERF-RADICAL-2 #487 CachedStore invalidate + eager stream materialize
e1206eee perf(storage): fjall zero-copy reads + remove pointless insert lookup (audit 1.1+1.2)
3abf9ca2 docs(prompts): brief for PERF-RADICAL-1 #486 fjall zero-copy + double-lookup
507b6105 docs(security): document at-rest encryption model + PII retention/erasure (audit C6)
```
