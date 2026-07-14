# Checkpoint — 2026-07-09 09:05 [audit-remediation-perf-radical-2]

## Session summary
Продолжение очень длинной кампании по устранению находок 5-агентного аудита `docs/dev-artifacts/audits/2026-07-06-*.md`. Пользователь дал инструкцию: последовательно, через `/crush` для реализации и `@sh` (Agent) для адверсариального ревью, коммит после ревью и правок; при исчерпании лимитов crush — переход на `@sh` как fallback. Установлен `/goal` («выполним все таски, друг») — сессия не должна останавливаться до выполнения условия.

Полностью закрыты: весь кластер **HIGH-concurrency-isolation** (#481, находки A2–A14), **#482 CLEANUP** (только doc-комментарии), все три **COMPLIANCE**-задачи (#483 cargo-deny/audit gate + SECURITY.md + captrack pin; #484 plaintext username + wasmtime policy; #485 at-rest encryption doc), и четыре **PERF-RADICAL** задачи:
- **#486** (fjall zero-copy + double lookup) — коммит `e1206eee`.
- **#487** (CachedStore invalidate→populate + eager stream→incremental cursor) — коммит `959d36f7`.
- **#488** (posting-cache Arc вместо clone-on-hit) — коммит `2fbb7cdd`. Особенность: crush-сессия упала до финального отчёта, оркестратор верифицировал состояние кода напрямую и разрешил найденный `@sh`-ревью нюанс (engine-wrapper клонирует на границе — подтверждено grep'ом, что публичный API сейчас не используется вообще, задокументировано как явный trade-off). 3.2 (structural sorted-slice) сознательно отложена → задача **#499**.
- **#489** (funclib distinct() O(N²) + WAL segment-open + interner reverse-vec) — коммит `acb15613`. ВАЖНО: `@sh`-ревью нашло РЕАЛЬНЫЙ баг — `Hash`/`Eq` для `Value<Key>` были несогласованы для NaN с разными битовыми паттернами (PartialEq считает любые NaN равными, но Hash хэшировал сырой bit-pattern) — баг был не в самом фиксе distinct(), а в общем коде `shamir-types`, куда фикс полагался. Оркестратор сам исправил `Hash`-impl (канонизация NaN перед хэшированием) и добавил регрессионный тест, затем отправил на повторное `@sh`-ревью, получил чистый SHIP IT. 2.1 (WAL segment-open full replay) и 2.3 (interner reverse-vec clone) сознательно отложены после тщательного расследования (проверено, что WAL-формат не поддерживает backward-seek без format bump; interner's entries_after сопряжён с A5/A8/A11 crash-safety инвариантами) → задачи **#500** и **#501**.

Сейчас в работе **#490** (PERF-RADICAL-5: CREATE INDEX полная материализация таблицы + fjall spawn_blocking-per-op + TCP framing memcpy). Crush-сессия `perf-radical5-490` реально работает (подтверждено `crush sessions locks` — alive), видны реальные изменения в: `crates/shamir-index/src/legacy/index_manager.rs` (2.4 — incremental batching для create_index), `crates/shamir-server/src/connection/{push_sink.rs,request_loop.rs}`, `crates/shamir-server/src/framer.rs`, `crates/shamir-transport-tcp/src/framing.rs` (3.4 — TCP framing memcpy fix). Похоже, 3.3 (fjall spawn_blocking) была либо отложена, либо ещё не начата — `crates/shamir-storage/` не тронут. Диф ЕЩЁ НЕ проверен оркестратором, гейты/тесты ЕЩЁ НЕ перепрогнаны, `@sh`-ревью ЕЩЁ НЕ отправлено, НЕ закоммичено.

Установившийся паттерн кампании: для находок, помеченных аудитом как «структурные»/высокой сложности, брифы явно разрешают crush-агенту scope-down (задокументировать находку + предложенный дизайн как follow-up задачу вместо рискованной половинчатой миграции) — сработало для #488→#499, #489→#500/#501. Каждый фикс проходит: бриф → коммит брифа отдельно → `/crush` (background, bare command БЕЗ `&`, следуя правилу из CLAUDE.md) → независимая проверка диффа+тестов+гейтов оркестратором (никогда не доверять отчёту агента на слово, особенно если сессия упала до отчёта, как в #488/#489) → `@sh` адверсариальное ревью с конкретными пунктами скрутини → при необходимости оркестратор сам правит найденные баги и повторно отправляет на ревью → коммит с подробным сообщением.

Heartbeat `/babysit` активен (cron `4046f5ff`, `7,27,47 * * * *`, каждые ~20 мин).

## Active goal
`/goal`: **"выполним все таски, друг"** (session-scoped Stop hook, активна, НЕ достигнута).

## TaskList
### in_progress
- #490 PERF-RADICAL-5: CREATE INDEX полная материализация таблицы + fjall spawn_blocking-per-op + TCP framing memcpy (crush-сессия `perf-radical5-490` работает, диф не проверен)

### pending
- #491 PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → инлайн Key128(u128) сквозной ключ (высокая сложность, возможно нужен отдельный дизайн-проход)
- #492 FLAKE: vr5_cofilter_sees_staged_and_filters_residual intermittent failure
- #493 Migrate remaining 41 criterion benches to bench-scale-tool fixed-iteration harness
- #494 MEDIUM-durability residual (1.5-1.9, 2.2-2.6 из durability-storage-wal-tx.md)
- #495 HIGH-security residual (ticket channel-binding + subscription fanout limits + WASM fuel/SSRF)
- #496 HIGH-perf residual (keyset O(N²) pagination + unbounded ts_index/cells + MemBuffer drain amplification + SQ8 SIMD fusion)
- #497 HIGH-client residual (error-code typing, timeouts, wire-type drift, executeWithTouch parity, e2e/parity gaps)
- #498 Triage 13 RUSTSEC advisories surfaced by the new cargo-deny gate (реальные vulnerability/unsound/unmaintained находки, security-триаж)
- #499 PERF-RADICAL-3.2: sorted-slice posting-list representation (structural, из #488)
- #500 PERF: WAL segment-open avoid full replay-for-max-version (finding 2.1, из #489)
- #501 PERF: Interner segmented-spine to avoid full reverse-vec clone (finding 2.3, из #489)

### recently completed
- #489 PERF-RADICAL-4 (distinct() + Hash/Eq NaN bugfix, коммит acb15613)
- #488 PERF-RADICAL-3 (posting-cache Arc, коммит 2fbb7cdd)
- #487 PERF-RADICAL-2 (CachedStore, коммит 959d36f7)
- #486 PERF-RADICAL-1 (fjall zero-copy, коммит e1206eee)
- #485 COMPLIANCE-3 (data-protection.md, коммит 507b6105)
- #484 COMPLIANCE-2 (plaintext username, коммит dec93a20)
- #483 COMPLIANCE-1 (cargo-deny/audit gate, коммит 1a6a67a0)
- #482 CLEANUP (stale comments, коммит b25c0b08)
- #481 HIGH-concurrency-isolation (A2-A14, множество коммитов)

## Decisions
- При обрыве crush-сессии до финального отчёта — оркестратор верифицирует состояние кода НАПРЯМУЮ (build/test/diff), продолжает пайплайн от своего имени, а не считает задачу проваленной (сработало дважды: #488, #489).
- Найденные `@sh`-ревью реальные баги (не нюансы/trade-off'ы) — оркестратор чинит САМ (не перезапускает crush заново), затем отправляет на повторное `@sh`-ревью перед коммитом (сработало для NaN Hash/Eq бага в #489).
- Структурные/высокой сложности под-находки внутри PERF-RADICAL задач — выделяются в отдельные follow-up задачи (#499, #500, #501) вместо рискованной половинчатой миграции в рамках более узкой задачи.
- Честная репортировка перф-результатов остаётся приоритетом — несколько задач в этой кампании честно репортировали случаи, где ожидаемый выигрыш не материализовался, и объясняли почему.

## Open questions
- #490: реальный статус всех трёх находок (2.4/3.3/3.4) станет известен только после завершения crush-сессии и независимой проверки оркестратором.
- #491 (PERF-RADICAL-STRUCTURAL) явно помечена аудитом как «высокая сложность, структурное» — возможно потребует отдельного дизайн-обсуждения с пользователем, аналогично тому как #499/#500/#501 были выделены как follow-up вместо немедленной реализации.
- Стрей-файлы `*.log` в корне репозитория продолжают накапливаться (артефакты гейт-прогонов) — не критично, кандидат для отдельной задачи очистки в будущем.

## Repo state
```
 M CLAUDE.md
 M crates/shamir-index/src/legacy/index_manager.rs
 M crates/shamir-server/src/connection/push_sink.rs
 M crates/shamir-server/src/connection/request_loop.rs
 M crates/shamir-server/src/framer.rs
 M crates/shamir-transport-tcp/src/framing.rs
(+ множество стрей *.log файлов в корне — не отслеживаются git, артефакты гейт-прогонов)
```
```
adc1f010 docs(prompts): brief for PERF-RADICAL-5 #490 create-index materialize + spawn_blocking + tcp framing
acb15613 perf(funclib): distinct() O(N)-hash dedup + fix Hash/Eq NaN bug (audit 1.6)
9f8b108d docs(prompts): brief for PERF-RADICAL-4 #489 distinct/WAL-open/interner-clone
2fbb7cdd perf(index): posting-cache Arc<BTreeSet> instead of clone-on-hit (audit 1.5)
891cd22f docs(prompts): brief for PERF-RADICAL-3 #488 posting-cache Arc + sorted-slice
959d36f7 perf(storage): CachedStore populates on Set + incremental streams (audit 1.3+1.4)
```
