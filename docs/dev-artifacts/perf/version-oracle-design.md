# Design: Оракул Версий — lock-free commit

Замко́вая Ключа I (`roadmap-keystones.md`). Раскрывает стопы #4
(materialize вне lock), #5 (inter-batch phantom), #14 (MVCC by version).
Super-win: commit перестаёт быть точкой сериализации — конкурентные
коммиттеры масштабируются с ядрами.

---

## Три часа, сплетённые в одну стрелку

Commit-конвейер сплетает три по природе раздельные вещи под
`commit_mutex`:

| Часы | Что | Сейчас (после Stage B/Db) |
|---|---|---|
| **Порядок** | `assign_next_version()` — кто за кем | под commit_lock |
| **Долговечность** | WAL append (`begin`/`begin_many`) | под commit_lock |
| **Видимость** | `publish_committed` + version-log append (materialize) | под commit_lock |

Замок держится, потому что **видимость не должна обогнать
долговечность**, а **порядок версий обязан быть монотонным**. Stage B
подтвердил: materialize нельзя вынести — version N+1 опубликуется раньше
чем N запишет в version-log, и читатель at N+1 не увидит данные N.

---

## Что УЖЕ есть (фундамент)

Чтение **уже примиряет по версии** — это ключевое открытие, делающее
Оракул bounded, а не research-project:

- `MvccStore::get_at(key, snapshot)` — находит новейшую версию ≤ snapshot
  через range-scan версионного лога (`<key>::0xFF::<version_be>`).
  Читатель НЕ зависит от глобального publish-замка — он сканирует лог.
- `assign_next_version()` — уже атомарный счётчик, просто вызывается под
  lock'ом.
- `publish_committed_max(version)` — уже документирован как «safe без
  commit_lock, только двигает счётчик через max».
- Stage Db — group commit leader/follower, `begin_many`, `conflicts_with`.

Значит #14 («MVCC B+ tree by version») в своей read-части **уже решён** —
версионный лог и есть version-indexed структура. Остаётся только
**publish-семантика**.

---

## Дизайн Оракула

Расплести три часа:

```
ПОРЯДОК (lock-free):
  version = oracle.fetch_add(1)         // AtomicU64, ноль сериализации

ДОЛГОВЕЧНОСТЬ (concurrent / batched):
  wal.begin_many(entries)               // group commit (Db), один fsync

ВИДИМОСТЬ (concurrent materialize + contiguous-prefix publish):
  materialize(version)                  // version-log append, per-table
                                        // uwl_guards — disjoint tables параллельны
  completion.mark(version)              // отметить версию завершённой
  publish_watermark.advance()           // двигать ТОЛЬКО по непрерывному
                                        // префиксу завершённых версий
```

**Ключевой инвариант — contiguous-prefix publish.**
`published_version` (read-floor для новых snapshot'ов) = наибольшая `V`
такая, что **все** версии ≤ V материализованы. Это снимает блок Stage B:
materialize идёт concurrent/out-of-order, но watermark не перешагнёт
дыру. Читатель at snapshot = watermark видит непрерывный, согласованный
префикс — никаких грязных чтений.

**Completion tracker.** Маленькая структура version → {Pending,
Materialized, Aborted}. Watermark двигается по contiguous-префиксу
{Materialized ∪ Aborted}. Кандидаты:
- плотный кольцевой буфер от watermark (версии монотонны, дыры
  кратковременны) — O(1) advance,
- или `scc::TreeIndex<u64, State>` если дыры могут жить долго.

---

## Инварианты (нерушимые)

1. **Монотонность.** `oracle.fetch_add` гарантирует уникальные растущие
   версии. Дыры (assigned-but-never-committed) допустимы.
2. **Version-hole tolerance.** Tx, получивший версию V и упавший (SSI
   fail / abort), помечает V как `Aborted` в completion tracker — иначе
   watermark застрянет на V−1 навсегда. **Критично:** путь отмены ОБЯЗАН
   пометить версию.
3. **Contiguous publish.** Watermark = max V где ∀ k≤V: state(k) ∈
   {Materialized, Aborted}. Никогда не перешагивает Pending.
4. **Durability перед visibility.** WAL-append версии V завершён ДО того
   как V может попасть в Materialized. (Краш до WAL → версия теряется,
   но она и не была durable; краш после WAL до materialize → recovery
   доигрывает, см. ниже.)
5. **Recovery.** WAL-запись несёт свою версию (WAL v3, commit `0e772ab`
   уже несёт interner-delta — версия добавляется туда же). Recovery
   реигрывает версии в порядке, восстанавливает completion-префикс,
   ставит watermark на наибольший contiguous materialized.

---

## Поэтапный план

```
P0 — MEASURE (дёшево, DIAG; решает идти ли дальше)
  Профилировать commit_mutex hold под concurrent load (wire_pipelining
  sync/n_32, n_128). Разложить hold на SSI / version / WAL / materialize
  / publish. ПОДТВЕРДИТЬ: materialize доминирует в lock-held времени.
  Если нет — Оракул не окупится, СТОП с цифрами.
  Инструмент: AtomicU64 + Instant вокруг фаз (как Stage 2c/7/8/14).
  Gate: нет (research-only, revert DIAG).

ПРЕД-РЕФАКТОР (две ревизии до единой строки P1; из созерцания) —
  не строительство, а проверка несущих допущений:
  R1 — read-path coherence audit: НИ ОДИН read данных записи не в обход
       get_at(snapshot). Главный подозреваемый — CachedStore (#4). Без
       этого инвариант contiguous-publish — ложь для обходных читателей.
  R2 — abort-path census: КАЖДЫЙ early-return после assign_version
       помечает версию Aborted (иначе watermark зависает навечно).
       Перенос assign вперёд (P2a) множит такие пути.
  Цель пред-рефактора: сделать коррупцию невозможной ПО ПОСТРОЕНИЮ, а не
  маловероятной по тестам. Дёшевы (чтение кода + grep), обязательны.
  Детальные брифы — version-oracle-execution-plan.md (R1, R2).

P1 — PRE-REFACTOR: completion tracker + contiguous-prefix publish
  - Добавить CompletionTracker (version → state) в RepoTxGate.
  - publish: вместо publish_committed(version) под lock — 
    completion.mark(version, Materialized) + watermark.advance().
  - abort path: completion.mark(version, Aborted) везде где tx падает
    после assign_version (SSI fail, validator fail, materialize err).
  - Watermark становится contiguous-max, lock-free.
  - Читатели НЕ меняются (get_at by snapshot; snapshot = watermark на
    begin_tx). Это и есть красота — read-path уже version-indexed.
  - Recovery: восстановить completion-префикс из WAL версий.
  Gate: SSI/phantom/recovery/concurrent ВСЕ зелёные. Любой
    недетерминизм → СТОП.
  Риск: ВЫСОКИЙ (версионная видимость). Делать как Stage A — мелкими
    independently-testable под-шагами (P1a tracker scaffolding,
    P1b abort-path marking, P1c watermark advance, P1d recovery).

P2 — ОРАКУЛ: decouple version-assign + concurrent materialize
  - assign_next_version → перенести из-под commit_lock на чистый
    oracle.fetch_add ДО критической секции (или в самом её начале,
    затем lock отпускается раньше).
  - materialize вынести из commit_lock, гейтить per-table uwl_guards
    (disjoint tables параллельны). После materialize → completion.mark.
  - commit_lock сжимается до... в идеале ИСЧЕЗАЕТ для disjoint-table
    txs. Остаётся короткая секция только для SSI cross-tx validation
    (или и она переходит на optimistic + completion-based abort).
  Gate: полный + bench wire_pipelining sync/n_32/n_128 — ОЖИДАНИЕ
    значимого роста с concurrency (это super-win).
  Риск: ВЫСОКИЙ.

P3 — VERIFY + #5 (inter-batch phantom) дозакрыть
  - С completion tracker'ом batch-local footprint accumulator (#5)
    становится естественным: accepted-so-far версии видны через tracker.
  - Stress-тест: N concurrent committers, disjoint + overlapping tables,
    проверить serializability + recovery под краш в каждой фазе.
  Gate: полный + stress.
```

**Декомпозиция P1/P2 как Stage A** — это урок кампании: WAL v3 разбит на
A1→A5, каждый independently компилируемый и тестируемый. Версионную
видимость резать так же — scaffolding → marking → advance → recovery,
каждый под gate.

---

## Зависимости и носители

```
P0 (measure) ──→ решает идти ли
                  │
P1 (completion tracker) ──→ снимает блок #4, готовит #5
                  │
P2 (oracle + concurrent materialize) ──→ #4 закрыт, super-win
                  │
P3 (verify + footprint accumulator) ──→ #5 закрыт
```

Stage Db (group commit, готов) — фундамент: leader/follower и `begin_many`
дают batched durability, на которую ложится concurrent materialize.

---

## Что НЕ входит (явные границы)

- **Не** переписываем версионный лог в B+ tree — он уже version-indexed
  (range-scan по `<key>::0xFF::<version>`). #14 в read-части закрыт
  фундаментом.
- **Не** трогаем wire format (это #7, отдельная release-волна).
- **Не** меняем read-path API — `get_at(snapshot)` остаётся как есть.
  Меняется только КАК вычисляется published watermark.

---

## Риск-резюме

| Фаза | Риск | Почему |
|---|---|---|
| P0 | низкий | research-only, revert |
| P1 | высокий | версионная видимость, recovery invariants |
| P2 | высокий | конкурентный materialize, SSI ordering |
| P3 | средний | verify + узкий #5 |

Data integrity > performance. На любой неопределённости в P1/P2 — СТОП,
как делали со всеми структурными стопами. Оракул стоит того, но не ценой
тихой коррупции версий.
