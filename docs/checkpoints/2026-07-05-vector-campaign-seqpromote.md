בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-05 22:00 [vector-campaign-seqpromote]

## Session summary

РЕАЛИЗАЦИЯ векторной кампании по `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
(/babygoal «реализуй всю векторную кампанию»). Конвейер: prompt-first бриф в
`docs/prompts/vector/<NN>-*.md` (коммит ДО запуска) → реализация **/crush**
(текущий делегат по просьбе пользователя; тонкая concurrency hnsw_adapter
ранее — @o46l) → ЛИЧНАЯ zero-trust верификация (fmt/clippy/`./scripts/test.sh`,
на прод-коде под нагрузкой N×) → adversarial-ревью **@ol** → починка → коммит.
babysit-cron `cd3e2d08` (15m). ТРИ СКВОЗНЫХ ПРИНЦИПА пользователя: (1) флейки
устранять НА МЕСТЕ (в них знание о дефекте); (2) КАЖДЫЙ баг → именованный
регресс-тест; (3) НЕ спрашивать по явным дефектам — сразу чинить.

**Закоммичено: 26 листов/фиксов.** Все 23 исходных (P0–P5: спайк→бенч-инфра→
upsert_batch→ef_search→персист(снапшот+delta+crash)→filtered ANN(post/pre/co+
selectivity)→компакция(double-write+reconcile)→SQ8(квантайзер+SIMD→u8-граф
Вариант A+DDL+wire+билдеры Rust/TS+parity→снапшот v2+миграция)) + #418
(освобождение f32-графа после fit — SQ8 теперь РЕАЛЬНО ~¼ памяти f32, @ol
APPROVE) + #416 (gap#1 HIGH-6: tx-deletes до графа+delta, вкл. update-ветку
по находке @ol, 6 регресс-тестов, 1765/1765).

**In-flight СЕЙЧАС: #420** — crush `vfix-seqpromote` (alive) чинит ПРЕ-
СУЩЕСТВУЮЩИЙ баг потери данных, найденный при #416: tx1 insert vector A +
commit (ок) → tx2 insert B + commit → B в графе, но search(B) score 0.0 —
второй последовательный Phase 5d промоут кладёт НЕ ТОТ вектор (staging
верен). Бриф `docs/prompts/vector/23-sequential-promote-loss.md` (red-first
регресс-тест → корень → фикс). Дерево грязное: crush правит
tx_vector_delete_tests.rs + создал sequential_promote_diag_tests.rs +
stray reg1.log/reg_run.log (удалить при коммите).

**Очередь (pending):** #413 Node e2e 18-vectors · #414 TS e2e расширение ·
#415 OQL+guide 06-search · #417 lock-free session challenge (единственная
замена из mutex-аудита @sh: 0 hot-path нарушений, ~38 санкционировано) ·
#419 WAL-флейк crash_at_mid_delete (редкий, вне vector).

**Пойманные баги сессии** (все с регресс-тестами): C1 wrong-results · single-
flight leak · resurrect-гонка компакции · Cosine-краш · Mutex hot-path →
ArcSwapOption · Relaxed→Acquire race · double-insert узла · потеря 6 векторов
при fit (2 корня: catch-up сходимость + hnsw_rs недостижимость малых графов
→ brute-force ≤512) · SQ8-память (dual-graph floor → #418) · ghost tx-delete
(#416) · update-removes-embedding ghost (@ol) · sequential-promote loss (#420,
в работе). НИЧЕГО не запушено (все коммиты локальные).

## Active goal

реализуй всю векторную кампанию
(Stop-hook активен; автономно, не спрашивать по явным дефектам.)

## TaskList
### in_progress
- #420 V-fix: потеря второго последовательного tx vector-промоута (crush vfix-seqpromote)
### pending
- #413 V6.1 Node e2e 18-vectors.test.js
- #414 V6.2 TS e2e расширение
- #415 V6.3 OQL + guide 06-search
- #417 V-lockfree session challenge Mutex→ArcSwapOption
- #419 flake-hunt crash_at_mid_delete (WAL child sidecar race)
### recently completed (10)
- #416 tx-vector-delete ghost-fix (d265b576) · #418 free f32 graph (35f853ff) ·
  #412 снапшот v2 (2f045e52) · #411 квант-граф A+B (222ed303+83c96abe+c7a6efbe) ·
  #410 SQ8+SIMD · #409 бенчи · #408 компакция · #407 O(1) len

## Decisions
- #416 вариант A: staged_vector_deletes в TxContext зеркалит insert-staging;
  delete-then-insert порядок в Phase 5d (конвергентен с tombstone/upsert).
- replace-тест #416 на staging-контракте (обход #420) — приемлемо по @ol,
  живая tombstone-проверка сохранена.
- fmt-дрейф #418 (bench+bench-utils) — отдельный style-коммит 5b026d72.
- Mutex-аудит: только session-challenge заменять (#417), санкционированные
  tokio-guard/Condvar НЕ трогать (пользователь подтвердил).

## Open questions
- Нет. Пуш — только по явной команде (не запушено).

## Repo state
```
(ГРЯЗНОЕ — crush #420 in-flight):
 M crates/shamir-engine/src/tx/tests/tx_vector_delete_tests.rs
?? crates/shamir-index/src/vector/tests/sequential_promote_diag_tests.rs
?? reg1.log  reg_run.log   (stray crush — удалить при коммите #420)
```
```
41704a8c docs(prompts): brief for #420 sequential tx vector promote loss
d265b576 fix(tx): wire tx-committed vector deletes into the HNSW graph + delta (#416, gap#1)
5b026d72 style(bench): rustfmt quantization_f32_vs_sq8 (fmt drift from #418)
9668f622 docs(prompts): brief for #416 tx-committed vector delete -> graph + delta (gap#1)
35f853ff perf(index): free the f32 graph after SQ8 fit — 4x memory saving realised (#418)
```

_Следующий шаг: дождаться crush #420 → zero-trust верификация (красный→зелёный
регресс-тест sequential_tx_vector_promotes_both_searchable, точный корень, гейт
@vector @engine --full 2×+) → @ol ревью если прод-код тонкий → коммит (удалить
reg*.log; diag-тест-файл либо оформить, либо crush уберёт) → далее /crush:
#413→#414→#415 (V6), #417, #419. Если сессия рестартует с грязным деревом:
`crush sessions locks vfix-seqpromote`; жив → ждать; мёртв → дособрать по
брифу 23-sequential-promote-loss.md._
