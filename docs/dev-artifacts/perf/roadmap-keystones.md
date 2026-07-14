# Roadmap: три замко́вых — синтез 17 стопов

Кристалл созерцания над `stop-themes.md`. 17 отложенных оптимизаций — не
плоский backlog, а **три хребта**. Этот файл — карта-индекс: природа
каждого ключа, граф «стоп → ключ → несущая волна → фаза», и порядок
исполнения. Глубокий дизайн единственной строимой замко́вой —
в `version-oracle-design.md`.

---

## Три ключа

| Ключ | Что меняет | Жанр | Раскрывает стопы |
|---|---|---|---|
| **I. Оракул Версий** | показатель степени (×ядра на записи) | design (строится) | #4, #5, #14 |
| **II. Несущая Волна** | marginal cost → 0 (бандлинг) | дисциплина | #3, #6, #8, #12-async |
| **III. Ленивый Дуал** | константу (двойная работа) | паттерн | #2, #9, #10 |
| — | measure-first хвост | дисциплина | #11, #13, #15, #16 |
| — | release-prep | планово | #7 (несёт #6) |
| — | уже выиграно | — | #1 (auto-inline), #17 (optimum) |

---

## Граф зависимостей: стоп → ключ → носитель → фаза

```
Ключ I — Оракул Версий (замко́вая)
  #4  materialize вне commit_lock ─┐
  #5  inter-batch phantom ─────────┼──→ требует version-indexed visibility
  #14 MVCC B+ tree by version ─────┘     (P1 pre-refactor) → Оракул (P2)
  фундамент уже есть: Stage Db (group commit), begin_many, conflicts_with

Ключ II — Несущая Волна (прицепить к структурной волне)
  #7  wire format bump v1  ←── НОСИТЕЛЬ
        ├─ #6  writev fan-out      (едет на протокольном слое даром)
        └─ (positional msgpack, typed sub_id — сами по себе часть #7)
  FilterValue эволюция  ←── НОСИТЕЛЬ
        └─ #8  funclib enum dispatch (едет на FilterValue::FnCall)
  validator API restructure  ←── НОСИТЕЛЬ
        └─ #3  merge_inner_maps in-place (едет на Diff-сигнатуре)
  Rust async-fn-in-trait stable  ←── НОСИТЕЛЬ (внешний)
        └─ #12 Store async_trait → GAT (едет на стабилизации языка)

Ключ III — Ленивый Дуал (механически приложить паттерн)
  #2  Interner dense+sparse split   = Vec + scc::HashMap
  #9  InnerValue Hash cache          = Value + OnceLock<u64>
  #10 legacy_to_inner zero-copy        = зеркало msgpack-#8 (done)
  образец: #12 StagedRow (DONE) = Live(InnerValue) + OnceLock<Bytes>

Measure-first (ждут сигнала, не преграды)
  #11 base58 lookup table     → low ROI, отложить
  #13 CachedStore lazy load   → измерить boot time
  #15 WAL recovery prefix scan→ verify prefix существует → механический фикс
  #16 WASM module cache       → когда WASM станет hot path
```

---

## Ключ II — Несущая Волна (детально)

**Принцип.** Стопы #3/#6/#8/#12-async остановлены по **ширине**, не по
сложности. Каждый — мелкое изменение, блокированное формой одного типа,
на который много ссылок. Не делать их отдельными поездками — **прицепить
пассажирами к запланированной структурной волне**, где тот тип всё равно
меняется.

| Стоп | Блокирующий тип | Несущая волна | Marginal cost на волне |
|---|---|---|---|
| #6 writev fan-out | `PushSink::try_push_event` | #7 wire bump (трогает транспорт) | ~0 |
| #8 funclib enum | `FilterValue::FnCall` | любая эволюция FilterValue | ~0 |
| #3 merge in-place | validator signature | validator API → Diff | малый |
| #12 async GAT | `Store` trait (9 backends) | Rust async-fn-in-trait stable | внешний триггер |

**Правило исполнения.** Когда планируется структурное изменение типа из
колонки 2 — открыть соответствующий стоп **в том же цикле**. Вести их в
этом файле как «прицепы», проверять при каждой структурной волне.

---

## Ключ III — Ленивый Дуал (детально)

**Паттерн (доказан кампанией):** `Lazy<Cheap, Expensive>` — держи дешёвую
форму, материализуй дорогую лениво, кэшируй. Победный ход Stage 18 (lazy
msgpack), #8 (zero-copy decode), #12 (StagedRow encoded cache),
decode/deliver-кэшей подписок.

**Оставшиеся места приложения:**

| Стоп | Форма | Файл-цель | Scope |
|---|---|---|---|
| #9 InnerValue Hash cache | `Value + OnceLock<u64>` | `shamir-types/types/value.rs` | medium (+ Hash impl) |
| #2 Interner dense+sparse | dense `Vec` (touch_ind) + sparse `scc::HashMap` (touch_with_id) | `shamir-types/core/interner/` | medium 150-300 LOC |
| #10 legacy_to_inner | зеркало msgpack zerocopy (#8 done) | `shamir-types/codecs/interned/legacy.rs` | medium |

**Правило исполнения.** Каждый — самостоятельный /opti-цикл. Перед #9 —
измерить hot-path вклад Map/Set хеширования (вероятно subscription
filters). #2 требует careful coherence-анализа между dense и sparse под
concurrency. #10 — когда legacy-путь станет горячим (сейчас msgpack — wire,
legacy text — REST/admin).

---

## Порядок исполнения (фазы верхнего уровня)

```
Фаза 0  — MEASURE: профиль commit_mutex hold под concurrent load.
          Подтвердить, что materialize доминирует в lock-hold.
          (DIAG, дёшево; как Stage 2c/7/8/14.) → решает идти ли в Ключ I.

Фаза 1  — PRE-REFACTOR (если Ф0 подтверждает): version-indexed read
          visibility + version-hole tolerance в recovery.
          Структурный prerequisite Оракула. Детали — version-oracle-design.md.

Фаза 2  — ОРАКУЛ ВЕРСИЙ: decouple version-assign (atomic), concurrent
          materialize, publish-as-durability. Super-win: lock-free commit.

Фаза 3  — Ленивый Дуал: #9 (после measure), #2 (coherence), #10 (когда hot).
          Параллельно/независимо от Ключа I.

Фаза 4  — Несущая Волна: #3/#6/#8 прицепить к их волнам по мере появления.
          #7 wire bump — на release-stage, несёт #6.

Хвост   — measure-first (#11/#13/#15/#16) по сигналу.
```

**Ключи I и III независимы** — можно вести параллельно (разные крейты:
commit pipeline vs types/interner). Ключ II — реактивный, не
самостоятельная фаза.

---

## Замко́вая мысль

Вся кампания до сих пор сжимала **константу** — меньше работы на путь
(итог ~2×). Оракул Версий меняет **показатель**: конкурентные коммиттеры
масштабируются с ядрами вместо очереди. Это переход от «быстрее» к «без
потолка записи». Из 17 камней — один краеугольный (Ключ I), и на нём
держится свод. Остальные два ключа — дисциплина (II) и шаблон (III),
механика, а не открытие.
