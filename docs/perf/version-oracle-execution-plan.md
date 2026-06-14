# Execution Plan: Оракул Версий — делегируемые под-агентам этапы

Исполнительный план к `version-oracle-design.md`. Каждый этап —
самодостаточный work-package: контекст, файлы, задача, ограничения,
gate, критерий «готово», формат отчёта. Копируется в вызов `Agent`.

**Дисциплина (из всей кампании):**
- Под-агент НЕ коммитит и НЕ пушит — возвращает diff, проверяю сам.
- Gate каждого этапа: `cargo fmt --all -- --check` +
  `cargo clippy --workspace --all-targets -- -D warnings` +
  `cargo test --workspace --lib`. Bench — изолированный target-dir.
- Data integrity > performance. На неопределённости в P1/P2 → СТОП.
- Структурные этапы режу мелко (как Stage A: A1→A5), каждый
  independently-компилируемый и под gate.

**Легенда типов агентов:**
- `research` — read-only, без правок кода (aol / Explore / asl).
- `structural` — высокий риск, нужно архитектурное суждение (ao46l).
- `mechanical` — scaffolding, additive, низкий риск (asl).

---

## Граф зависимостей

```
P0 (measure) ─┐
R1 (read audit) ─┼─ [parallel-ok] все три независимы, research-only
R2 (abort census) ─┘
        │ (все три завершены, go подтверждён)
        ▼
P1a (tracker scaffold) → P1b (abort marking) → P1c (watermark advance) → P1d (recovery)
        │ (P1 полностью зелёный)
        ▼
P2a (atomic assign) → P2b (materialize out of lock) → P2c (lock shrink)
        │
        ▼
P3a (footprint accumulator #5) → P3b (stress + crash-injection verify)
```

`P0`/`R1`/`R2` запускаются **параллельно** (три research-агента, разные
области, чтения не конфликтуют). Всё остальное — последовательно.

---

## P0 — MEASURE: профиль commit_mutex hold

- **Agent:** research
- **Prereq:** нет · **[parallel-ok]** с R1, R2
- **Контекст:** `version-oracle-design.md` §P0. Нужно подтвердить, что
  materialize доминирует во времени удержания commit_mutex под
  concurrent load — иначе Оракул не окупится.
- **Файлы:** `crates/shamir-engine/src/tx/commit.rs` (commit_tx_inner),
  `crates/shamir-engine/src/tx/materialize.rs`,
  `crates/shamir-engine/src/tx/pre_commit.rs`,
  `crates/shamir-tx/src/repo_tx_gate.rs` (commit_mutex),
  `crates/shamir-server/benches/wire_pipelining.rs` (sync/n_32, n_128).
- **Задача:** временно инструментировать AtomicU64+Instant вокруг фаз
  под commit_lock: SSI(Phase 2) / version(3) / WAL(4) / materialize(5) /
  publish(6). Прогнать wire_pipelining sync/n_32 и n_128. eprintln
  разбивку µs/% каждые N коммитов. РЕВЕРТ инструментовки в конце.
- **Ограничения:** research-only, никаких правок поведения, `git diff`
  пустой в конце, `cargo check --workspace` после реверта.
- **Done:** таблица «фаза → µs → % lock-hold» при n_32 и n_128 + вывод:
  доминирует ли materialize. Если нет — рекомендация СТОП с цифрами.
- **Отчёт:** разбивка + go/no-go вердикт. <300 слов.

---

## R1 — READ-PATH COHERENCE AUDIT (пред-рефактор)

- **Agent:** research
- **Prereq:** нет · **[parallel-ok]** с P0, R2
- **Контекст:** `version-oracle-design.md` §«Что уже есть» + созерцание.
  Оракул держится на допущении: ВСЯКОЕ чтение данных записи идёт через
  version-reconciliation `MvccStore::get_at(key, snapshot)`. Любой путь,
  читающий «latest» в обход версий, сломается под concurrent
  out-of-order materialize. Главный подозреваемый — **CachedStore**
  (только что мигрирован на TreeIndex, #4).
- **Файлы:** `crates/shamir-tx/src/mvcc_store/mod.rs` (get_at,
  resolve_read), `crates/shamir-storage/src/storage_cached.rs`,
  `crates/shamir-engine/src/table/read_exec.rs`,
  `crates/shamir-engine/src/table/table_manager_streaming.rs`. Плюс grep
  по всем `.get(`, `iter_stream`, `scan_prefix`, прямым обращениям к
  data store в read-путях движка.
- **Задача:** составить ИСЧЕРПЫВАЮЩИЙ список путей чтения данных записи.
  Для каждого классифицировать: (a) идёт через get_at(snapshot)
  version-reconciliation, (b) явно не-MVCC (системные метаданные, DDL,
  interner), (c) **ОБХОД** — читает latest напрямую (мина). Особое
  внимание: ходит ли read_exec в CachedStore напрямую за «текущим», или
  всегда через MVCC get_at.
- **Ограничения:** read-only, ничего не менять. Только классификация.
- **Done:** таблица «путь чтения [file:line] → класс (a/b/c) →
  обоснование». Список мин класса (c) — это пред-рефактор-долг для P1.
  Если мин ноль — премиса Оракула подтверждена, P1 безопасен.
- **Отчёт:** таблица + список мин + вердикт «премиса полна / есть N
  обходов». <500 слов.

---

## R2 — ABORT-PATH CENSUS (пред-рефактор)

- **Agent:** research
- **Prereq:** нет · **[parallel-ok]** с P0, R1
- **Контекст:** `version-oracle-design.md` §Инварианты #2 (hole-tolerance)
  + созерцание. Оракул двигает `assign_version` РАНЬШЕ (fetch_add до
  критической секции). Тогда между assign и publish появляется много
  early-return'ов, и КАЖДЫЙ обязан пометить версию `Aborted` в
  completion tracker — иначе watermark зависнет навечно (liveness-смерть).
- **Файлы:** `crates/shamir-engine/src/tx/commit.rs`,
  `crates/shamir-engine/src/tx/pre_commit.rs` (pre_commit_locked,
  pre_commit_locked_validate), `crates/shamir-engine/src/tx/group_commit.rs`
  (run_leader, run_batch, run_single_tx),
  `crates/shamir-engine/src/tx/materialize.rs`.
- **Задача:** перечислить КАЖДЫЙ путь, который может вернуть Err / abort
  ПОСЛЕ точки assign_version и ДО publish: SSI fail, phantom fail,
  validator fail, unique-constraint fail, materialize err, WAL err,
  follower conflict reject. Для каждого — file:line + причина + где
  логически встанет `completion.mark(version, Aborted)`.
- **Ограничения:** read-only. Учесть: сейчас assign ПОСЛЕ SSI — после
  переноса assign вперёд (P2a) карта путей изменится; описать ОБА
  состояния (сейчас / после P2a).
- **Done:** список «abort-путь [file:line] → причина → точка пометки».
  Полнота критична: один пропуск = зависший watermark.
- **Отчёт:** список + замечание о путях, появляющихся только после P2a.
  <400 слов.

---

## P1a — CompletionTracker scaffolding

- **Agent:** structural
- **Prereq:** P0 (go), R1 (премиса чиста или мины учтены), R2 (список)
- **Контекст:** `version-oracle-design.md` §Дизайн + §Инварианты. Чистая
  additive — новый тип, ещё не подключён к commit. Поведение неизменно.
- **Файлы:** `crates/shamir-tx/src/repo_tx_gate.rs` (рядом с commit_mutex,
  assign_next_version, publish_committed). Новый файл
  `crates/shamir-tx/src/completion_tracker.rs`.
- **Задача:** добавить `CompletionTracker`: version → {Pending,
  Materialized, Aborted}. Методы: `mark(version, state)`,
  `contiguous_watermark() -> u64` (наибольшая V где ∀k≤V: state ∈
  {Materialized, Aborted}). Реализация: плотный кольцевой буфер от
  watermark (версии монотонны, дыры кратковременны) ИЛИ
  `scc::TreeIndex<u64, State>` — выбрать по анализу, обосновать.
  `#[allow(dead_code)]` — не подключать к commit (это P1c). Unit-тесты:
  contiguous advance, дыра блокирует, Aborted пропускает дыру,
  concurrent mark.
- **Ограничения:** additive, поведение commit неизменно. lock-free
  (ideology pillar 1) — atomics/scc, не std::Mutex на hot path.
- **Done:** тип + тесты, всё зелёное, commit-путь не тронут.
- **Отчёт:** структура tracker'а + обоснование выбора + тесты. <300 слов.

---

## P1b — Abort-path marking

- **Agent:** structural
- **Prereq:** P1a, R2 (список путей)
- **Контекст:** подключить `completion.mark(version, Aborted)` к каждому
  abort-пути из переписи R2. Версия пока назначается под lock (assign не
  перенесён — это P2a), так что путей мало; но инфраструктуру ставим
  сейчас, чтобы P2a лёг чисто.
- **Файлы:** по списку R2 (commit.rs, pre_commit.rs, group_commit.rs,
  materialize.rs).
- **Задача:** в каждой точке отмены после assign_version вызвать
  `gate.completion().mark(version, Aborted)`. Версия должна быть
  доступна в точке отмены (если назначается позже — отложить пометку до
  P2a, отметить в отчёте). Тест: tx с форсированным SSI-fail помечает
  свою версию Aborted; watermark перешагивает.
- **Ограничения:** не менять момент assign (P2a). Только проводка mark.
- **Done:** все abort-пути из R2 покрыты; тест на пропуск watermark'а.
- **Отчёт:** покрытые пути + отложенные на P2a. <300 слов.

---

## P1c — Contiguous-prefix publish (подключение watermark)

- **Agent:** structural · **РИСК: высокий**
- **Prereq:** P1a, P1b
- **Контекст:** `version-oracle-design.md` §Инвариант #3. Переключить
  publish с монотонного-под-lock на contiguous-prefix через tracker.
  Читатели НЕ меняются (get_at by snapshot; snapshot = watermark на
  begin_tx) — это и есть красота, read-path уже version-indexed.
- **Файлы:** `crates/shamir-tx/src/repo_tx_gate.rs` (publish_committed /
  publish_committed_max / read-floor), `materialize.rs` (точка publish).
- **Задача:** после materialize версии V → `completion.mark(V,
  Materialized)` + read-floor = `completion.contiguous_watermark()`.
  begin_tx берёт snapshot = текущий contiguous watermark. Сохранить:
  materialize ВСЁ ещё под lock (вынос — P2b); меняется только КАК
  вычисляется опубликованный watermark.
- **Ограничения:** durability перед visibility (инвариант #4). SSI/
  phantom/concurrent/recovery — ВСЕ зелёные. Любой недетерминизм → СТОП.
- **Done:** publish через contiguous watermark; полный SSI/recovery
  набор зелёный; bench wire_pipelining без регрессии.
- **Отчёт:** механика watermark + тесты + bench. <400 слов.

---

## P1d — Recovery: восстановление completion-префикса

- **Agent:** structural · **РИСК: высокий**
- **Prereq:** P1c
- **Контекст:** `version-oracle-design.md` §Инвариант #5. WAL-запись
  несёт версию (WAL v3, commit `0e772ab` уже несёт interner-delta —
  версия туда же, если ещё не там; проверить). Recovery реигрывает
  версии по порядку, восстанавливает completion-префикс, ставит
  watermark на наибольший contiguous materialized.
- **Файлы:** `crates/shamir-engine/src/tx/recovery.rs`,
  `crates/shamir-tx/src/repo_wal_manager.rs` (recover_inflight_v2),
  `crates/shamir-wal/src/wal_entry_v2.rs` (проверить наличие версии в
  записи; добавить если нет — bump аккуратно как A2).
- **Задача:** при recovery для каждой durable WAL-записи: применить
  (как сейчас) + `completion.mark(version, Materialized)`. После всех —
  watermark = contiguous max. Версии в WAL, но не материализованные до
  краха, доигрываются (значит Materialized). Тест: краш между WAL-append
  и materialize → recovery доигрывает → запись видна; watermark корректен.
- **Ограничения:** WAL byte-format совместим (или аккуратный bump с
  legacy-decode как A2). recovery-инвариант — главное место ошибки.
- **Done:** recovery восстанавливает watermark; crash-recovery тест.
- **Отчёт:** recovery-логика + тест краша. <400 слов.

---

## P2a — Version-assign → atomic fetch_add до критической секции

- **Agent:** structural
- **Prereq:** P1d (вся P1 зелёная)
- **Контекст:** `version-oracle-design.md` §P2. Перенести
  assign_next_version из-под commit_lock на чистый `oracle.fetch_add(1)`
  ДО критической секции. Теперь появляются новые abort-пути между assign
  и publish — покрыть отложенные из P1b (по замечанию R2).
- **Файлы:** `repo_tx_gate.rs` (assign_next_version),
  `commit.rs`/`group_commit.rs` (порядок: assign раньше lock).
- **Задача:** assign до lock; провести версию через критическую секцию;
  покрыть mark(Aborted) на путях, ставших достижимыми после раннего
  assign (отложенные P1b). Тест: tx, упавший на SSI ПОСЛЕ раннего
  assign, помечает версию; watermark перешагивает; дыра не вечна.
- **Ограничения:** монотонность (инвариант #1). hole-tolerance —
  каждый новый abort-путь помечает. СТОП на неопределённости.
- **Done:** assign lock-free; все abort-пути (включая новые) помечают;
  полный набор зелёный.
- **Отчёт:** новая карта путей + покрытие + тесты. <400 слов.

---

## P2b — Materialize вне commit_lock (per-table uwl_guards)

- **Agent:** structural · **РИСК: высокий (ядро super-win)**
- **Prereq:** P2a
- **Контекст:** `version-oracle-design.md` §P2. Это снимает блок Stage B.
  Теперь безопасно: P1c сделал publish contiguous-prefix, так что
  out-of-order materialize НЕ показывает дыр читателям. materialize
  гейтится per-table uwl_guards (disjoint tables параллельны); после —
  `completion.mark(version, Materialized)`.
- **Файлы:** `commit.rs`/`group_commit.rs` (вынести materialize из-под
  commit_lock), `materialize.rs`, `repo_tx_gate.rs` (uwl_guards).
- **Задача:** materialize выполняется ВНЕ commit_lock под uwl_guards
  затронутых таблиц. Версия известна (P2a). После materialize → mark +
  watermark advance (P1c). Stage B-инвариант держится через
  contiguous-prefix. Тест: два disjoint-table commit'а материализуются
  параллельно; reader видит согласованный префикс; overlapping-table
  сериализуются на uwl_guard.
- **Ограничения:** visibility ordering (инвариант), durability перед
  visibility. ABBA-free: uwl_guards в sorted order. СТОП на любом
  недетерминизме SSI/recovery.
- **Done:** materialize concurrent; полный SSI/phantom/recovery/
  concurrent зелёный; bench wire_pipelining sync/n_32,n_128 — РОСТ с
  concurrency (super-win метрика).
- **Отчёт:** механика выноса + bench Δ + тесты. <500 слов.

---

## P2c — commit_lock shrink/dissolve для disjoint-table txs

- **Agent:** structural · **РИСК: высокий**
- **Prereq:** P2b
- **Контекст:** `version-oracle-design.md` §P2. Финал: commit_lock
  сжимается до короткой SSI cross-tx секции — или исчезает для
  disjoint-table txs (их единственная сериализация — per-table
  uwl_guard + atomic oracle).
- **Файлы:** `commit.rs`/`group_commit.rs`, `repo_tx_gate.rs`.
- **Задача:** для disjoint-table commit'ов убрать commit_lock с пути.
  Оставить короткую секцию только там, где cross-tx SSI требует атомарной
  проверки (или перевести на optimistic + completion-based abort).
  Тест: N disjoint-table txs коммитятся БЕЗ взаимной блокировки на
  commit_lock (проверить через contention-метрику/таймстемпы).
- **Ограничения:** serializability. Group commit (Db) совместимость.
  СТОП если cross-tx SSI нельзя сделать без короткого lock — тогда
  оставить минимальный, задокументировать.
- **Done:** disjoint commits lock-free; bench sync/n_128 — большой Δ.
- **Отчёт:** что осталось под lock и почему + bench. <500 слов.

---

## P3a — Inter-batch footprint accumulator (#5)

- **Agent:** structural
- **Prereq:** P2c (или P1c — tracker достаточно)
- **Контекст:** `stop-themes.md` #5. С completion tracker'ом batch-local
  footprint accumulator естественен: accepted-so-far версии видны через
  tracker, footprints доступны для cross-follower SSI.
- **Файлы:** `group_commit.rs` (run_batch), `pre_commit.rs`
  (pre_commit_locked_validate).
- **Задача:** аккумулировать write-footprints accepted-so-far в batch,
  передавать в validate каждого follower'а вместе с committed footprints.
  Закрывает узкий phantom-кейс из Db. Тест: два Serializable follower'а
  с predicate-зависимостью в одном batch — конфликт детектируется.
- **Ограничения:** не регрессировать пропускную способность batch'а.
- **Done:** inter-batch phantom детектируется; тест.
- **Отчёт:** механика accumulator + тест. <300 слов.

---

## P3b — Stress + crash-injection verify

- **Agent:** structural
- **Prereq:** P3a
- **Контекст:** финальная валидация Оракула под нагрузкой и крашами.
- **Файлы:** новый stress-тест в `crates/shamir-engine` или
  `crates/shamir-db/tests/`.
- **Задача:** harness: N concurrent committers, disjoint + overlapping
  таблицы, краш-инъекция в каждой фазе (после assign / после WAL / в
  середине materialize / после mark до watermark). Проверять
  serializability + recovery-сходимость + watermark-корректность после
  каждого краша.
- **Ограничения:** детерминируемый (seed через args, не Math.random).
- **Done:** stress зелёный под всеми точками краша; serializability
  держится.
- **Отчёт:** покрытые точки краша + результаты. <400 слов.

---

## Сводка делегирования

| Этап | Тип | Риск | Prereq | Параллельно |
|---|---|---|---|---|
| P0 | research | низкий | — | P0/R1/R2 |
| R1 | research | низкий | — | P0/R1/R2 |
| R2 | research | низкий | — | P0/R1/R2 |
| P1a | structural | низкий | P0,R1,R2 | — |
| P1b | structural | средний | P1a,R2 | — |
| P1c | structural | высокий | P1b | — |
| P1d | structural | высокий | P1c | — |
| P2a | structural | средний | P1d | — |
| P2b | structural | высокий | P2a | — |
| P2c | structural | высокий | P2b | — |
| P3a | structural | средний | P2c | — |
| P3b | structural | средний | P3a | — |

**Поток:** {P0‖R1‖R2} → если go → P1a→b→c→d → P2a→b→c → P3a→b.
3 research-агента параллельно, затем 9 структурных последовательно.
Каждый структурный — отдельный коммит после моей проверки diff'а.
