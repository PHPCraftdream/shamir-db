# Crate organization — что вынести в отдельные крейты

Анализ workspace перед transactional работой: где границы могут
пройти чисто, что упростит итерацию, что — преждевременная
abstraction.

## Текущий workspace

10 крейтов в default workspace (`shamir-client-node` excluded):

```
shamir-types         ←── фундамент: InnerValue, RecordId, Interner, codecs
shamir-storage       ←── Store trait + 7 backends + MemBuffer + Cached
shamir-query-types   ←── DTO для wire protocol
shamir-engine        ←── 1.6 MB. DbInstance / Repo / Table / Index / WAL / Query
shamir-db            ←── ShamirDb facade + batch executor
shamir-connect       ←── crypto / handshake / session / rotation
shamir-server        ←── server launcher + TLS + tables registry
shamir-transport-tcp ←── TCP framing
shamir-transport-ws  ←── WebSocket framing
shamir-client        ←── high-level SDK
```

`shamir-engine` — 1.6 MB исходников, ~10 submodules. Это уже близко
к точке где монолит становится тяжёлым для итерации. Transactional
работа добавит ~10-15k строк (MvccStore, RepoTxGate, GcWorker,
TxContext, LayeredInterner, MVCC-aware Index hooks). Без выделения
получим 2 MB engine — много для одного crate.

## Что я предлагаю вынести

### A. `shamir-wal` — **ДА**, до старта Этапа 0

**Что внутри.** Текущий `crates/shamir-engine/src/wal/` — три файла,
~28 KB. `WalManager`, `WalEntry`, `WalOp`. После Этапа 0.4 туда же
переезжает `WalEntryV2` + `WalOpV2`. После Этапа 2.4 — `RepoWalManager`
тоже сюда (он логически — расширение WAL, не отдельная сущность).

**Зависимости.**
- `shamir-types` (RecordId)
- `shamir-storage` (Store trait, DbError)

Никаких import'ов из engine. Проверено:

```
$ grep -h "^use " crates/shamir-engine/src/wal/*.rs | sort -u
use bytes::Bytes;
use serde::...;
use shamir_storage::error::...;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
...
```

**Зачем выделять до transactional работы:**

1. WAL **становится центральной** transactional primitive. Repo-level
   WAL (Этап 2.4), inline-body V2 entries (Этап 0.4), recovery в две
   фазы (V1 + V2 — Этап 5.6). Всё это **расширяет** WAL — лучше
   делать в crate с собственным lib.rs, нежели в submodule среди
   table/query/index/migration логики.
2. Recovery тесты сейчас живут в `crates/shamir-engine/src/wal/`
   submodule — но завязаны на mock InMemoryStore. После extract'а
   они тестируются в **изоляции** от engine compile time —
   итерация 5× быстрее.
3. Будущие consumers WAL (например, migration coordinator со
   своим shadow log, audit chain) могут зависеть от `shamir-wal`
   без втягивания всего engine.
4. **Migration cost минимален** — 3 файла, 28 KB. Один день работы
   максимум.

**Когда делать.** Лучше **до** Этапа 0. Тогда все WAL-расширения
(WalEntryV2, RepoWalManager) пишутся в правильном крейте сразу.

### B. `shamir-tx` — **ДА**, в процессе Этапа 2-3

**Что внутри.** Новый crate, постепенно набирается в Этапах 0-3:

- Этап 0 (Foundations):
  - `version_codec.rs` — `encode_version_key` / `decode_version_key`
  - `IsolationLevel`, `TxId`, `TxConflict` (типы)
- Этап 1 (Write isolation):
  - `IndexWriteOp` enum, `apply_index_ops` helper
  - `StagingStore`
- Этап 2 (Per-repo coordinator):
  - `RepoTxGate`
  - `TxContext`
  - `LayeredInterner` (или wraps `shamir_types::Interner`)
- Этап 3 (MVCC):
  - `MvccStore`
- Этап 6 (GC):
  - `GcWorker`, `TxReaper`

Сумма ~5-7k LOC после полной реализации Phase A.

**Зависимости.**
- `shamir-types` (Interner, RecordId, InnerValue для validation)
- `shamir-storage` (Store, KvOp)
- `shamir-wal` (WalEntryV2 + RepoWalManager после extract'а)

Не зависит от `shamir-engine` — потому что MvccStore работает с
`Arc<dyn Store>`, а не с TableManager. Engine **использует**
shamir-tx, не наоборот.

**Зачем выделять:**

1. **Compile-time isolation.** Engine — большой crate. Изменение в
   MvccStore или TxContext триггерит recompile всего engine,
   query module, table module. В `shamir-tx` отдельный crate —
   recompile только этого crate + downstream consumers (engine).
   Iter cycle с 30 s → 5 s.
2. **Test isolation.** Unit tests для MvccStore / TxContext /
   RepoTxGate работают на in-memory backend, не требуют ни Table,
   ни IndexManager. Запускаются `cargo test -p shamir-tx` в
   изоляции, sub-second.
3. **Phase B (interactive transactions) естественно ложится сюда.**
   Когда придёт время — `session-scoped TxContext`, lease management
   живут в `shamir-tx`, не размазываются.
4. **Public API явный.** Что engine экспортирует наружу, что — нет,
   становится виднее. Сейчас engine — это «всё что нужно для
   работы DB» (1.6 MB) — слишком много для одного crate с публичным
   API.

**Когда делать.** Начать crate с Этапа 0 (туда складываем
`version_codec.rs` + типы). Дальше каждый этап **добавляет**
модули в `shamir-tx`, переписывая existing callers в engine на
`use shamir_tx::*`.

**Migration cost.** Нулевой — это **новый** код. Старый engine код
не трогается, кроме точек где будут import'ы.

## Что НЕ выделять (и почему)

### `shamir-keyspace` — нет

**Идея.** `SysKey` enum из Этапа 0.1 в собственном crate.

**Почему нет.** Один enum + два метода (`to_bytes` / `parse`).
~200 LOC. Crate с одним типом — overhead на издержки workspace
maintenance больше выгоды. Кладём в `shamir-engine::keyspace`
submodule, или, если хочется ещё чище, — в `shamir-tx::keyspace`
(где он используется heavy).

### `shamir-hnsw` — нет

**Идея.** Выделить vector index код из `index2/vector/` в свой crate.

**Почему нет.** Vector backend завязан на `IndexBackend` trait,
`IndexDescriptor`, `InternerKey`, `InnerValue` extraction. Это
кросс-модульный API. Выделение разорвёт logical group `index2/`.

**Когда переоценить.** Если когда-нибудь возьмёмся за external
vector adapter (Qdrant / Milvus integration), там crate boundary
естественен — `shamir-vector-external` со своим trait. Пока — нет.

### `shamir-index2` — нет (пока)

**Идея.** Выделить весь `index2/` в свой crate.

**Почему нет.**
- Завязан на `TableManager::interner()` через `LayeredInterner`
  (после Этапа 2.3).
- Hooks вызываются из `TableManager::insert/update/delete`.
- `MetaEnvelope` и `keyspace` shared с другими engine submodules.

Crate boundary получится дырявый. Логически — да, отдельный, но
practically — оставляем в engine.

**Когда переоценить.** Когда index2 станет 20k+ LOC (сейчас ~5k)
и появятся точки переиспользования (например, indexing внутри
shadow log analyzer).

### `shamir-bench-fixtures` — нет (пока)

**Идея.** Общий crate с `make_record(idx)`, `realistic_records(n)`,
`Interner::with_keys(["id", "name", ...])` для всех бенчей.

**Почему нет.** Сейчас дублирование в 3-4 benchfile'ах (~40 LOC
each). Это малая проблема. Выделение требует unification по форме
record которая будет ограничивать individual бенчи (кто-то хочет
другой shape).

**Когда переоценить.** Когда дублирование превысит ~300 LOC или
появится 10+ benchfiles с одинаковой fixture-логикой.

### `shamir-migration` — нет

**Идея.** Migration coordinator + shadow log в свой crate.

**Почему нет.** `MigrationCoordinator` использует `IndexBackend`,
`TableManager` (для replicate_index2_descriptors_from), interner
state. Это **очень engine-internal** logic. Crate boundary разрезал
бы такие cross-references неестественно.

## Сводная таблица

| Кандидат | Решение | Когда | Зачем / Почему нет |
|---|---|---|---|
| `shamir-wal` | **ДА** | До Этапа 0 | Чистые deps, центральная primitive, ускорит итерацию |
| `shamir-tx` | **ДА** | Этап 0 → растёт | Compile/test isolation, естественный home для MVCC + Phase B |
| `shamir-keyspace` | Нет | — | Один enum, overhead больше выгоды |
| `shamir-hnsw` | Нет | — | Завязан на IndexBackend, не reusable |
| `shamir-index2` | Нет | — | Дырявая граница, deep engine deps |
| `shamir-bench-fixtures` | Нет (пока) | Позже | Дублирование мало |
| `shamir-migration` | Нет | — | Engine-internal, не reusable |

## Итоговая структура workspace после Phase A

```
shamir-types
shamir-storage
shamir-wal              ←── NEW (extract из shamir-engine, до Этапа 0)
shamir-tx               ←── NEW (Этапы 0-3, инкрементально)
shamir-query-types
shamir-engine           ←── ~30% меньше (WAL + tx ушли)
shamir-db
shamir-connect
shamir-server
shamir-transport-tcp
shamir-transport-ws
shamir-client
```

12 крейтов вместо 10. Engine уменьшается с 1.6 MB до ~1.1 MB.
Граница между «движок» и «транзакционная логика» становится
явной — что упрощает понимание для новичков и code review.

## Что менять в плане preparation

Этапы 0-7 **не меняются по содержанию**, только по physical layout:

- Этап 0 начинается с пункта **0.0**: extract `crates/shamir-wal/`
  из `shamir-engine/src/wal/`. Создать новый crate, перенести три
  файла, обновить engine import'ы. ~1 день. После — все WAL
  extension'ы (0.4 inline body, 2.4 repo-level) пишутся в правильном
  crate.
- Этап 0.1 (`keyspace.rs`) — кладём в `shamir-engine::keyspace`
  submodule (не в отдельный crate).
- Этап 0 завершается созданием **пустого** `crates/shamir-tx/`
  крейта с `version_codec` + типами. Сразу с правильным `Cargo.toml`,
  rustfmt config, lib.rs.
- Этапы 1, 2, 3 — каждый добавляет модули в `shamir-tx` (а не в
  `shamir-engine/src/tx/`).
- Этап 6 — `GcWorker` и `TxReaper` в `shamir-tx`.

Никаких других изменений плана.
