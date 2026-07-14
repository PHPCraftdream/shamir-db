בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-05 [vector-p3-inflight]

## Session summary

Идёт РЕАЛИЗАЦИЯ векторной кампании (production-ready vector search) по
`docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`, запущенной через /babygoal
(цель «реализуй всю векторную кампанию»). Конвейер серийный по листам
(#393–#416): prompt-first бриф в `docs/dev-artifacts/prompts/vector/` (коммит ДО запуска) →
реализация агентом → ЛИЧНАЯ верификация оркестратором (гейт: fmt/clippy/
`./scripts/test.sh`) → ревью агентом **@ol** (Opus 4.8 low) → починка находок →
коммит перед следующим листом. babysit-cron `cd3e2d08` (15m) страхует.

**ВАЖНО — смена делегата:** пользователь попросил переключиться с /crush на
**@o46l** (Opus 4.6 low, Agent tool, синхронный) начиная с #405. Также сказал
«работаем до 12 часов по моему времени» (продолжать автономно до полудня).

**Готово и закоммичено: 12/23 листов** (P0+P1+P2 полностью, P3 начата):
- P0 (#393-398): спайк hnsw_rs, генератор, upsert_batch, criterion+report+baseline.
- P1 (#399): per-query ef_search+oversample, wire+билдеры Rust/TS+parity.
- P2 persist (#400-403): snapshot-кодек+crc32 → load-on-open+fallback+ArcSwap →
  delta-log+фоновый generation-flip → crash-тесты+cold-start (рестарт 4.75×).
- P3 (#404): filtered ANN post-filter+oversample (dbc46b05).

**In-flight СЕЙЧАС:** #405 (V3.2 pre-filter+co-filter+cost-based). o46l-агент
(agentId `acb558bb30960da44`) РАБОТАЕТ В ФОНЕ над починкой ревью-находок.
Рабочее дерево ГРЯЗНОЕ (незакоммичены правки o46l: read_exec.rs, backend.rs +
все backend-impl'ы as_any, hnsw_adapter.rs, simd.rs, новый
cofilter_prefilter_tests.rs). Первая итерация o46l дала РАБОЧИЙ ГЕЙТ (1690
passed), НО @ol нашёл 2 блокера — отправлены на фикс через SendMessage:
- **C1 (CRITICAL, wrong-results):** pre/co-filter отбрасывают НЕПОКРЫТЫЙ
  индексом остаточный конъюнкт (`_residual2` игнорируется) → возвращают строки,
  нарушающие WHERE. Фикс: fast-path ТОЛЬКО при полном покрытии остатка; иначе
  fall through в post-filter. + C1-ловящий тест.
- **M3 (MAJOR, вакуумный тест):** `search_filter_overscan_contract` не ассертит
  `tight<knbn` — долг V0.0 не закрыт. Фикс: статистический assert (цикл build'ов).
- m6: удалить мёртвый `has_rid`.

**Пойманные баги за сессию (двойная верификация себя оправдывает):** #395
двойной clone · #399 сломанная сборка · #400 UAF-инвариант · #401 Guard-через-
await · #402 MAJOR single-flight флаг залипал при panic · #403 MAJOR SLOW 99.7s
+ durability gap#1 (→ #416) · #404 double-fetch/retry · #405 CRITICAL C1
wrong-results + M3 вакуумный тест.

Не запушено НИЧЕГО из кампании (все коммиты локальные; последний push —
репликация `23aaf331` в прошлой части сессии).

## Active goal

реализуй всю векторную кампанию

(Stop-hook активен; авто-снимется когда все таски выполнены. Работать до 12:00
по времени пользователя. НЕ говорить про /goal clear.)

## TaskList

### in_progress
- #405 V3.2 pre-filter+co-filter+cost-based — o46l фиксит C1/M3/m6 в фоне
  (agentId acb558bb30960da44)

### pending (по фазам)
P3: #406 selectivity-бенч [405]
P4: #407 batch promote+deleted_ratio+fix O(N) len() [395 done — ГОТОВ] ·
  #408 компакция [407,402] · #409 бенчи [408]
P5: #410 SQ8+int8 SIMD [393 done — ГОТОВ] · #411 квант.граф+DDL [410,400] ·
  #412 снапшот v2 [411,408]
V6: #413 Node e2e [399,405,411] · #414 TS e2e [399,405,411] · #415 OQL+guide [406,412]
#416 tx-delete ghost-vector fix (pre-existing HIGH-6, вскрыт P2) — приоритет

Готовы к старту без блокеров: #407 (batch promote), #410 (SQ8), #416.

### recently completed (10)
#404 V3.1 filtered post-filter · #403 V2.4 crash+cold-start · #402 V2.3 delta-log ·
#401 V2.2 load-on-open · #400 V2.1 snapshot-кодек · #399 V1.1 ef_search ·
#398 V0.5 baseline · #397 V0.4 vector_report · #396 V0.3 criterion · #395 V0.2 upsert_batch

## Decisions

- **Делегат: @o46l (Opus 4.6 low) вместо /crush** с #405 (по просьбе user'а).
  Синхронный Agent-tool. Ревью @ol + верификация гейтом обязательны — поймали
  CRITICAL-баг у o46l на первой итерации.
- **Бенчи только QUICK**, **$score → отдельный трек where-select-binds**,
  **ArcSwap<AdapterSlot>** (задел под live-swap) — прежние решения.
- **#405 fast-path гейтить на ПОЛНОМ покрытии остатка** (C1-фикс) — иначе тихий
  wrong-results.

## Open questions

- Нет открытых вопросов от пользователя. Кампания автономна до 12:00.
- Пуш: НИЧЕГО из кампании не запушено — жду явной команды.

## Repo state

```
(дерево ГРЯЗНОЕ — o46l #405 in-flight, незакоммичены:)
 M crates/shamir-engine/src/table/read_exec.rs
 M crates/shamir-index/src/backend.rs (+ fts/fts_ranked/functional/registry/write_ops: as_any)
 M crates/shamir-index/src/vector/{hnsw_adapter,mod,simd,vector_backend}.rs
 M crates/shamir-index/src/vector/tests/mod.rs
?? crates/shamir-index/src/vector/tests/cofilter_prefilter_tests.rs
?? docs/dev-artifacts/checkpoints/* (untracked)
```

```
7e4d99c3 docs(prompts): brief for V3.2 pre-filter + co-filter + cost-based selection
dbc46b05 feat(engine): filtered ANN — And(vector, preds) post-filter + oversample (V3.1)
4fa6e891 docs(prompts): brief for V3.1 filtered ANN post-filter + oversample
6f92bb2b test(index): P2 crash-recovery tests + cold-start bench — phase P2 closed (V2.4)
d1a4b900 docs(prompts): brief for V2.4 crash tests + cold-start bench (closes P2)
```

_Следующий шаг: дождаться уведомления о завершении o46l-фикса #405 → перегнать
гейт САМ (fmt/clippy --workspace/@vector @engine --full, проверить что новый
C1-тест реально краснел на старом коде и зеленеет, M3 не вакуумен) → при
зелёном коммит V3.2 → #406. Если сессия перезапускается с ГРЯЗНЫМ деревом:
проверить транскрипт агента acb558bb30960da44
(.../tasks/acb558bb30960da44.output); если мёртв — дособрать/верифицировать
вручную по брифу 12-prefilter-cofilter.md + находкам C1/M3/m6 выше._
