# STOP-темы оптимизаций — отложенные структурные изменения

Темы из `opt_crush/SUMMARY.md` и `remaining-optimizations-plan.md`,
которые были честно остановлены при попытке реализации. Каждая запись
содержит: причину блока, какой prerequisite/refactor нужен для разблока,
и оценку scope.

---

## 1. opt_crush #6 — `resolve_field_ref` cached `SmallVec<InternerKey>`

**Цель**: хранить `SmallVec<InternerKey>` вместо `SmallVec<u64>` в
`CompactPath`, чтобы избежать `InternerKey::new(id)` per access.

**Блок**: после opt_crush #3 (commit `0db79df`) `InternerKey::new` —
`#[inline]` zero-cost newtype-wrap. Компилятор уже инлайнит вызов в
hot loop. Изменение типа поля на `SmallVec<InternerKey>` — pure type
annotation rename с идентичным codegen.

**Чтобы разблокировать**: ничего — оптимизация уже выполнена компилятором.

**Status**: SKIPPED — no measurable benefit. Honest no-op.

---

## 2. opt_crush #9 — Interner `reverse-vec` append-only via `boxcar::Vec`

**Цель**: заменить `ArcSwap<Vec<Option<UserKey>>>` (CAS-loop с
`O(N)` clone) на `boxcar::Vec` (lock-free `O(1)` append) для разблока
горячего пути нового key insert.

**Блок**: текущая структура **разрежённая** (`Vec<Option<UserKey>>`)
для поддержки `touch_with_id` (commit `0e772ab`, A4-recovery). WAL
recovery replay'ит entries по произвольным id, которые могут быть
non-contiguous и out-of-order. `boxcar::Vec` — append-only, push
возвращает индекс (нельзя insert_at произвольной позиции).

**Чтобы разблокировать**: split path:
  - **Dense path** для `touch_ind` (нормальный insert, sequential
    id allocation) → `boxcar::Vec` для O(1) append.
  - **Sparse path** для `touch_with_id` (WAL recovery) → отдельная
    `scc::HashMap<u64, UserKey>` для арбитрарных id.
  - Reverse-lookup (`name(id)`): сначала проверить boxcar `[id]`,
    потом fallback в scc::HashMap.

Это medium refactor: ~150-300 LOC, требует careful invariant analysis
(coherence между двумя структурами под concurrency).

**Status**: STOP — architectural redesign required.

---

## 3. opt_crush #10/#11 — `merge_inner_maps` in-place / copy-on-write

**Цель**: `-90% clone` на UPDATE: вместо клонирования всей `old_record`
выполнять mutate in-place.

**Блок**: все 4 call sites (`execute_update`, `execute_set`, etc.)
нуждаются и в `old_record`, и в `new_record` **одновременно**:
  - equality short-circuit `new != old` перед записью.
  - `run_validators(old, new)` принимает оба.
  - changefeed emission emit'ит old + new.

Перевод сигнатуры на consuming/in-place перемещает clone-boundary
с callee на caller — total allocation count unchanged.

**Чтобы разблокировать**: restructure validator API + equality check
+ changefeed emission так чтобы они принимали разные представления
(например, `Diff` структура, или явный `&new` + `Option<&old>` где
старое значение не нужно для всех веток). Это **API change**
затрагивающий validators, changefeed, eventually internal index hooks.

**Scope**: large refactor. Out of /opti cycle.

**Status**: STOP — function already optimal given current call structure.

---

## 4. Stage B (план): Phase 5 (materialize) вне `commit_lock`

**Цель**: вынести `materialize` (Phase 5a/5c — data + index writes) из
критической секции `commit_mutex`, чтобы disjoint-table commits
запускались параллельно (×N cores).

**Блок**: visibility ordering invariant. Если materialize выполняется
после `assign_version` (Phase 3) но вне lock'а, может произойти:
  - tx1 публикуется (version N).
  - tx2 публикуется (version N+1) до того как tx1's materialize запишет
    в Store.
  - reader at version N+1 не видит tx1's data (грязное чтение).

**Чтобы разблокировать**: либо
  - **read-path фильтрует по materialize-completion** вместо
    `published_version` — больше state, сложнее invariants.
  - **publish ATOMIC с materialize** — materialize становится частью
    публикации, требует storage trait expansion (атомарный
    transact-and-publish primitive).

**Scope**: фундаментальный redesign visibility model. Не /opti цикл.

**Status**: STOP (Stage B сделал что мог — Phase 1 + 2.5/2.6 hoisted
pre-lock; commit `c2a3955`).

---

## 5. Group commit (Stage Db) — inter-batch phantom detection

**Цель**: при group commit (commit `db53955`) detect predicate
conflicts BETWEEN followers in the same batch.

**Блок**: writes survivor N's footprint не опубликованы при validate
follower N+1 (publish происходит после materialize всего batch'а).
Write-set KEY conflicts детектируются (cross-tx conflict filter via
`conflicts_with`), но predicate conflicts — нет.

**Импакт**: extremely narrow — одновременные Serializable txs с
predicate-зависимостями на writes друг друга в одном batch window.
Write-set KEY conflicts уже покрывают типичные случаи.

**Чтобы разблокировать**: batch-local footprint accumulator —
аккумулировать write-footprints accepted-so-far в batch local
structure, передавать в `pre_commit_locked_validate` каждого
follower'а вместе с committed footprints.

**Scope**: medium, локализован в `group_commit.rs` + `pre_commit.rs`.

**Status**: KNOWN LIMITATION (commit `db53955` message). Defer for
production-stress testing — if real workloads exhibit phantom misses,
implement; otherwise low-priority.

---

## 6. opt_crush Stage E (writev fan-out) — scatter-gather

**Цель**: TCP transport через `write_vectored` (`(&[u8] header,
&Bytes payload)`) — устранить последний memcpy payload на subscriber.

**Блок**: API change `PushSink::try_push_event` рябит через 8
файлов (trait def, real impl, 3 test mocks, 3 bench mocks).

**Чтобы разблокировать**: либо принять 8-file refactor (медленно, но
не структурно сложно), либо ввести параллельный API
`try_push_event_vec(IoSlices)` с default impl делегирующим в старый
`try_push_event(buf)`. Win sub-µs per subscriber — может быть в
пределах bench noise.

**Status**: STOP — Stage 40 уже взял dominant win (borrow-based
fan-out). Этот finalisation мелкий, ROI низкий.

---

## 7. opt_crush Stage F — wire format bump v1

**Цель**: координированный bump протокола перед публикацией:
  - positional msgpack response (drop `to_vec_named`)
  - typed `sub_id: u64` (intern at subscribe time)
  - framed WAL codec вместо bincode (zero-copy decode)
  - WAL v3 уже включён частично (commit `0e772ab`)

**Блок**: это **release-prep**, не /opti. Любое изменение wire
формата требует синхронного обновления TS/Rust SDK, обновления
`docs/client-server-protocol-spec/`, миграционной истории.

**Status**: DEFERRED to release-stage. Сделать когда переходим к
публикации.

---

## Сводка

| # | Тема | Разблок | Scope | Приоритет |
|---|------|---------|-------|-----------|
| 1 | resolve_field_ref InternerKey | — (auto-inline) | — | DONE-by-#3 |
| 2 | Interner reverse-vec | split dense+sparse | large | medium |
| 3 | merge_inner_maps in-place | restructure validator API | large | low |
| 4 | materialize вне commit_lock | redesign visibility model | huge | low |
| 5 | inter-batch phantom (group) | batch footprint accumulator | medium | low (narrow case) |
| 6 | writev fan-out | 8-file PushSink refactor | medium | low (sub-µs) |
| 7 | wire format bump v1 | release sync | medium | release-stage |

Каждая тема — либо реально архитектурный redesign (#2, #3, #4),
либо известный TODO с конкретным trigger'ом (#5, #6, #7).

---

## Per-crate STOP-темы (из opt_crush per-crate scan, aol agent)

### 8. funclib enum dispatch (T-fl-1)

**Цель**: scalar fn registry `HashMap<String, Fn>` → enum/index
dispatch. Per-call vtable jump + HashMap probe → match-arm.

**Блок**: требует хранить resolved `FnKind` в `FilterValue::FnCall`
(shamir-query-types). Ripples через query-builder, filter compile,
filter eval, tests — >5 files.

**Чтобы разблокировать**: координированное изменение `FilterValue::FnCall`
с миграцией всех потребителей. Само по себе мелкое (enum + match), но
scope refactor wide.

**Status**: STOP — defer.

### 9. shamir-types InnerValue Hash cache (T-types-1)

**Цель**: `Hash` для Set/Map в `InnerValue` — pre-compute и cache hash,
избегать recursive subtree walk per lookup.

**Блок**: не атаковали. Влечёт: добавление `OnceLock<u64>` (или просто
`Cell<u64>`) в `InnerValue::Map` / `Array`. Pattern matching на enum
variants становится сложнее (нужны `pub fn hash_cached(&self)` helpers).

**Чтобы разблокировать**: добавить interior-mutable hash slot в Map/Array
variants; компилировать на первом Hash::hash вызове.

**Scope**: medium (touch InnerValue + Hash impl); ROI зависит от того
насколько часто Map/Set хешируются (probably HashMap keys/Sets лookup-heavy
in subscription filters).

**Status**: DEFERRED — измерить hot-path вклад перед implementation.

### 10. shamir-types json_to_inner custom Deserializer (T-types-2)

**Цель**: `json_to_inner` сейчас double-parse через `serde_json::Value`
intermediate. Custom Deserializer строит InnerValue напрямую.

**Блок**: не атаковали. Mirror of opt_crush #8 (zero-copy msgpack
decoder, done), но для JSON path.

**Status**: DEFERRED — JSON path менее hot чем msgpack (msgpack — wire,
JSON — REST/legacy). Применить когда станет горячим.

### 11. shamir-types base58 lookup table (T-types-5)

**Цель**: Base58 encode для fixed 16-byte RecordId — division-based →
lookup table (constant-time).

**Блок**: не атаковали. Очень мелкий (~30 LOC), но требует подключения
`bs58` крейта или ручной table. ROI: RecordId::to_string() редко
вызывается на hot path (только debug/display).

**Status**: DEFERRED — low ROI.

### 12. shamir-storage async_trait box-future (T-stor-4)

**Цель**: Store trait boxed-future alloc per hot get/set/insert →
devirtualize или native impl Future.

**Блок**: async_trait alloc'ает Box<dyn Future> на каждый call. Замена:
GAT (`type GetFut<'a>: Future<...> + 'a`) — требует rust nightly или
`async fn in trait` (stable from 1.75). Trait API change → all 9
backend impls должны мигрировать.

**Scope**: large, high risk (touches 9 backends).

**Status**: STOP — выйти на это после Rust trait-impl-future становится
fully stable + benchmark показывает что boxed-fut overhead реален.

### 13. shamir-storage CachedStore lazy load (T-stor-5)

**Цель**: `new_with_mode` сейчас загружает ВСЕ данные upfront.
Lazy / background load.

**Блок**: не атаковали. Меняет initialization semantics — callers
могут полагаться что cache populated after constructor returns.

**Чтобы разблокировать**: добавить mode `LazyEager` / `LazyOnDemand`,
оставить eager mode как default backward-compat. Background load
через `tokio::spawn` с `OnceCell<bool>` flag.

**Status**: DEFERRED — useful for large datasets, измерить boot time
impact first.

### 14. shamir-tx MVCCStore B+ tree by version (T-tx-1)

**Цель**: version-chain сейчас linked-list traversal. B+ tree by
version / copy-on-write snapshots.

**Блок**: фундаментальный redesign MVCC version storage. Влечёт
изменение `MVCCStore` core + recovery + crash safety invariants.

**Scope**: huge.

**Status**: STOP — research project, не /opti.

### 15. shamir-wal recovery prefix scan (T-wal-2)

**Цель**: recovery full-scans info_store → prefix scan на WAL prefix.

**Блок**: не атаковали. Требует уверенности что WAL keys имеют
common prefix отличный от других info_store keys. Если да — простой fix.

**Scope**: small-medium.

**Status**: DEFERRED — verify prefix existence first; if confirmed,
mechanical fix.

### 16. shamir-wasm-host module cache + zero-copy (T-wh-1/2)

**Цель**: WASM module per-call instantiation → cache; Rust↔WASM
linear-memory copy → shared buffers.

**Блок**: WASM optimization is its own subdiscipline. Module cache:
need lifetime management (memory reclaim policy). Zero-copy buffers:
WASM memory model differs from Rust's; safety reviews required.

**Scope**: large for module cache, huge for zero-copy.

**Status**: DEFERRED — when WASM workloads become measured hot path.

### 17. shamir-storage iter_stream batched yield (T-stor-2)

**Цель**: InMemoryStore iter_stream snapshots whole tree → streaming
batched yield.

**Блок**: `scc::ebr::Guard` (lock-free epoch guard) is `!Send` —
cannot be held across `.await`. Current impl already collects snapshot
under one guard then drains in batch_size chunks via Vec::drain — это
**уже практический optimum** at given concurrency primitives.

**Status**: STOP — confirmed already-optimal.

---

## Финальная сводка STOP-тем

| # | Тема | Прогноз |
|---|------|---------|
| 1 | resolve_field_ref auto-inline | DONE-by-#3 |
| 2 | Interner sparse Vec redesign | medium ROI, big refactor |
| 3 | merge_inner_maps validator restructure | low ROI |
| 4 | materialize вне commit_lock | huge redesign |
| 5 | inter-batch phantom (group) | narrow case |
| 6 | writev fan-out | sub-µs, big API change |
| 7 | wire format bump v1 | release-stage |
| 8 | funclib enum dispatch | medium scope, ripple |
| 9 | InnerValue Hash cache | measure first |
| 10 | json_to_inner custom Deserializer | low-priority path |
| 11 | base58 lookup table | very low ROI |
| 12 | async_trait → GAT | huge, 9 backend ripple |
| 13 | CachedStore lazy load | measure boot impact |
| 14 | MVCCStore B+ tree | huge research |
| 15 | WAL recovery prefix scan | verify, mechanical |
| 16 | WASM module cache + zero-copy | subdiscipline |
| 17 | iter_stream batched yield | already-optimal (guard !Send) |
