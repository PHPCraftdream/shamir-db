בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-06 09:54 [panel-review-audits]

## Session summary

Продолжение сессии после завершения ВЕКТОРНОЙ КАМПАНИИ (#393-422, 30 коммитов,
финальный 10×-прогон зелёный). Пользователь запустил ревью самой кампании
агентом **@fh** (качество/ошибки/осторожность/комплаенс/полнота) — нашло Б-1
HIGH CONFIRMED (fit-переход теряет graph-связность), Б-2 HIGH design (durability
Phase 5d), Б-3..Б-6, О-1/О-2, К-1, П-1/П-3/П-4. По этому ревью заведены таски
#423-432 (VR-1..VR-10) с blockedBy-цепочкой (hnsw_adapter-путь строго
последователен: #423→#424→#428→#429→#430; #427→#431 и #432 — параллельные
ветки). Пользователь: «реализуй план, используй /crush, ревью сам делай» —
**@ol НЕ используется** в этом конвейере, ревью диффов делает оркестратор лично.

**Волна 1 (VR-1/#423, VR-3/#425, VR-10/#432) запущена параллельно через crush**
(непересекающиеся файлсеты). VR-3 (дизайн-док durability) завершился первым —
я прочитал `docs/design/vector-phase5d-durability.md`, утвердил **Вариант A**
(delta-append внутри commit critical section до publish; graph-half остаётся
post-lock; идемпотентность через существующий `replay_delta` LWW), закоммитил
(`8d67a710`), закрыл #425, написал бриф #426 (`563a1177`) и запустил crush
`vr4-impl` на реализации. Параллельно `vr1-fit` (#423) чинил
graph-связность в hnsw_adapter.rs, `vr10-multiidx` (#432) — DDL-валидацию
второго vector-индекса — **этот таск готов и отревьюен мной** (guard в
правильном месте, честный корень в комментарии, BACKLOG-строка), ждёт коммита
после того как параллельные правки в других файлах engine/index устаканятся
(vr10 трогал table_manager_index_mgmt.rs, который НЕ пересекается с vr1/vr4 по
плану, но git status показывает чужие in-flight правки рядом).

**Пользователь остановил vr1-fit и vr4-impl** («останови агента, потом
продолжим его сессию») — я убил процессы (`taskkill` по дереву PID, т.к.
`crush sessions kill` не сработал из-за занятого lock-файла), `crush sessions
reap` снял orphan-locks. **crush-сессии `vr1-fit` и `vr4-impl` СОХРАНЕНЫ** —
продолжить можно `crush run --session vr1-fit`/`--session vr4-impl` с
продолжающим промтом. Рабочее дерево грязное: правки vr1 (hnsw_adapter.rs +
quantized_graph/quantization_snapshot тесты) и vr4 (commit.rs, commit_phases.rs,
materialize.rs, commit_phase5_tests.rs, crash_recovery.rs — crash-seam
phase5d_delta) остались НЕЗАКОММИЧЕННЫМИ и незавершёнными (прерваны на
середине работы). VR-10 готов, но коммит отложен.

**Затем пользователь запросил широкое ревью ВСЕГО проекта** («что мы обязаны
улучшить, где осторожность упускает, что улучшить, что ускорить») через
**панель из 5 параллельных агентов @fxx (max effort)**, каждый на своей
области: (1) durability storage/wal/tx, (2) конкурентность engine, (3) перф
hot-paths, (4) security сетевой поверхности, (5) клиентская поверхность
parity. Каждому агенту явно указано анализировать in-flight файлы **по HEAD**
(`git show HEAD:<path>`), не по грязному дереву, и не переоткрывать
VR-находки. Все 5 агентов завершились и вернули отчёты. Я свёл их в единую
сводку и **записал все 6 файлов** (5 отчётов + SUMMARY) в `docs/audits/` —
**НЕ закоммичены** (по обычному правилу — коммит только по явной команде).

**Ключевой результат панели** — найдены CRITICAL находки, независимо от
VR-кампании и БОЛЕЕ серьёзные, чем векторные баги:
- Durability: recovery глотает ошибку записи history и всё равно `mark_durable`
  → truncation стирает единственную копию ack-коммита (`recovery.rs:413,265`);
  truncation WAL не гейтится на interner-hwm → после crash id переиспользуется
  под другое имя (`drainer.rs:531`); `drain_to_history` метит durable ЧУЖУЮ
  версию — **подтверждено НЕЗАВИСИМО двумя агентами** (durability §1.4 +
  concurrency A5) — RENAME стирает данные другой таблицы.
- Concurrency: lock-free commit не сериализует validate→publish → write-skew
  коммитится под Serializable (A1, CRITICAL); `GroupCommit` виснет навсегда
  при отмене лидера — **тоже подтверждено независимо** (durability §2.1 +
  concurrency A7).
- Security: `Subscribe` не проверяет per-table read-ACL → live-утечка любой
  таблицы; WASM-хост компилирует недоверенный Rust с полным env → эксфильтрация
  + `db_execute` без actor-скоупа → межтенантный доступ.
- Perf: FTS/index2 пустой ответ → full scan, для VectorSimilarity отдаёт ВСЕ
  строки (~400× на miss) — уже видно в собственном бенче с неверным
  комментарием.
- Client: TS `.limits()` ГАРАНТИРОВАННО роняет запрос (нет `max_nesting_depth`
  в TS-типе), причём юнит-тест закрепляет баг как эталон.

Сессия сейчас в состоянии ожидания решения пользователя: продолжать ли VR-
конвейер (#423-432) или переключиться на новую кампанию по находкам панели
(durability-first? security-first? единая кампания по всем CRITICAL?).
babysit-cron `85d8de87` (15m) — жив, репортит blocked (VR на паузе намеренно).

## Active goal

Нет явного /goal. Рабочая цель предыдущего этапа — VR-фиксы по ревью
векторной кампании (#423-432), сейчас на паузе. Ожидается решение
пользователя по приоритетам после панельного ревью проекта.

## TaskList

### in_progress (VR — НА ПАУЗЕ, агенты остановлены пользователем)
- #423 VR-1: fit-переход теряет graph-связность (Б-1) + ранний convergence-exit (Б-3) — crush-сессия vr1-fit остановлена, СОХРАНЕНА, дерево грязное (незавершённые правки hnsw_adapter.rs)
- #426 VR-4: реализация durability Phase 5d по утверждённому дизайну (Вариант A) — crush-сессия vr4-impl остановлена, СОХРАНЕНА, дерево грязное (commit.rs/commit_phases.rs/materialize.rs/crash_recovery.rs)
- #432 VR-10: валидация мульти-vector-index (П-3) + докнота fit-порога (П-4) — ГОТОВ, отревьюен мной, коммит отложен

### pending
- #424 VR-2: пустой search в окне fit (Б-4) (blockedBy: #423)
- #427 VR-5: read-your-own-writes pre/co-filter (Б-5) (blockedBy: #423)
- #428 VR-6: quantization-aware компакция (П-1) (blockedBy: #423, #424)
- #429 VR-7: /opti dot_u8 + норм-кэш Cosine (О-1) (blockedBy: #428)
- #430 VR-8: delete-гонка с флипом (Б-6) + fit в spawn_blocking (О-2) (blockedBy: #429)
- #431 VR-9: style inline-тесты filtered_vector.rs → tests/ (К-1) (blockedBy: #427)

### completed
- #425 VR-3: дизайн durability Phase 5d — Вариант A утверждён (8d67a710)
- #393-422: вся векторная кампания (30 коммитов)

### НЕ заведено (новые находки панели — ждут решения пользователя)
- 8 CRITICAL находок из docs/audits/2026-07-06-SUMMARY.md (см. Decisions)

## Decisions

- VR-конвейер: ревью диффов делает оркестратор САМ (пользователь: «ревью сам
  делай») — @ol НЕ используется для VR-задач, в отличие от векторной кампании.
- #425: утверждён Вариант A (delta-append pre-publish) из трёх дизайн-вариантов
  — минимальное изменение contract surface, идемпотентность через существующий
  replay_delta, не трогает WAL schema/cold-start.
- Панельное ревью: 5 агентов @fxx, каждому — явное указание анализировать
  in-flight файлы по HEAD и не переоткрывать VR-находки (дедупликация усилий).
- Отчёты панели ЗАПИСАНЫ в docs/audits/, но НЕ закоммичены (обычное правило —
  коммит по явной команде).
- Пользователь остановил vr1-fit/vr4-impl намеренно («потом продолжим его
  сессию») — crush-сессии НЕ убиты полностью, только процессы; продолжение
  возможно через `crush run --session <name>`.

## Open questions

- **Главный открытый вопрос**: что делать дальше — продолжить VR-конвейер
  (#423-432, приостановлен) или переключиться на находки панельного ревью
  (durability CRITICAL находки серьёзнее оставшихся VR-задач)? Пользователь
  ещё не ответил на предложение оркестратора «приостановить VR, завести
  кампанию по панельным находкам».
- Не запушено ничего.

## Repo state
```
(ГРЯЗНОЕ — vr1-fit/vr4-impl остановлены с незавершёнными правками,
 vr10-multiidx готов но не закоммичен, audits-отчёты не закоммичены):
 M crates/shamir-engine/src/table/table_manager_index_mgmt.rs   (vr10 — ГОТОВ)
 M crates/shamir-engine/src/table/tests/mod.rs                  (vr10 — ГОТОВ)
 M crates/shamir-engine/src/tx/commit.rs                        (vr4 — недоделан)
 M crates/shamir-engine/src/tx/commit_phases.rs                 (vr4 — недоделан)
 M crates/shamir-engine/src/tx/materialize.rs                   (vr4 — недоделан)
 M crates/shamir-engine/src/tx/tests/commit_phase5_tests.rs     (vr4 — недоделан)
 M crates/shamir-engine/tests/crash_recovery.rs                 (vr4 — недоделан)
 M crates/shamir-index/src/vector/hnsw_adapter.rs               (vr1 — недоделан)
 M crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs (vr1 — недоделан)
 M crates/shamir-index/src/vector/tests/quantized_graph_tests.rs (vr1 — недоделан)
 M docs/BACKLOG.md                                              (vr10 — ГОТОВ)
 M docs/guide/06-search.md                                      (vr10 — ГОТОВ)
?? clippy_engine.log, clippy_engine_lib.log, clippy_ws.log      (stray, от vr10)
?? crates/shamir-engine/src/table/tests/multi_vector_index_guard_tests.rs (vr10 — ГОТОВ)
?? docs/audits/2026-07-06-*.md (6 файлов, панельное ревью — НЕ закоммичены)
?? docs/checkpoints/*.md (untracked чекпоинты)
```
```
563a1177 docs(prompts): brief for #426 Phase 5d durability implementation (option A)
8d67a710 docs(design): Phase 5d vector durability — option A approved (delta-append pre-publish) (#425, VR-3)
703364f6 docs(prompts): briefs for VR-1 (#423), VR-3 (#425), VR-10 (#432) — review fix wave 1
460de464 docs(backlog): WAL truncate_below PermissionDenied segment lingering (found in #422)
1f207fff test(wal): pin the bounded-segment invariant deterministically (#422)
```

_Следующий шаг: ДОЖДАТЬСЯ РЕШЕНИЯ ПОЛЬЗОВАТЕЛЯ по приоритетам. Варианты на
столе: (а) закоммитить #432 (VR-10, готов) отдельно, затем возобновить
vr1-fit/vr4-impl продолжающим промтом; (б) приостановить VR полностью и
завести кампанию по CRITICAL находкам docs/audits/2026-07-06-SUMMARY.md
(durability recovery/interner/drain_to_history — 3 находки трогают
recovery.rs/drainer.rs/mvcc_store — ВНИМАНИЕ: пересекаются с файлами, которые
правит vr4-impl (commit_phases.rs, materialize.rs) — нужно координировать,
чтобы не запустить два агента на одних файлах); (в) сделать оба параллельно с
чётким разделением файлсетов. Если сессия рестартует: `crush sessions locks`
проверит, живы ли vr1-fit/vr4-impl (должны быть offline/stopped — их
останавливал пользователь); при возобновлении — читать это дерево как
отправную точку, НЕ откатывать (правки представляют реальную незавершённую
работу)._
