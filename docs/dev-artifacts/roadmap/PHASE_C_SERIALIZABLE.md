# Phase C — Predicate / Range Locks → True Serializability (Phantom Protection)

Status: **IMPLEMENTED (2026-05-31).** Predicate/range SIREAD locks ship:
`PredicateDep`/`PredicateSet` on `TxContext`, the `Filter → IndexRange`
bridge (`predicate_range.rs`, precise via sort-codec / coarse `TableScan`),
the commit-time write-key log on `RepoTxGate` (built from `index_write_set`,
pruned at `min_alive`), and Phase 2-bis in `pre_commit` (`PhantomConflict`
+ `txs_aborted_phantom` metric) — all gated on `Serializable` so Snapshot /
non-tx stay byte-for-byte unchanged. 22 `ssi_phantom_tests` pass: anomalies
(indexed-range / Between / update-into-range / coarse-scan) abort the second
commit; precision cases (disjoint ranges, other table, update-outside) do
NOT falsely abort; Snapshot records nothing. Builds on Phase A SSI (done —
see [`TRANSACTIONS.md`](./TRANSACTIONS.md) /
[`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md) and
[`../pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md)). The
design below matches what shipped.
Complementary to — and intentionally orthogonal from — Phase B
(interactive multi-call transactions). Phase C does not add any new
isolation level: it *closes the last serializability hole* in the
existing `IsolationLevel::Serializable` path (the predicate/phantom hole),
leaving Snapshot a strict zero-overhead subset.

This is the companion to Phase A's two design docs. Where
`TRANSACTIONS.md` gave the *what* (engine-managed MVCC over dumb KV) and
`TRANSACTIONS_IMPL.md` gave the *how* (concrete code recon, the
zero-overhead rule, the test matrix), this file does the same job for one
specific gap: **point-read SSI catches write-skew on keys actually read;
it does NOT catch phantoms.** We plan the predicate/range-lock layer that
does.

> **House rule reminder (from CLAUDE.md).** Predicate tracking lives only
> on the `Serializable` path. The Snapshot path must stay byte-for-byte
> the current code — *zero* extra work per read/write. Every new type
> below is labelled **PROPOSED**; nothing here exists yet, and nothing in
> this doc touches `Cargo.toml`. Crate suggestions are PROPOSALS that
> require maintainer sanction before any dependency moves.

---

## 1. Что уже ловит SSI, и где он слеп — по-русски

### Где мы сейчас

Phase A построил SSI ровно так, как описано в `TRANSACTIONS.md` §«SSI»:
на каждом чтении внутри Serializable-транзакции мы пишем в read-set пару
`(table_id, key) → version_seen`, а на commit'е проверяем, что версия
каждого прочитанного ключа не убежала вперёд. Это **point read-set**:
множество конкретных ключей, которые транзакция *реально прочитала*.

Машинерия живёт здесь, и она настоящая, end-to-end:

- `TxContext.read_set: scc::HashMap<(u64, Bytes), u64>` —
  `crates/shamir-tx/src/tx_context.rs:110`. `scc::HashMap`, а не
  `std::HashMap`, чтобы `record_read_shared(&self, …)`
  (`tx_context.rs:226`) могла писать через shared-ссылку — это
  load-bearing для HIGH-C.
- Точечное чтение пишет read-set: `TableManager::read_one_tx`
  (`crates/shamir-engine/src/table/table_manager.rs:1455`, запись на
  `:1468`), версия берётся из `MvccStore::version_of`
  (`crates/shamir-tx/src/mvcc_store.rs:239`).
- Scan'ы пишут read-set для *выданных* записей: `record_scan_reads`
  (`table_manager.rs:1345`, запись на `:1364`), через `read_tx` /
  `record_query_reads` (`table_manager.rs:1554` / `:1579`).
- Wire-путь: executor гонит `BatchOp::Read` через `read_tx` с shared
  `&TxContext` (`crates/shamir-engine/src/query/batch/executor.rs:412`).
- Валидация на commit'е: Phase 2 в `pre_commit`
  (`crates/shamir-engine/src/tx/commit.rs:603`) зовёт
  `validate_read_set` (`tx_context.rs:250`), провайдер версий —
  `RepoVersionProvider` (`crates/shamir-engine/src/repo/version_provider.rs:10`),
  подключается в `begin_tx` только для Serializable
  (`repo_instance.rs:379`).

Stub-провайдер `|_, _| Some(0)` (`commit.rs:609`) — это by-design
fallback: `0 <= любая version_seen`, значит при отсутствующем провайдере
валидация тривиально проходит, и Serializable деградирует до Snapshot
вместо того, чтобы абортить всё подряд.

### Где слепота

Point read-set по построению знает только про ключи, которые
**существовали и были прочитаны**. Он ничего не знает про ключи, которые
*появятся* и удовлетворят предикату. Это и есть **phantom**: конкурентная
транзакция вставляет новую строку, попадающую в диапазон/предикат, по
которому мы сканировали, и наш read-set её не содержит — версии всех
прочитанных нами ключей не сдвинулись, валидация проходит, обе
транзакции коммитятся в несериализуемое расписание.

Это не теория и не «edge case» — это прямым текстом задокументированная
граница в коде:

> «Streaming-scan SSI scope: this records the keys the scan *yields*. It
> does NOT install predicate / range locks, so phantom inserts into the
> scanned range by a concurrent tx are not detected.»
> — `table_manager.rs:1271`

> «It records by point key only — it does NOT install predicate / range
> locks, so a concurrent tx inserting a NEW row into the scanned
> predicate (a phantom) is not detected.»
> — `table_manager.rs:1549`

Phase A сам это признаёт честно (Q5 в
`../pre-transactional/REVIEW.md:446` оставляет Serializable opt-in именно
потому, что он ещё не полный). Phase C закрывает этот зазор.

### Worked example — write-skew vs phantom

Сначала то, что SSI **уже ловит** (write-skew на реально прочитанных
ключах), чтобы контраст был чёткий:

```
table doctors { id, on_call: bool }   // два врача, оба on_call=true
                                       // rid=D1, rid=D2

Tx1 (serializable)                Tx2 (serializable)
─ begin snap=V                    ─ begin snap=V
─ read D1, read D2  ───┐          ─ read D1, read D2  ───┐
  (read_set={D1@v,D2@v})           (read_set={D1@v,D2@v})
─ count on_call == 2             ─ count on_call == 2
─ set D1.on_call=false           ─ set D2.on_call=false
─ commit  → publishes D1@v+1      ─ commit
                                    Phase 2: version_of(D2) == v? НЕТ —
                                    Tx1 не трогал D2... но Tx2 ПРОЧИТАЛ
                                    D1, а D1 теперь v+1 > v → SsiConflict.
                                    Tx2 АБОРТИТСЯ. Инвариант (≥1 on_call)
                                    спасён.
```

Этот случай защищён, потому что обе транзакции **прочитали обе строки**,
и записи попали в read-set. Теперь phantom — тот же по духу инвариант, но
выраженный через **предикат**, а не через перечисление ключей:

```
table users { id, age: int }          // изначально: НИ ОДНОГО age>30

Tx1 (serializable)                     Tx2 (serializable)
─ begin snap=V                         ─ begin snap=V
─ SELECT * WHERE age > 30  ───┐        ─ SELECT * WHERE age > 30  ───┐
  → {}  (пусто!)                          → {}  (пусто!)
  read_set = {}  ← НЕЧЕГО                 read_set = {}  ← НЕЧЕГО
  записывать: ни один ключ                записывать
  не прочитан, строк нет
─ business rule: «можно вставить       ─ тот же rule: «можно вставить
  нового, только если сейчас               нового, только если сейчас
  никого с age>30 нет» → OK                никого с age>30 нет» → OK
─ INSERT users{age=35}                 ─ INSERT users{age=40}
─ commit  → Phase 2:                   ─ commit  → Phase 2:
  read_set пуст → проходит               read_set пуст → проходит
  COMMIT ✓                               COMMIT ✓

Итог: ДВЕ строки age>30. Ни одно последовательное расписание
(Tx1;Tx2 или Tx2;Tx1) этого допустить не могло: вторая транзакция
обязана была увидеть строку, вставленную первой, и нарушить своё
правило. Расписание НЕ сериализуемо. SSI это пропустил.
```

ASCII-таймлайн того же:

```
        t →
Tx1:  begin    SELECT age>30 → {}            INSERT age=35     COMMIT
Tx2:     begin     SELECT age>30 → {}            INSERT age=40      COMMIT
                   ▲                              ▲
                   обе видят пустой предикат      обе вставляют в него
                   (snapshot V)                   фантом друг для друга
```

Корень проблемы: **read_set фиксирует прочитанные ключи; предикат — это
утверждение о ключах, которых ещё нет.** Чтобы поймать фантом, на commit'е
надо проверять не «сдвинулась ли версия прочитанного ключа», а
«не вставил ли кто-то ключ, попадающий в мой предикат, после моего
snapshot'а». Для этого предикат надо *запомнить* в момент чтения и
*пересечь* с write-key'ами конкурентных коммиттеров.

---

## 2. Theory grounding — SSI, SIREAD, predicate vs index-range vs next-key

### Тот же первоисточник, что и Phase A

Phase A уже опирается на модель **Serializable Snapshot Isolation**
Cahill/Fekete/Röhm (SIGMOD 2008) и её реализацию в PostgreSQL 9.1
(Ports & Grittner, VLDB 2012). Phase A реализовал *часть* этой модели:
обнаружение **rw-antidependency** через сравнение версий read-set'а с
текущими committed-версиями под commit-gate. Это закрывает write-skew на
точечных чтениях.

Чего Phase A не реализовал — это **SIREAD-локи над предикатами**. В
терминологии Cahill: SIREAD — это не блокирующий «лок», а запись «эта
транзакция прочитала эту область данных»; он живёт, чтобы конкурентная
запись в ту же область могла обнаружить rw-ребро `T_writer → T_reader`.
Над точечным ключом наш `read_set`-элемент уже *является* SIREAD-локом.
Над предикатом нам нужен SIREAD-лок, **покрывающий область, которой ещё
нет физического ключа** — а это ровно problem of phantoms.

### Три практических подхода (и почему мы выбираем именно один)

**(A) True predicate locks.** Хранить сам предикат (`age > 30`) и на
каждой конкурентной записи *вычислять* `predicate.matches(new_row)`.
Максимально точно (никаких ложных конфликтов), но: (1) дорого — каждая
запись прогоняет каждый живой предикат; (2) предикат произволен (regex,
`Computed`, `Fts`), и `matches` над чужим staged-значением — это уже
полноценная evaluation. PostgreSQL отказался от настоящих predicate-локов
ровно поэтому.

**(B) Index-range locks (наш выбор по умолчанию).** Если предикат
обслуживается **отсортированным индексом**, он эквивалентен интервалу в
пространстве ключей индекса: `age > 30` ⇒ `[encode(30)+ε, +∞)` в
key-space sorted-индекса по `age`. Тогда «фантом, попадающий в предикат»
≡ «конкурентный коммиттер записал posting-ключ индекса внутри этого
интервала». Проверка — пересечение интервалов, без evaluation предиката.
Это то, что делает большинство production-движков (это, по сути,
key-range locking над B-деревом индекса).

**(C) Next-key locking** (DB2/InnoDB-стиль) — это *физический* приём:
лочится «следующий ключ» за прочитанным, чтобы вставка между ними
блокировалась. Он завязан на блокирующие row-локи и на конкретный
обход B-дерева под локом. У нас архитектура **MVCC-над-dumb-KV**: backend
«тупой», блокирующих row-локов нет (см. `TRANSACTIONS.md` §«Что остаётся
от backend»). Next-key locking сюда не ложится без затаскивания
lock-manager'а в каждый backend — это противоречит самому смыслу
`Store`/`Repo`-трейтов.

### Решение для ShamirDB

> **Index-range locks как основной механизм; coarse predicate (table- или
> prefix-granularity) как безопасный fallback для предикатов без
> sorted-индекса.**

Обоснование под нашу архитектуру:

1. **Ложится на MVCC-над-KV.** Range-lock — это чисто in-memory структура
   конфликт-детекции на стороне engine; backend остаётся тупым KV.
   Никаких блокирующих локов в storage-слое — ровно как commit-gate и
   read-set уже устроены лок-фри/CAS (`RepoTxGate`,
   `repo_tx_gate.rs:21`; read-set — `scc::HashMap`).
2. **Уже есть носитель интервала.** `SortedIndexManager::lookup_range`
   (`crates/shamir-engine/src/index/sorted_index_manager.rs:339`) уже
   строит физический `[lower, upper]` через `range_bounds` (`:516`); а
   `index2`-слой уже имеет `IndexQuery::Range { lo: Bound<Vec<u8>>, hi:
   Bound<Vec<u8>> }` (`crates/shamir-engine/src/index2/backend.rs:23`).
   Интервал, который надо «залочить», — это ровно тот, что мы и так
   сканируем.
3. **Точность там, где она дёшева; безопасность везде.** Indexed range →
   узкий интервал → мало ложных абортов. Full-table filter (нет индекса,
   или предикат — `Regex`/`Computed`/`Fts`) → грубый table-granularity
   лок → возможны ложные аборты, но **никогда** пропущенный фантом.
   Over-locking SSI-safe; under-locking — нет.

---

## 3. Design — расширение read-set от точек к предикатам/диапазонам

### 3.1 Новая структура на TxContext (PROPOSED)

Сегодня `TxContext` несёт точечный read-set. Phase C добавляет
параллельный **predicate read-set** — список «предикатных зависимостей»,
которые тот же commit-gate валидирует рядом с точечным.

```rust
// crates/shamir-tx/src/predicate_set.rs   (PROPOSED — new file)

/// PROPOSED. One captured predicate dependency of a Serializable tx.
/// Recorded at read time; validated at commit against concurrent
/// committed write-keys (see §6). Pure data — shamir-tx stays ignorant
/// of how index keys are composed (same contract as `UniqueGuard`,
/// tx_context.rs:29).
#[derive(Debug, Clone)]
pub enum PredicateDep {
    /// PRECISE. The read was served by a sorted index over `index_name`
    /// (interned). The scan covered the physical key interval
    /// `[lo, hi]` in that index's posting key-space. Built engine-side
    /// from `SortedIndexManager::range_bounds` (sorted_index_manager.rs:516)
    /// or from `IndexQuery::Range` (index2/backend.rs:23) and handed in
    /// as raw bytes.
    IndexRange {
        table_token: u64,
        /// Interned sorted-index name (matches `SortedIndexDefinition
        /// .name_interned`, sorted_index_manager.rs:66) OR the index2
        /// descriptor id. Distinguishes which index's key-space `lo/hi`
        /// live in.
        index_id: u64,
        lo: std::ops::Bound<bytes::Bytes>,
        hi: std::ops::Bound<bytes::Bytes>,
    },

    /// COARSE. The read was a full-table scan / a predicate no sorted
    /// index serves (Regex, Computed, Fts brute-force, or just no index
    /// on the field). We cannot bound it to an interval, so ANY insert
    /// or update into this table by a concurrent committer is a conflict.
    /// Over-aborts; never misses a phantom.
    TableScan { table_token: u64 },
}

/// PROPOSED. Per-tx predicate read-set. Lives next to `read_set`.
/// `Mutex<Vec<…>>` (not `scc::HashMap`) because (a) it is append-only
/// during execution and scan-only at commit, (b) entries are not keyed,
/// (c) the executor runs a tx's queries serially (executor.rs:203) so
/// contention is nil — the lock is taken uncontended. A plain
/// `parking_lot`/`std` Mutex is acceptable ONLY here because it is never
/// held across `.await` and never on a hot non-tx path; alternatively a
/// lock-free `scc::Queue` (PROPOSAL, needs sanction) avoids the lock
/// entirely. Default to the simplest that compiles; revisit under bench.
pub type PredicateSet = /* PROPOSED */ Vec<PredicateDep>;
```

Подключение к `TxContext` (PROPOSED, мирроринг существующих полей в
`tx_context.rs:51`):

```rust
pub struct TxContext {
    // ... existing fields (tx_context.rs:51-135) ...

    /// PROPOSED (Phase C). Predicate / range read dependencies.
    /// Populated ONLY when `isolation == Serializable` — exactly like
    /// `read_set` (record_read_shared no-ops off Serializable,
    /// tx_context.rs:227). Validated at commit alongside `read_set`.
    /// Interior-mutable so the engine's scan path can append through a
    /// shared `&TxContext` (same reason `read_set` is scc::HashMap —
    /// `read_one_tx` holds the tx by `Option<&TxContext>`).
    pub predicate_set: PredicateSet, // behind interior mutability
}
```

Recording API (PROPOSED, mirrors `record_read_shared`):

```rust
impl TxContext {
    /// PROPOSED. Record a predicate dependency for SSI phantom
    /// detection. No-op under Snapshot. Append-only.
    pub fn record_predicate_shared(&self, dep: PredicateDep) {
        if self.isolation == IsolationLevel::Serializable {
            // push into interior-mutable predicate_set
        }
    }
}
```

### 3.2 Что попадает в `IndexRange` vs `TableScan`

| Read shape (как приходит) | Источник | Что записываем |
|---|---|---|
| `Filter::Gt/Gte/Lt/Lte` по полю с sorted-индексом (`filter_enum.rs:20-35`) | планировщик выбрал sorted index | `IndexRange{ lo, hi }` из `range_bounds` |
| `Filter::Between` по полю с sorted-индексом (`filter_enum.rs:82`) | sorted index | `IndexRange{ lo=encode(from), hi=encode(to)+0xFF×16 }` |
| `Filter::Eq` по полю с sorted-индексом | sorted index | `IndexRange` (вырожденный интервал `[k, k+tiebreak]`) |
| `index2` range через `IndexQuery::Range` (`backend.rs:23`) | btree-backend | `IndexRange` из `lo/hi` |
| `Filter::Gt/...` по полю **без** индекса | full-table `filter_stream` | `TableScan` (coarse) |
| `Filter::Regex/Like/Computed/Fts` (`filter_enum.rs:46/38/134/117`) | brute-force scan | `TableScan` (coarse) |
| `SELECT` без `WHERE` (`read_query.rs:21` = `None`) | `list_stream` | `TableScan` (весь table — корректно) |
| Точечный `read_one_tx` (`table_manager.rs:1455`) | point | НИЧЕГО нового — это уже `read_set` |

Принцип: **precise когда читаем через sorted/btree-индекс по упорядочиваемому
значению; coarse во всех остальных случаях.** Граница ровно совпадает с
тем, что сегодня выбирает планировщик между `lookup_range` и
`filter_stream`.

### 3.3 Кодирование интервала

Значения предиката кодируются ровно тем же `sort_codec`, что и postings
sorted-индекса: `encode_i64` / `encode_f64` / `encode_str` / `encode_bool`
/ `encode_bytes` (`sorted_index_manager.rs:638-647`,
`shamir_types::core::sort_codec`). Это критично: интервал предиката и
posting-ключи конкурентной записи **обязаны лежать в одном байтовом
пространстве**, иначе пересечение бессмысленно. `range_bounds`
(`sorted_index_manager.rs:516`) уже строит физический ключ как
`SORTED_TAG ‖ name_interned(BE8) ‖ encoded_value ‖ rid(16)`
(`build_entry_key`, `:582`); `lo/hi` в `IndexRange` хранят ровно эти
физические байты (или их `Bound`), так что пересечение — это сравнение
байтовых срезов.

---

## 4. Where it hooks — точки в read-путях

Phase C **не** добавляет новый read-путь. Он навешивает запись предиката
на те же `*_tx`-обёртки, которые сегодня пишут точечный read-set. Все они
уже принимают `Option<&TxContext>` и уже само-гейтятся на Serializable —
поэтому Snapshot не платит ничего.

1. **`TableManager::read_tx` / `record_query_reads`**
   (`table_manager.rs:1554` / `:1579`) — единственная точка, где известен
   и `query` (значит `query.r#where`, `read_query.rs:21`), и `tx`. Здесь
   мы решаем `IndexRange` vs `TableScan` *до* запуска scan'а, по той же
   логике, по которой планировщик выбирает индекс. Это естественный дом
   для `record_predicate_shared`.

2. **`SortedIndexManager::lookup_range_tx`**
   (`sorted_index_manager.rs:456`) — сейчас просто форвардит и **игнорит
   `_tx`** (`:461`). Phase C: когда `tx` — Serializable, перед
   форвардингом строит `IndexRange{ lo, hi }` ровно из тех `lower/upper`,
   что `lookup_range` отдаёт в `range_bounds` (`:346`/`:516`), и зовёт
   `record_predicate_shared`. Аналогично `lookup_min_tx` / `lookup_max_tx`
   / `lookup_first_k_tx` / `lookup_last_k_tx` (`:468`/`:477`/`:496`/`:486`)
   — они сканируют весь prefix индекса ⇒ `IndexRange` с открытыми
   границами (`Bound::Unbounded`) внутри prefix'а.

3. **`index2` range scans.** Когда range обслуживается btree-backend
   через `IndexBackend::lookup` с `IndexQuery::Range { lo, hi }`
   (`backend.rs:23`) — `lo/hi` *уже* `Bound<Vec<u8>>` нужного вида.
   Phase C добавляет `lookup_tx`-ветку (default-форвард уже есть,
   `backend.rs:78`), которая записывает `IndexRange`. `IndexKind::Btree`
   (`kind.rs:11`) — это тот вид, который range обслуживает.

4. **`filter_stream_tx` / `list_stream_tx`** (`table_manager.rs:1306` /
   `:1288`). Это coarse-путь: full-table scan. Они уже пишут точечный
   read-set по выданным ключам (`record_scan_reads`, `:1345`). Phase C
   добавляет **один** `record_predicate_shared(TableScan{token})` на весь
   stream (не per-record). Точечные записи остаются — они дешёвая
   избыточность и не мешают.

5. **Откуда берётся предикат из `BatchOp`.** `BatchOp::Read(query)`
   (`executor.rs:403`) → `query.r#where: Option<Filter>`
   (`read_query.rs:21`). `Filter` (`filter_enum.rs:10`) — это та самая
   AST, которую `compile_filter` (`crates/shamir-engine/src/query/filter/eval.rs:613`)
   превращает в `FilterNode` (`eval.rs:218`) с `CompareOp`
   (`eval.rs:204`) и `Between` (`eval.rs:266`). Для derive-интервала нам
   достаточно **верхнего** `Filter` (field + op + value) — не нужно
   спускаться в скомпилированное дерево; решение «этот предикат → этот
   индексный интервал» принимается на уровне `Filter` + наличия
   sorted-индекса по полю (`SortedIndexManager::find_by_field`,
   `sorted_index_manager.rs:123`).

**Zero-overhead инвариант (как в `TRANSACTIONS_IMPL.md` §«Как НЕ замедлить
не-tx»).** Все пять точек уже имеют ранний выход «не Serializable → ничего
не делаем» (`record_scan_reads` фильтрует на `:1356`; `read_tx` — на
`:1562`). `record_predicate_shared` no-op'ит на Snapshot (`§3.1`). Non-tx
(`tx == None`) и Snapshot пути компилируются в ровно сегодняшний код.

---

## 5. Conflict detection at commit — расширение Phase 2

### 5.1 Чего сегодня НЕ хватает в gate

`validate_read_set` (`tx_context.rs:250`) спрашивает у провайдера
`version_of(table, key)` — то есть «какая сейчас версия вот этого
конкретного ключа». Для фантома этого мало: фантом — это ключ, **которого
в нашем read-set нет**. Нам нужно знать **обратное**: «какие write-key'и
закоммитили транзакции в окне `(snapshot_version, commit_version]`».

Сегодня такого журнала нет. `MvccStore::version_cache`
(`mvcc_store.rs:37`) хранит `key → version`, но (а) только для **data**
RecordId, не для index-posting-ключей, и (б) это «текущая версия ключа»,
а не «список ключей, записанных в версии V». `RepoVersionProvider`
(`version_provider.rs:10`) умеет только point-`version_of`. Значит Phase C
обязана добавить **commit-time write-key log**.

### 5.2 Commit-time write-key log (PROPOSED)

```rust
// crates/shamir-tx/src/repo_tx_gate.rs   (PROPOSED additions)

/// PROPOSED. A ring/sorted log of recently-committed write footprints,
/// owned by RepoTxGate (the natural home — it already owns commit_mutex,
/// version_counter, last_committed, active_snapshots; repo_tx_gate.rs:21).
/// Each committed tx appends ONE entry under commit_lock at publish time.
struct CommitWriteRecord {
    commit_version: u64,
    /// Data + index write-keys this tx published, grouped by table token.
    /// For phantom detection we only need the INDEX posting keys (they
    /// carry the encoded value, so they intersect predicate intervals)
    /// plus a per-table "touched" bit for the coarse TableScan case.
    /// Data RecordIds alone do NOT intersect an index interval, so the
    /// log records index posting keys explicitly.
    per_table: HashMap<u64, TableWriteFootprint>,
}

struct TableWriteFootprint {
    /// Was any row inserted/updated/deleted in this table? Drives the
    /// coarse `PredicateDep::TableScan` check.
    touched: bool,
    /// Sorted index-posting keys written (SetPosting) by this tx in this
    /// table. Drives the precise `PredicateDep::IndexRange` check. These
    /// are exactly the `IndexWriteOp::SetPosting.key`s the tx staged
    /// (tx_context.rs index_write_set; the SetPosting variant lives in
    /// shamir-tx/src/index_write_op.rs).
    inserted_index_keys: Vec<bytes::Bytes>,
}
```

Откуда берётся footprint: **бесплатно из того, что и так есть в
`TxContext` на момент commit'а.** `tx.index_write_set: Vec<(u64,
IndexWriteOp)>` (`tx_context.rs:72`) уже содержит все posting-ключи
(`SetPosting{key,..}` — см. `wal_ops_from_tx`, `commit.rs:254`);
`tx.write_set` (`tx_context.rs:67`) даёт «какие таблицы тронуты». То есть
запись в журнал — это проекция уже собранного commit-состояния, без новых
сборов.

Где живёт окно: журнал держит записи с `commit_version > min_alive()`
(`repo_tx_gate.rs:140`). Всё, что ниже `min_alive`, не нужно ни одной
живой транзакции и подрезается тем же тиком, что GC истории
(`MvccStore::gc_below`, `mvcc_store.rs:355`) — см. §7.

### 5.3 Phase 2 расширяется (PROPOSED)

В `pre_commit` (`commit.rs:558`), сразу после существующего блока Phase 2
(read-set, `commit.rs:603-615`), добавляется **Phase 2-bis: predicate
validation**. Всё под уже-held `commit_lock` (`commit.rs:380`), так что
снимок «закоммиченных в окне» консистентен — никто не может закоммитить
между нашей проверкой и нашим publish.

```rust
// PROPOSED — inside pre_commit, after the read_set Phase 2 block.
if tx.isolation == IsolationLevel::Serializable {
    // Concurrent committers = those with commit_version in
    // (tx.snapshot_version, now]. Under commit_lock this set is frozen.
    for dep in tx.predicate_set.iter() {
        let conflict = gate.predicate_conflicts(dep, tx.snapshot_version);
        if conflict {
            repo.tx_metrics().on_tx_aborted_phantom(); // PROPOSED counter
            return Err(TxError::PhantomConflict { /* dep summary */ });
        }
    }
}
```

`predicate_conflicts` (PROPOSED, on `RepoTxGate`):

```rust
fn predicate_conflicts(&self, dep: &PredicateDep, snapshot: u64) -> bool {
    // Walk only records with commit_version > snapshot (the tx's window).
    self.commit_write_log_scan(snapshot, |rec| match dep {
        PredicateDep::TableScan { table_token } => {
            rec.per_table.get(table_token).is_some_and(|f| f.touched)
        }
        PredicateDep::IndexRange { table_token, index_id, lo, hi } => {
            rec.per_table.get(table_token).is_some_and(|f| {
                f.inserted_index_keys.iter().any(|k|
                    key_in_interval(k, *index_id, lo, hi))
            })
        }
    })
}
```

`key_in_interval` сравнивает байты posting-ключа с `[lo, hi]` (учитывая
`SORTED_TAG ‖ index_id`-prefix, `sorted_index_manager.rs:574`/`:582`) —
чистое сравнение срезов, без декода значения.

**Симметрия с read-set (важно).** Существующий read-set-чек ловит «другая
транзакция записала ключ, который Я прочитал» (rw на существующем ключе).
Predicate-чек ловит «другая транзакция вставила ключ В МОЙ предикат» (rw
на несуществовавшем ключе). Вместе они дают полный набор rw-antidependency
рёбер, и при commit-gate-сериализации коммиттеров это даёт настоящую
serializability.

### 5.4 Cost analysis

- **Запись в журнал на commit:** O(|index_write_set| + |tables touched|),
  и это уже собранные данные → фактически один `Vec`-extend под
  `commit_lock`. Пренебрежимо против Phase 5a/5c физических записей,
  которые и так идут под тем же локом (`commit.rs:805`/`:880`).
- **Predicate-валидация на commit:** O(|predicate_set| × |committed в
  окне| × |index keys per committed tx|). Окно мало (живые транзакции
  короткие; Phase B удлиняет — см. §7), а `index keys per tx` — это размер
  батча. Для типичного «несколько предикатов × несколько конкурентов» —
  единицы–десятки сравнений байтов. Для precise-интервалов можно держать
  per-table posting-ключи в журнале **отсортированными** и делать
  `binary_search` по `lo`/`hi` → O(log n) на запись вместо линейного
  скана (PROPOSED оптимизация, под bench).
- **Snapshot/non-tx:** ноль. Журнал пишется только при наличии
  закоммиченных Serializable-footprint'ов; predicate_set пуст вне
  Serializable; чек гейтится на `isolation`.

---

## 6. Где именно derive интервал — от `Filter` к `lo/hi`

Чтобы не было магии, явный мост `Filter` → `IndexRange` (PROPOSED,
живёт в `read_tx`/планировщике, использует существующий `sort_codec`):

```rust
// PROPOSED. Returns Some(IndexRange) iff `f` is an order-comparison on a
// field served by a sorted index; else None → caller records TableScan.
fn predicate_to_index_range(
    f: &Filter,                      // filter_enum.rs:10
    sorted: &SortedIndexManager,     // sorted_index_manager.rs:86
    table_token: u64,
) -> Option<PredicateDep> {
    let (field, lo, hi) = match f {
        Filter::Gt  { field, value } => (field, after(encode(value)?), Unbounded),
        Filter::Gte { field, value } => (field, incl(encode(value)?),  Unbounded),
        Filter::Lt  { field, value } => (field, Unbounded, before(encode(value)?)),
        Filter::Lte { field, value } => (field, Unbounded, incl(encode(value)?)),
        Filter::Between { field, from, to } =>
            (field, incl(encode(from)?), incl(encode(to)?)),
        Filter::Eq  { field, value } => (field, incl(encode(value)?), incl(encode(value)?)),
        // And: derive a range per conjunct; record each (intersection is
        // sound but the union of per-conjunct ranges over-locks safely).
        // Or / Not / Regex / Like / Computed / Fts → None (coarse).
        _ => return None,
    };
    let def = sorted.find_by_field(&interned_path(field))?; // :123
    Some(PredicateDep::IndexRange {
        table_token,
        index_id: def.name_interned,                        // :66
        lo: physical_bound(def.name_interned, lo),          // range_bounds-style, :516
        hi: physical_bound(def.name_interned, hi),
    })
}
```

`encode(value)` = `sort_codec::encode_*` ровно как
`extract_and_encode` (`sorted_index_manager.rs:632`). `physical_bound`
оборачивает закодированное значение в `SORTED_TAG ‖ name_interned ‖ …`
как `build_entry_key`/`range_bounds` (`:582`/`:516`), чтобы байты совпали
с posting-ключами в журнале.

Если `predicate_to_index_range` вернул `None` (нет индекса / предикат не
порядковый) — записываем `TableScan{table_token}`. Это сознательное
огрубление: корректно, но over-aborts (см. §7).

---

## 7. Где станет больно (risks) — честно

В духе `TRANSACTIONS_IMPL.md` §«Где станет больно» и
`REVIEW.md` §«honest follow-ups».

1. **Ложные аборты от coarse-предикатов.** `TableScan` конфликтует с
   *любой* вставкой/апдейтом в таблицу за окно. Под write-heavy нагрузкой
   на таблицу, по которой кто-то делает full-table `Regex`-scan в
   Serializable, аборты могут стать частыми. Митигейшн: (а) поощрять
   sorted-индексы → precise-путь; (б) метрика `txs_aborted_phantom`
   (PROPOSED, рядом с `txs_aborted_ssi`, `metrics.rs:11`) делает проблему
   видимой; (в) клиент ретраит, как уже делает для `tx_conflict`
   (`executor.rs:309`). Это честный SI/SSI trade-off, не баг.

2. **Память predicate-set'а у долгих транзакций.** Каждый предикат — это
   запись, живущая до commit/abort. У короткой single-batch транзакции это
   единицы записей. **Phase B (interactive tx) удлиняет окно**: предикат,
   записанный в первом round-trip, должен пережить все последующие до
   commit'а — и одновременно держать живым commit-write-log выше своего
   snapshot'а. Это та же проблема, что long-running tx уже создаёт для GC
   (`TRANSACTIONS.md` §«Long-running tx blocks GC»; `REVIEW.md` 6.3
   max-lifetime 5 мин, `commit.rs:81`). Cross-ref: см.
   `PHASE_B_INTERACTIVE_TX.md` — лимит времени жизни tx становится ещё
   важнее, потому что он ограничивает и размер predicate-set'а, и глубину
   commit-write-log.

3. **Full-table-scan predicate = table-granularity конфликт =
   bottleneck сериализации.** Если «горячая» таблица постоянно читается
   coarse-предикатом в Serializable и постоянно пишется — это
   фактически сериализует писателей против каждого такого читателя.
   Архитектурно честно (так и должно быть для serializability без
   индекса), но операционно болезненно. Тот же совет: индекс → precise.

4. **Взаимодействие с version GC.** Commit-write-log нельзя подрезать выше
   `min_alive()` (`repo_tx_gate.rs:140`): транзакция со snapshot=S должна
   видеть всех коммиттеров в `(S, commit]`, а самый старый живой S и есть
   `min_alive`. То есть GC журнала использует **тот же** `min_alive`
   threshold, что `prune_version_cache` (`mvcc_store.rs:446`, инвариант
   разобран на `:416`). Подрезать раньше = пропустить фантом (корректность
   нарушена); подрезать позже = лишняя память. Это ровно та же дисциплина,
   что уже доказана для version_cache — переиспользуем её, не изобретаем.

5. **Zero-overhead на Snapshot обязан остаться нулём.** Любой регресс
   non-SSI пути — провал. Поэтому: `predicate_set` не аллоцируется и не
   трогается вне Serializable; commit-write-log пишется только когда есть
   Serializable-коммиты; все hooks гейтятся на `isolation` ДО любой
   работы (как `record_scan_reads`, `table_manager.rs:1356`). Регрессионный
   бенч (§9) пинит это.

6. **Index-posting-ключи в журнале vs data-ключи.** Тонкость: фантом
   обнаруживается по **index posting** ключу (он несёт закодированное
   значение и потому пересекает интервал), а НЕ по data RecordId (он
   случаен и ни в какой интервал осмысленно не попадает). Значит precise-
   путь работает только если у таблицы есть sorted-индекс по полю
   предиката — что согласовано с тем, что и сам read шёл через этот
   индекс. Если read был coarse (нет индекса), и запись тоже не создала
   relevant posting — нас спасает `touched`-бит (coarse-чек). Симметрия
   сохранена: coarse-read ↔ coarse-write-footprint.

7. **`And`/`Or` и составные предикаты.** Для `And` записываем range на
   каждый порядковый конъюнкт (пересечение интервалов было бы точнее, но
   объединение per-conjunct безопасно over-locks). `Or`/`Not` →
   `TableScan`. Это сознательная потеря точности ради простоты — отметить
   как возможную будущую оптимизацию (intersect для `And`).

---

## 8. Order of work — staged, с грубой оценкой

Зеркалит формат `TRANSACTIONS_IMPL.md` §«Order of work». Каждый шаг
landable отдельно, не ломает Snapshot/non-tx.

1. **`PredicateDep` + `PredicateSet` в `shamir-tx`** (PROPOSED new file
   `predicate_set.rs`) + поле на `TxContext` + `record_predicate_shared`
   (no-op off Serializable) + unit-тесты (interval-encode round-trip,
   no-op-on-Snapshot). (~2-3 ч)
2. **`key_in_interval` + `predicate_to_index_range`** (мост `Filter` →
   `IndexRange`, переиспользуя `sort_codec` и `range_bounds`-layout) +
   таблично-управляемые тесты на каждую форму `Filter`. (~3-4 ч)
3. **Commit-write-log на `RepoTxGate`** (PROPOSED `CommitWriteRecord` +
   append под `commit_lock` на publish + `min_alive`-подрезка) + тесты на
   окно/подрезку. (~3-4 ч)
4. **Phase 2-bis в `pre_commit`** (`commit.rs`): валидация predicate-set'а
   против журнала; `TxError::PhantomConflict` (PROPOSED) +
   `txs_aborted_phantom` (PROPOSED, `metrics.rs`). (~3 ч)
5. **Hook precise-путь:** `SortedIndexManager::lookup_range_tx` и
   соседи (`sorted_index_manager.rs:456-503`) перестают игнорить `_tx` и
   пишут `IndexRange`; `index2` `lookup_tx` пишет `IndexRange` для
   `IndexQuery::Range`. (~3-4 ч)
6. **Hook coarse-путь:** `read_tx`/`record_query_reads`
   (`table_manager.rs:1554`/`:1579`) и `filter_stream_tx`/`list_stream_tx`
   пишут `TableScan` когда индекса нет / предикат не порядковый. (~2-3 ч)
7. **GC-интеграция:** подрезка журнала на том же тике, что
   `MvccStore::gc_below` / `RepoInstance::run_gc`, с `min_alive`
   threshold; тест «журнал не подрезан ниже живого snapshot'а». (~2 ч)
8. **Канонические anomaly-тесты** (§9), failing-first. (~4-5 ч)
9. **Бенчи:** predicate-tracking overhead (Serializable) + zero-overhead
   регрессия (Snapshot/non-tx), рядом с существующими tx-бенчами
   (`crates/shamir-engine/benches/`, `REVIEW.md` 4.G.6/4.H). (~2-3 ч)
10. **Docs:** этот файл → «implemented»; `REVIEW.md` §5 (закрыть
    phantom-границу); `TRANSACTIONS.md` Q5 (можно ли поднимать default до
    Serializable, когда фантомы закрыты). (~1 ч)

**Итого: ~1.5–2 недели** сфокусированной работы. Только Phase C (phantom
protection), поверх существующего SSI. Без Phase B.

---

## 9. Test strategy — failing-first concurrency + precision + zero-overhead

Каждый сценарий — сначала **красный** тест (компилируется в баг), затем
зелёный. Размещение — рядом с существующими SSI-тестами
(`crates/shamir-engine/src/tx/tests/ssi_unique_serialization_tests.rs` —
паттерн multi-tx; новый файл `ssi_phantom_tests.rs`), плюс pure-`shamir-tx`
unit-тесты для interval-логики.

**A. Канонические аномалии (должны АБОРТИТЬ один из двух коммитов):**

1. **Phantom insert в indexed range.** Два Serializable tx делают
   `SELECT WHERE age > 30` (через sorted index по `age`), оба видят `{}`,
   оба `INSERT age=3x`. Ожидание: второй commit → `PhantomConflict`.
   (Красный сегодня: оба коммитятся — это баг из §1.)
2. **Phantom insert в `Between`-range.** `SELECT WHERE age BETWEEN 18 AND
   65`, конкурентный `INSERT age=40`. Абортит.
3. **Write-skew через предикат (doctors-on-call, но выраженный как
   `COUNT WHERE on_call=true`).** Инвариант защищается только когда
   `on_call` индексирован (precise) ИЛИ через coarse TableScan. Абортит.
4. **Coarse predicate (no index): `Regex`/full-scan.** `SELECT WHERE
   name LIKE '...'` + конкурентный `INSERT` → `TableScan`-конфликт →
   абортит (over-abort, но корректно).
5. **Phantom через UPDATE, входящий в range.** Tx1 `SELECT age>30`→`{}`;
   Tx2 `UPDATE rid SET age=35` (значение *входит* в предикат). Абортит
   (posting-ключ `age=35` попадает в интервал).

**B. Precision (НЕ должны ложно абортить, ranges не пересекаются):**

6. **Disjoint ranges.** Tx1 `SELECT age>30`; Tx2 `INSERT age=20`. Версии
   read-set не сдвинуты, posting `age=20` вне `[30, +∞)` → **commit обоих**.
   (Доказывает, что precise-путь не огрубляет.)
7. **Disjoint values, same field.** Tx1 `SELECT age BETWEEN 10 AND 20`;
   Tx2 `INSERT age=99`. Оба коммитятся.
8. **Update вне предиката.** Tx1 `SELECT age>30`; Tx2 `UPDATE rid SET
   age=5` (было `age=4`). Оба коммитятся (ни старое, ни новое не в
   интервале).
9. **Другая таблица.** Tx1 `SELECT users WHERE age>30`; Tx2 `INSERT
   orders{...}`. `table_token` разные → нет конфликта.

**C. Zero-overhead регрессия:**

10. **Snapshot-режим тот же.** Те же сценарии A1/A6 на
    `IsolationLevel::Snapshot` → predicate_set пуст, журнал не пишется,
    оба коммитятся (Snapshot не обязан ловить фантом — это by-design).
11. **Non-tx путь не тронут.** Бенч `read`/`insert` без tx: байт-в-байт
    сегодняшние числа (новый бенч рядом с `REVIEW.md` 4.H.2/3).
12. **Property/interleaving (PROPOSAL, под sanction для `proptest`):**
    рандомные расписания {SELECT-range, INSERT, UPDATE, DELETE} двух tx;
    инвариант — финальное состояние эквивалентно *какому-то*
    последовательному порядку. Закрывает то, что `REVIEW.md` §11
    отмечает как открытое («property/fuzz coverage … only example-based»).

Как и в Phase A: тесты SSI-логики гоняются на **in-memory backend** →
быстрые, в основном `cargo test` sweep; семантика идентична на всех
backend'ах, потому что весь predicate-механизм — engine-layer над dumb-KV
(тот же принцип, что `TRANSACTIONS.md` §«все backends работают одинаково»).

---

## 10. Cross-references

- [`NEXT_PHASES.md`](./NEXT_PHASES.md) — обзорная карта пост-Phase-A
  работ (Phase B / Phase C / Phase A tails). Этот файл — детализация
  ветки «Phase C». *(Если файл ещё не создан — это его место в иерархии
  roadmap'а; пока обзор живёт в `../pre-transactional/REVIEW.md` §7.)*
- [`PHASE_B_INTERACTIVE_TX.md`](./PHASE_B_INTERACTIVE_TX.md) — interactive
  multi-call транзакции. **Прямая связь:** Phase B удлиняет окно жизни
  транзакции, а значит и время, которое predicate-локи (и commit-write-
  log выше их snapshot'а) обязаны прожить — см. §7 риск 2. Phase C
  корректен и без Phase B (single-batch), но их max-lifetime-дисциплина
  общая.
- [`PHASE_A_TAILS.md`](./PHASE_A_TAILS.md) — хвосты Phase A (MED-A
  physical-write atomicity, real-crash harness, telemetry exporter — см.
  `../pre-transactional/REVIEW.md` §11 «honest follow-ups»). Phase C не
  блокируется ими и их не блокирует.
- [`TRANSACTIONS.md`](./TRANSACTIONS.md) — Phase A design (MVCC-над-KV,
  SI/SSI, single-writer commit-gate). Q5 (`../pre-transactional/REVIEW.md:446`)
  про default isolation становится re-litigable после Phase C.
- [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md) — Phase A
  implementation analysis. Этот документ намеренно держит ту же глубину,
  тот же bilingual-стиль и тот же zero-overhead-инвариант.
- [`../pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md) —
  state-of-the-world. §5.1 + §10 (SSI read-side), §11 (HIGH-C, I.1 —
  read_set был пуст в проде, теперь wired end-to-end), Q5 (default
  isolation). Phase C закрывает явную границу, задокументированную в
  `table_manager.rs:1271` и `:1549`.

---

### Реальные якоря (всё, на что опирается дизайн, существует в коде)

Точечный SSI (база, которую расширяем):
`tx_context.rs:110` (read_set), `:226` (record_read_shared), `:250`
(validate_read_set); `commit.rs:603` (Phase 2), `:609` (stub provider);
`version_provider.rs:10` (RepoVersionProvider); `repo_instance.rs:379`
(auto-attach Serializable); `mvcc_store.rs:239` (version_of).

Носители интервала (precise-путь):
`sorted_index_manager.rs:339` (lookup_range), `:456` (lookup_range_tx —
сейчас игнорит `_tx`), `:516` (range_bounds), `:582` (build_entry_key),
`:60` (SORTED_TAG), `:638` (sort_codec encode); `index2/backend.rs:23`
(`IndexQuery::Range`), `:78` (lookup_tx default), `kind.rs:11`
(`IndexKind::Btree`).

Read-пути для hooks:
`table_manager.rs:1455` (read_one_tx), `:1554` (read_tx), `:1579`
(record_query_reads), `:1345` (record_scan_reads), `:1288`/`:1306`
(list/filter_stream_tx); `executor.rs:403`/`:412` (BatchOp::Read → read_tx).

Предикат как данные:
`filter_enum.rs:10` (`Filter`), `:20-35` (Gt/Gte/Lt/Lte), `:82` (Between);
`read_query.rs:21` (`r#where`); `eval.rs:218` (FilterNode), `:204`
(CompareOp), `:266` (Between); `sorted_index_manager.rs:123` (find_by_field).

Commit-gate / журнал / GC (conflict detection):
`repo_tx_gate.rs:21` (gate), `:123` (commit_lock), `:128`
(assign_next_version), `:133` (publish_committed), `:140` (min_alive);
`commit.rs:380` (commit_lock held), `:805`/`:880` (Phase 5a/5c writes);
`mvcc_store.rs:355` (gc_below), `:446` (prune_version_cache + invariant);
`metrics.rs:11` (TxMetrics abort counters).

Recovery model (почему predicate-локи НЕ durable):
`tx/recovery.rs` — replay восстанавливает только durable commit-state
(Put/Delete/IndexPut/IndexDel/CounterDelta, `:27`-`:187`). Predicate-локи
и commit-write-log — это **in-memory conflict-detection state живущих
транзакций**, ничего durable. После краха все in-flight транзакции
аборчены (их staging пропал — `commit.rs` cancel-safe-контракт), значит
ни predicate-локов, ни «окна конкурентов» восстанавливать не нужно: окно
определено относительно snapshot'ов **живых** транзакций, а после рестарта
живых нет (`mvcc_store.rs:259` — «no snapshot survives a restart»). Это
ровно та же причина, по которой `version_cache` и `active_snapshots` не
durable. Журнал восстанавливать незачем и нельзя.

---

*Status footer: IMPLEMENTED — predicate/range SIREAD locks + Phase 2-bis
shipped; 22 phantom tests green; Snapshot/non-tx zero-overhead verified.
Benches (§9 step 9) remain an optional follow-up. Last updated 2026-05-31.*
