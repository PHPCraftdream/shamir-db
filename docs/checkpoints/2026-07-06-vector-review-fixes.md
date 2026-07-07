בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-06 08:19 [vector-review-fixes]

## Session summary

ВЕКТОРНАЯ КАМПАНИЯ ЗАВЕРШЕНА (30 коммитов, #393–#422, финальный 10×-прогон
@vector @engine --full = 10×1767/1767, fmt --all чист). После завершения
пользователь запустил ревью кампании агентом **@fh** (качество/ошибки/
осторожность/комплаенс/полнота). Ревью нашло: **Б-1 HIGH CONFIRMED** —
fit-переход теряет graph-связность (c7a6efbe удалил parallel_insert из
catch-up/self-migration, коммент ссылается на несуществующий код; векторы
окна fit невидимы для graph-search при len>512, регресс-тест на 400
векторах бил по брутфорсу); **Б-2 HIGH design** — durability Phase 5d
(провал/crash после ack → мутация нигде, rebuild-on-open с V2.2 не
reconcile'ит); Б-3 (convergence-check инфлируется post-flip вставками),
Б-4 (пустой search в окне fit), Б-5 (read-your-own-writes только на
post-filter пути), Б-6, О-1 (мёртвый dot_u8 на hot-path + нет норм-кэша
Cosine), О-2, К-1 (inline-тесты filtered_vector.rs), П-1 (компакция НЕ
quantization-aware — SQ8 теряет квантизацию), П-3 (два vector-индекса на
таблицу ломают промоут), П-4.

По команде пользователя («реализуй план, используй /crush, ревью сам
делай» — @ol НЕ используется, ревью диффов делает оркестратор) заведены
таски #423–#432 (VR-1..VR-10) с blockedBy-цепочкой и запущена ВОЛНА 1:
три параллельных crush-агента с непересекающимися файлсетами —
`vr1-fit` (#423, hnsw_adapter.rs), `vr3-design` (#425, только дизайн-док
docs/design/vector-phase5d-durability.md), `vr10-multiidx` (#432,
DDL-валидация + guide/BACKLOG). Брифы закоммичены (703364f6:
31-fit-graph-connectivity.md, 32-phase5d-durability-design.md,
33-multi-vector-index-validation.md), во всех стоит ⛔-запрет git-мутаций
(пользователь это явно перепроверял). Babysit-cron `85d8de87` (15m).

Конвейер: crush → личная zero-trust верификация (дифф + гейт 1× — по
договорённости с пользователем МЕЖДУ тасками ровно 1 прогон гейта,
нагрузочный 10× только в самом конце) → ревью сам → коммит → следующая
волна. Принципы: флейки чинить на месте; каждый баг → именованный
регресс-тест; не спрашивать по явным дефектам. НИЧЕГО не запушено.

## Active goal

нет формального /goal; рабочая цель — реализовать план VR-фиксов
(#423–#432) по ревью кампании.

## TaskList

### in_progress
- #423 VR-1: fit-переход теряет graph-связность (Б-1) + ранний convergence-exit (Б-3) — crush vr1-fit
- #425 VR-3: дизайн durability Phase 5d (Б-2/П-2) — crush vr3-design
- #432 VR-10: валидация мульти-vector-index (П-3) + докнота fit-порога (П-4) — crush vr10-multiidx

### pending
- #424 VR-2: пустой search в окне fit (Б-4) — retry в u8-ветку (blockedBy: #423)
- #426 VR-4: реализация durability Phase 5d (blockedBy: #425 — дизайн утверждает оркестратор)
- #427 VR-5: read-your-own-writes pre/co-filter (Б-5) (blockedBy: #423)
- #428 VR-6: quantization-aware компакция (П-1) (blockedBy: #423, #424)
- #429 VR-7: /opti dot_u8 + норм-кэш Cosine (О-1) (blockedBy: #428)
- #430 VR-8: delete-гонка с флипом (Б-6) + fit в spawn_blocking (О-2) (blockedBy: #429)
- #431 VR-9: style inline-тесты filtered_vector.rs → tests/ (К-1) (blockedBy: #427)

### recently completed (кампания)
- #422 WAL bounded-segment (1f207fff) · #421 6 e2e-падений (8728c40c) ·
  #419 флейк crash_at_mid_delete (f5efc08d) · #417 lock-free changepw
  (b20584ac) · #415 guide 06-search (97899df6) · #414 TS e2e (8c174314) ·
  #413 Node e2e + msgpack glue (6c207a17) · #420 регресс-guard (9a25262d) ·
  #416 · #418

## Decisions

- Ревью VR-диффов делает ОРКЕСТРАТОР сам (пользователь: «ревью сам делай»)
  — @ol в конвейере VR-фиксов не используется.
- Между тасками 1 прогон гейта; 10×-луп — только в самом конце
  (пользователь явно попросил).
- Цепочка hnsw_adapter (#423→#424→#428→#429→#430) строго последовательна
  (один файл); #427→#431 и #432 — параллельные ветки.
- #425 — только дизайн-док; реализацию (#426) утверждает оркестратор по
  доку (варианты: delta-до-ack / reconcile / WAL-повтор).
- Попутная находка #422 (PermissionDenied lingering в truncate_below)
  занесена в BACKLOG.md (460de464), не в TaskList.

## Open questions

- Нет. Пуш — только по явной команде (не запушено).

## Repo state
```
(ГРЯЗНОЕ — crush волна 1 in-flight):
 M crates/shamir-engine/src/table/table_manager_index_mgmt.rs   (vr10)
 M crates/shamir-engine/src/table/tests/mod.rs                  (vr10)
 M crates/shamir-index/src/vector/hnsw_adapter.rs               (vr1)
?? crates/shamir-engine/src/table/tests/multi_vector_index_guard_tests.rs (vr10)
?? docs/checkpoints/*.md (untracked чекпоинты)
```
```
703364f6 docs(prompts): briefs for VR-1 (#423), VR-3 (#425), VR-10 (#432) — review fix wave 1
460de464 docs(backlog): WAL truncate_below PermissionDenied segment lingering (found in #422)
1f207fff test(wal): pin the bounded-segment invariant deterministically (#422)
8728c40c fix(e2e): close six pre-existing Node e2e failures after replication drift (#421)
82c4c7bd docs(prompts): brief for #422 shamir-wal bounded segment count failure
```

_Следующий шаг: дождаться crush-агентов волны 1 → zero-trust верификация
каждого (дифф + гейт 1×: #423 — @vector @engine --full + регресс >512
через graph-путь; #425 — прочитать дизайн-док, УТВЕРДИТЬ вариант, написать
бриф #426; #432 — -p shamir-engine + валидационный регресс) → ревью сам →
коммит по одному → волна 2 (#424 после #423; #427 параллельно; #426 после
утверждения) → ... → #429 /opti с бенчами (CARGO_TARGET_DIR=D:/dev/rust/
.cargo-target-bench, forward slashes!) → финальный 10×-прогон. Если сессия
рестартует с грязным деревом: `crush sessions locks` — vr1-fit /
vr3-design / vr10-multiidx живы → ждать; мёртвы → дособрать по брифам
31/32/33-*.md._
