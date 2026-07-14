בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-05 [vector-plan]

## Session summary

Сессия закрыла хвост репликации (#388/#389), навела чистоту в репозитории и
целиком спроектировала кампанию доработки **векторной подсистемы** до
production-ready, заведя её в TaskList (23 таски #393–#415). Ничего из
векторного ещё НЕ реализовано — только план + доки + таски; работа не начата
(ждём «поехали» от пользователя).

Хронология: (1) `/resume` подхватил чекпоинт репликации; (2) через sh-агентов
доделаны #388 (двухсерверный Node e2e — написан, требует MSVC-хоста для
прогона) и #389 (nextest test-group `wasm-heavy` от CPU-голодания WASM-тестов),
плюс мой фикс fallout от #392 (поле `replication` в `Config` ломало 4
struct-литерала в тестах shamir-client); всё закоммичено и **запушено** на
origin/master; (3) очистка корня: `opt_crush/`→`docs/dev-artifacts/audits/`, снёс
`.flamegraphs/`, `run.log` untrack+gitignore; (4) `.gitattributes` обогащён +
`core.safecrlf=false` локально (убрать CRLF-предупреждения); (5) изучены
vector-роадмапы, через Explore+Plan-агентов собрана карта реального кода,
написан исполняемый план и разбит на листы; (6) заведены таски.

Ключевой продукт сессии — два дока (закоммичены `3b622c94`, `5dd3675d`):
`docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md` (ревизия по факту кода: поправки
K1–K8, фазы P0–P5+V6, дизайн персистентности, решения) и
`docs/dev-artifacts/design/where-select-binds.md` (отдельный языковой трек — как отдавать
значение, вычисленное предикатом в where; склон к `$meta`-каналу).

Живые факты кода (проверено разведкой): HNSW in-memory `hnsw_rs 0.3.4`, граф
теряется на рестарте (rebuild-скан); ранжирующее число (дистанция/BM25)
отбрасывается в `read_exec.rs:327-330` `map(|(r,_)| r)`; `len()` зовёт
`deleted.len()` O(N) на каждом search (K8, чинится в V4.1); FTS уже в коде
(`bm25.rs`, `fts_ranked_backend.rs`) → hybrid разблокирован, но вне кампании;
образец chunk-persist — `InternerManager`. hnsw_rs 0.3.4 API (file_dump/
HnswIo/parallel_insert/search_filter) — источники противоречат, поэтому V0.0 —
спайк-контракт ДО кода.

Таймеры/агенты: нет активных cron/babysit. Активной /goal нет. Работа руками
поручается sh-агентам (Sonnet top) по prompt-first; пользователь ранее в
сессии авторизовал sh-агентов и коммиты.

## Active goal

none (Stop-hook «реши все текущие задачи» с ранней части сессии авто-снялся,
когда #388/#389 закрылись; новый не ставился)

## TaskList

### in_progress
(пусто)

### pending — векторная кампания (23 таски, все pending)
P0: #393 V0.0 спайк hnsw_rs (без блокеров) · #394 V0.1 bench-инфра (без
блокеров) · #395 V0.2 upsert_batch [393] · #396 V0.3 criterion [394,395] ·
#397 V0.4 vector_report [394,395] · #398 V0.5 baseline [396,397]
P1: #399 V1.1 ef_search [395]
P2: #400 V2.1 snapshot-кодек [393,395] · #401 V2.2 startup [400] · #402 V2.3
delta-log [401] · #403 V2.4 crash+бенч [402]
P3: #404 V3.1 post-filter [398,399] · #405 V3.2 pre/co-filter [404] · #406
V3.3 selectivity-бенч [405]
P4: #407 V4.1 promote+len()fix [395] · #408 V4.2 компакция [407,402] · #409
V4.3 бенчи [408]
P5: #410 V5.1 SQ8+SIMD [393] · #411 V5.2 граф+DDL [410,400] · #412 V5.3
снапшот v2 [411,408]
V6 (полный стек): #413 Node e2e 18-vectors [399,405,411] · #414 TS e2e [399,
405,411] · #415 OQL+guide [406,412]

Готовы к старту прямо сейчас (без блокеров): **#393, #394**.
Критический путь persist: 393→395→400→401→402→403.

### recently completed (последние 10)
#392 386-c supervisor boot · #391 386-b subscription lifecycle · #390 386-a
репл-DDL execution · #387 Node e2e репликации · #386 серверное исполнение
репл-DDL · #385 MVCC-attach · #384 R1-d e2e · #383 R1-c pull-loop · #382 R1-b
bookmark · #381 R1-a apply_replicated · (#388/#389 закрыты в этой сессии)

## Decisions

- **Объём векторной кампании — только ядро P0–P5 (+V6 клиентская поверхность).**
  Отвергли: P6 Hybrid RRF и Layer 1 embedders (вне кампании; hybrid
  разблокирован, добавляем позже). Пользователь: «полнотекст с вектором не
  объединяется» (я мягко поправил — hybrid не сливает данные, а фьюзит два
  ранкинга; но решение оставили — core-only).
- **Бенчи — только QUICK.** FULL-заглушку в shamir-bench-utils НЕ снимаем
  (полные прогоны на векторных ступенях нигде реально не отработают). Отвергли
  re-enable FULL и отдельный строгий инструмент.
- **`$score` отвергнут** как магичное метаполе. Вместо него — отдельный трек
  «where-бинды» (`docs/dev-artifacts/design/where-select-binds.md`): `bind` на предикате +
  форма возврата; склон к варианту **B** ($meta-канал, объект неприкосновенен)
  над вариантом A (bound как элемент select). Вне векторной кампании.
- **Сквозное требование пользователя:** доработки покрывают ВСЕ уровни — unit,
  OQL, Query Builders (Rust+TS), Node e2e, TS e2e, бенчмарки → отдельная фаза
  V6 + уровни внутри каждого листа фичи.
- **Делегирование — sh-агенты** (Sonnet top) по prompt-first (бриф в
  `docs/dev-artifacts/prompts/vector/<NN>-*.md`, коммит ДО запуска).

## Open questions

- **Как запускать реализацию** — жду «поехали»: sh-агенты по одному /
  параллельно / `/babygoal`-конвейер с babysit. Не начато намеренно.
- Рабочие дефолты листов (можно пересмотреть в брифах): R2 пин
  `hnsw_rs = "=0.3.4"`; R3 `Box::leak` HnswIo (boot-only); R4 dev-dep
  `memory-stats` для RSS; R5 delta-log = двойная запись между снапшотами;
  R7 1M-ступень только ручной эксперимент за env.
- Финал варианта where-биндов A vs B — решается на старте того трека (не сейчас).

## Repo state

```
?? docs/dev-artifacts/checkpoints/2026-07-02-replication-design.md
?? docs/dev-artifacts/checkpoints/2026-07-03-replication-complete.md
(+ этот файл после записи; рабочее дерево иначе чисто, всё закоммичено)
```

```
5dd3675d docs(roadmap): vector plan — full-stack coverage phase V6 (e2e/TS/OQL)
3b622c94 docs(roadmap): vector production execution plan + where-select binds design
78539c00 chore(gitattributes): explicit LF rules for source + dotfiles, silence CRLF noise
616358a1 chore(repo): tidy root — move opt_crush to docs/audits, drop flamegraphs, untrack run.log
23aaf331 test(e2e): two-server leader->follower convergence (#388)
```

_Следующий шаг: по команде пользователя стартовать #393 (спайк hnsw_rs 0.3.4,
без блокеров) + #394 (bench-инфра) через sh-агентов, prompt-first. Незапушены:
локальные doc/chore-коммиты этой сессии после 23aaf331 (репликация запушена)._
