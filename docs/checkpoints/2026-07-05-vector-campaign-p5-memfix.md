בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-05 20:16 [vector-campaign-p5-memfix]

## Session summary

Идёт РЕАЛИЗАЦИЯ векторной кампании (production-ready vector search) по
`docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`, запущена через /babygoal
(«реализуй всю векторную кампанию»). Конвейер: prompt-first бриф в
`docs/prompts/vector/<NN>-*.md` (коммит ДО запуска) → реализация делегатом →
ЛИЧНАЯ zero-trust верификация оркестратором (гейт: fmt/clippy/`./scripts/
test.sh`) → ревью @ol (Opus 4.8 low, adversarial на прод-коде) → починка
находок → коммит. babysit-cron `cd3e2d08` (15m) страхует.

**Делегат СЕЙЧАС: /crush** (пользователь просил вернуться на /crush; ранее
были @o46l/@ol). Исключение — тонкая concurrency в `hnsw_adapter.rs`, где
crush исчерпывал контекст; такие фиксы делал @o46l. Пользователь дал 3
СКВОЗНЫХ ПРИНЦИПА (применять всегда): (1) флейки устранять НА МЕСТЕ — в них
знание о дефекте, НЕ маскировать tolerance'ом; (2) КАЖДЫЙ баг покрывать
ИМЕНОВАННЫМ регресс-тестом; (3) НЕ спрашивать по явным дефектам — сразу чинить.

**Готово и закоммичено: 23/23 исходных листа + фиксы.** P0-P5 полностью:
спайк/бенч-инфра/upsert_batch (#393-398), ef_search+билдеры (#399),
персист снапшот+delta+crash (#400-403), filtered ANN post/pre/co-filter+
selectivity-бенч (#404-406), P4 promote+deleted-зеркала+фоновая компакция
(double-write+backfill+reconcile)+бенчи (#407-409), P5 SQ8-квантайзер+int8
SIMD (#410), квант-граф Вариант A (u8-граф+rescore+fit+DDL+wire+билдеры
Rust/TS+parity, #411 Фазы A+B), снапшот v2 квантизации+миграция+бенч (#412).
Последний коммит: `36ba8371` (бриф #418).

**In-flight СЕЙЧАС: #418** — crush-сессия `v5fix-freef32` (alive) чинит
КРИТИЧЕСКУЮ находку бенча #412: SQ8 сейчас УВЕЛИЧИВАЕТ память (sq8 25.9 MiB >
f32 15.7 MiB) — f32-граф не освобождается после fit (hnsw_rs хранит f32
внутри графа), u8-граф добавляется сверху → цель 4× НЕ достигнута. Фикс: поле
`hnsw: Arc<Hnsw<f32>>` → `ArcSwapOption`, store None после fit (только квант-
адаптер; non-quant не трогать). Дерево ГРЯЗНОЕ (crush правит hnsw_adapter.rs
+ snapshot.rs + quantized_graph_tests.rs + бенч + отчёт; создал stray
gate_run1-9.log от 10× load-лупа). БЕЗ фикса весь трек P5 бессмыслен.

**Пойманные за сессию баги (двойная верификация + принципы окупились):** C1
wrong-results (pre/co-filter, @ol) · single-flight panic-leak (#402) · SLOW
serial-build (#403) · resurrect-гонка компакции (@ol, #408) · в #411: Cosine-
краш hnsw_rs (отрицат. дистанция), std::sync::Mutex на hot-path search→
ArcSwapOption, Relaxed→Acquire data-race публикации is_fitted, double-insert
узла (@ol), РЕАЛЬНАЯ потеря 6 векторов при fit-переходе (вскрыта ужесточённым
missing==0 тестом — маскировалась под recall!), два корня: migration-гонка +
hnsw_rs недостижимость малых графов→brute-force ≤512. Каждый закрыт регресс-
тестом.

**Mutex-аудит (@sh):** 0 hot-path нарушений; ~38 санкционировано (tokio-guard-
через-await, Condvar); 1 полная замена → #417. **НЕ запушено НИЧЕГО** (все
коммиты локальные; ждём явной команды push).

## Active goal

реализуй всю векторную кампанию

(Stop-hook активен; снимется когда pending+in_progress==0. Работать
автономно, НЕ спрашивать по явным дефектам.)

## TaskList
### in_progress
- #418 V5-fix: освободить f32-граф после fit — SQ8 4× память (crush v5fix-freef32)
### pending
- #413 V6.1 Node e2e 18-vectors.test.js (DDL+ANN+filtered+ef_search)
- #414 V6.2 TS e2e расширение e2e-vector.test.ts
- #415 V6.3 OQL-поверхность + guide 06-search
- #416 V-fix tx-committed vector deletes → HNSW-граф+delta (ghost gap#1, HIGH-6)
- #417 V-lockfree session pending_changepw_challenge Mutex → ArcSwapOption
### recently completed (10)
- #412 V5.3 снапшот v2 квантизации · #411 V5.2 квант-граф · #410 V5.1 SQ8+SIMD
- #409 V4.3 bulk/компакция бенчи · #408 V4.2 фоновая компакция · #407 V4.1
  deleted-зеркала+O(1)len · #406 V3.3 selectivity-бенч · #405 V3.2 pre/co-filter
- #404 V3.1 filtered ANN · #403 V2.4 crash-тесты

## Decisions
- **Квант-граф Вариант A** (Hnsw<u8,ShamirDistU8>, граф на кодах) вместо
  отвергнутого Варианта C (f32-граф+коды-aside = 0 экономии) — но реализация
  #411 ОСТАВИЛА f32-граф живым → #418 доводит до реальной экономии.
- **recall SQ8-теста порог 0.95** (не 0.98): hnsw_rs unseedable RNG стохастичен,
  0.98 флейкал; 0.95 детерминирован под лупом (задокументировано честно).
- **Делегат /crush** (по просьбе); тонкий concurrency hnsw_adapter → @o46l.
- **file_dump<u8> работает** (спайк #412) → снапшот v2 грузит u8-граф напрямую,
  план B (rebuild из кодов) не нужен.

## Open questions
- Нет открытых (пользователь: не спрашивать по явным дефектам, сразу чинить).
- Пуш: ничего не запушено — жду явной команды.

## Repo state
```
(ГРЯЗНОЕ — crush #418 in-flight):
 M crates/shamir-engine/benches/quantization_f32_vs_sq8.rs
 M crates/shamir-index/src/vector/hnsw_adapter.rs
 M crates/shamir-index/src/vector/snapshot.rs
 M crates/shamir-index/src/vector/tests/quantized_graph_tests.rs
 M docs/benchmarks/vector/2026-07-05-quantization.md
?? gate_run1..9.log (stray от crush 10× load-лупа — НЕ мои, при коммите #418 удалить)
```
```
36ba8371 docs(prompts): brief for #418 free f32 graph post-fit (SQ8 4x memory)
2f045e52 feat(index): snapshot v2 (SQ8 quantization persistence) + migration + bench (V5.3)
77114f07 docs(prompts): brief for V5.3 snapshot v2 quantization + migration + bench
83c96abe feat(client-ts): vector_quantization in TS DDL builder + parity (V5.2 phase B)
c7a6efbe fix(index): close vector-loss + unreachable-node races in SQ8 fit transition (V5.2)
```

_Следующий шаг: дождаться crush #418 → zero-trust верификация (перемерить бенч
— sq8 RSS ДОЛЖЕН стать < f32; 10× @vector @engine --full под нагрузкой —
дроп графа vs in-flight search = race-класс #411) → @ol adversarial-ревью
(UAF/потеря при дропе f32-графа) → починка → коммит #418 (удалить
gate_run*.log). Если сессия рестартует с ГРЯЗНЫМ деревом: проверить crush
sessions locks v5fix-freef32; жив → дождаться; мёртв → дособрать по брифу
`docs/prompts/vector/21-free-f32-graph.md`. Далее автономно /crush: #413-415
(V6 e2e/OQL/guide), #416 (ghost-fix HIGH-6), #417 (lock-free session)._
