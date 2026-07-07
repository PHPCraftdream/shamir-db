בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-05 [vector-campaign-progress]

## Session summary

Идёт РЕАЛИЗАЦИЯ векторной кампании (production-ready vector search) по плану
`docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`, запущенной через /babygoal с
целью «реализуй всю векторную кампанию». Конвейер: серийно по одному листу
(таски #393–#415), для каждого — prompt-first бриф в `docs/prompts/vector/`
(коммит ДО запуска) → реализация агентом **/crush** (сессии `vector-<NN>`,
foreground-запуск, харнесс фоново ведёт и шлёт уведомление о завершении) →
ЛИЧНАЯ верификация оркестратором (гейт сам: fmt/clippy/`./scripts/test.sh`) →
ревью агентом **@ol** (Opus 4.8 low) → применение находок → коммит перед
следующим листом. babysit-cron `cd3e2d08` (15m) страхует.

**Готово и закоммичено: 9/23 листов** (фазы P0 + P1 + P2 наполовину):
- P0 bench-фундамент: #393 спайк hnsw_rs 0.3.4 (`5ec84564`), #394 clustered
  генератор+@vector scope (`c4603693`), #395 upsert_batch/parallel_insert
  (`c777eea1`), #396 criterion-бенч tune_tiered (`b92e04ee`), #397 vector_report
  recall/RSS+bench-vector.sh (`a603cacc`), #398 baseline+гигиена доков
  (`b60fc05d`).
- P1: #399 per-query ef_search + oversample поле, wire+билдеры Rust/TS+parity
  (`5be1252d`).
- P2 persist: #400 snapshot-кодек dump/load+crc32 (`c80d99f9`), #401 load-on-open
  +fallback rebuild+ArcSwap hot-swap (`6596ac24`).

**In-flight СЕЙЧАС:** #402 (V2.3 delta-log + фоновый снапшот + generation flip)
— crush-сессия `vector-402` (ID `b9ox2is8t`) ЖИВА и активно правит дерево
(commit_phases.rs Phase 5d append delta, snapshot.rs, vector_backend.rs
триггер, tunable `VECTOR_SNAPSHOT_DELTA_THRESHOLD`). Рабочее дерево ГРЯЗНОЕ
(незакоммиченные правки crush #402). Жду уведомление о завершении, затем
верификация гейта (@vector @oracle --full — Phase 5d тронут) + ревью @ol.

**Пойманные баги (верификация гейтом себя оправдывает):** в #399 crush оставил
сломанную сборку (несуществующий `rmp_serde::encode::write_*` в parity-тесте) —
починил сам литеральными байтами; в #395 убрал двойной clone + добавил
непокрытый D12-подслучай; в #400 применил non-mmap-инвариант против UAF (@ol
трассировал hnsw_rs исходники); в #401 заменил `.load()`→`.load_full()` на 7
hot-path (Guard через .await). Ключевое решение #399: byte-identical parity
НЕВОЗМОЖНА для f32 через @msgpack → семантическая parity (взаимная
декодируемость), @ol подтвердил.

Не запушено НИЧЕГО из кампании (все коммиты локальные; последний push был
репликации `23aaf331` в прошлой сессии).

## Active goal

реализуй всю векторную кампанию

(Stop-hook активен; авто-снимется когда все таски выполнены. НЕ говорить
пользователю про /goal clear.)

## TaskList

### in_progress
- #402 V2.3 delta-log + фоновый снапшот (generation flip) — crush vector-402 ЖИВ

### pending (по фазам)
P2: #403 crash-тесты+cold-start бенч [401 done] (blocked by #402)
P3 filtered ANN: #404 post-filter+oversample [398,399 done] · #405 pre/co-filter
  (search_filter, есть overscan-заметка из V0.0) [404] · #406 selectivity-бенч [405]
P4: #407 batch promote+deleted_ratio+fix O(N) len() [395 done — ГОТОВ к старту] ·
  #408 компакция [407,402] · #409 бенчи bulk/компакции [408]
P5: #410 SQ8+int8 SIMD [393 done — ГОТОВ] · #411 квант.граф+DDL Rust/TS [410,400] ·
  #412 снапшот v2+миграция [411,408]
V6 полный стек: #413 Node e2e 18-vectors [399,405,411] · #414 TS e2e [399,405,411] ·
  #415 OQL+guide [406,412]

Готовы к старту без блокеров ПОСЛЕ #402: #407 (batch promote), #410 (SQ8).
Критический путь persist: #402→#403.

### recently completed (10)
#401 V2.2 load-on-open · #400 V2.1 snapshot-кодек · #399 V1.1 ef_search ·
#398 V0.5 baseline · #397 V0.4 vector_report · #396 V0.3 criterion ·
#395 V0.2 upsert_batch · #394 V0.1 bench-инфра · #393 V0.0 спайк ·
(+ #388/#389 репликации в прошлой части сессии)

## Decisions

- **Делегирование: /crush реализует, @ol ревьюит, коммит между листами** (по
  просьбе пользователя). Верификация гейтом оркестратором ОБЯЗАТЕЛЬНА (не по
  конверту агента) — поймала сломанные сборки.
- **Бенчи только QUICK** (FULL-заглушка не снимается) — по решению кампании.
- **$score отвергнут** → отдельный трек where-select-binds (не в кампании).
- **byte-identical parity → семантическая** (f32 через @msgpack невозможен) —
  #399, @ol подтвердил как улучшение.
- **ArcSwap<AdapterSlot> в VectorBackend** (#401) — постоянный, задел под #402/
  #408 live-swap; читатели через load_full() (Arc, не Guard через await).

## Open questions

- Нет открытых вопросов, требующих ввода пользователя. Кампания идёт
  автономно по плану; развилки решаются «в сторону совершенства».
- Пуш: НИЧЕГО из кампании не запушено — пользователь пуш не просил, жду явной
  команды (или в конце кампании спросить).

## Repo state

```
(рабочее дерево ГРЯЗНОЕ — crush #402 in-flight; незакоммичены:)
 M crates/shamir-engine/src/tx/commit_phases.rs
 M crates/shamir-index/src/backend.rs
 M crates/shamir-index/src/vector/{adapter,hnsw_adapter,snapshot,vector_backend}.rs
 M crates/shamir-index/src/vector/tests/vector_restore_tests.rs
 M crates/shamir-tunables/src/lib.rs
?? docs/checkpoints/* (несколько, untracked)
```

```
a7a893d7 docs(prompts): brief for V2.3 delta-log + background snapshot generation flip
6596ac24 feat(index): load HNSW snapshot on open, fallback to rebuild (V2.2)
5fd4c1cd docs(prompts): brief for V2.2 snapshot startup integration + fallback
c80d99f9 feat(index): HNSW snapshot codec — dump/load to info_store (V2.1)
910908a5 docs(prompts): brief for V2.1 HNSW snapshot codec
```

_Следующий шаг: дождаться уведомления о завершении crush #402 (b9ox2is8t) →
верифицировать гейт @vector @oracle --full → ревью @ol → починить находки →
коммит V2.3 → запустить #403 (crash-тесты, закроет P2). Если сессия
перезапускается с ГРЯЗНЫМ деревом от #402: проверить crush sessions locks
vector-402; если мёртв — прочитать .crush/stdin/vector-402.out, дособрать/
верифицировать вручную ИЛИ перезапустить crush на брифе 09-delta-log.md._
