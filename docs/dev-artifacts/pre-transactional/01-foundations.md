# Этап 0. Foundations

**Срок:** ~3 дня суммарно, разбитые на 9 атомарных подэтапов. **Блокатор:** всех следующих этапов.

Цель — заложить низкоуровневые примитивы, на которых стоит весь
последующий transactional layer. Каждый подпункт ниже — **отдельный
landable commit**: не ломает существующее, имеет собственные тесты,
проходит pre-commit gate (fmt + clippy -D warnings + tests).

## Состояние до старта (что уже есть)

При медитации над codebase обнаружено что foundation **частично уже
существует**:

- **`RecordId::system("tag")`** (`shamir-types`) — encoding 4-byte zero
  prefix + до 12 байт ASCII tag в одном 16-байтном RecordId.
  Используется массово: `system("internals")`, `system("indexes")`,
  `system("count")`, `system("buffer_config")`, `system("sorted_indexes")`.
- **`MetaKey` enum** в `shamir-engine/src/meta/namespace.rs` — уже
  типизированная обёртка над `RecordId::system` для 4 namespace'ов
  (`Indexes`, `Tables`, `Wal`, `Migrations` с tags `_m.idx`/`_m.tbl`/
  `_m.wal`/`_m.mig`). **Половина работы по 0.1 уже сделана.**
- **`MetaEnvelope`** в `shamir-engine/src/meta/envelope.rs` — versioned
  serialization wrapper (magic `"SDB2"` + version byte + bincode body).
  Используется в `index2/persistence.rs`. Естественный механизм для
  WAL V2.
- **`shamir-wal`** — extracted сегодня (commit `3b1b5ad`).
  `WalManager` живёт в собственном крейте.
- **`shamir-tx`** — skeleton сегодня (commit `a9791b4`).
  `version_codec` + базовые типы (`TxId`, `IsolationLevel`, `TxConflict`,
  `TxError`) commit'нуты. **D6 separator решение принято: `0xFF`** —
  см. `architectural-decisions.md`.

Это меняет план Этапа 0: вместо «создаём всё с нуля» делаем
**инкрементальный refactor + новые varианты к существующему**.

## Что НЕ делаем в Этапе 0 (отложено)

- **`Store::compare_and_swap`** — изначально планировался для version
  counter MVCC. После медитации: counter живёт **in-memory**
  (`RepoTxGate::version_counter: AtomicU64`) и инкрементится **под
  `commit_mutex`** — concurrent CAS не нужен в single-process модели.
  Возвращаем если когда-нибудь будет multi-process / sharded server.
  YAGNI до тех пор.

---

## Подэтап 0.1 — `MetaKey` extension #1: существующие inline-tags

**Срок:** 1-2 часа.

**Что.** Расширить существующий `MetaKey` enum в
`shamir-engine/src/meta/namespace.rs` вариантами для **уже
используемых** через `RecordId::system("...")` system tags:

```rust
pub enum MetaKey {
    // Existing
    Indexes, Tables, Wal, Migrations,
    // New (replace inline literals):
    Internals,       // → "internals"      (interner state)
    Count,           // → "count"           (record counter)
    BufferConfig,    // → "buffer_config"
    SortedIndexes,   // → "sorted_indexes"
    LegacyIndexes,         // → "indexes"        (deprecated path)
    LegacyIndexesUnique,   // → "indexes_unique" (deprecated path)
}
```

Заменить call-sites:
- `crates/shamir-engine/src/index/index_manager.rs:123-124,493,1212`
- `crates/shamir-engine/src/index/sorted_index_manager.rs:478,486`
- `crates/shamir-engine/src/table/buffer_config.rs:20`
- `crates/shamir-engine/src/table/interner_manager.rs:70,98,146`
- `crates/shamir-engine/src/table/record_counter.rs:25`
- `crates/shamir-engine/src/table/table_manager.rs:777`

Все эти `RecordId::system("name")` → `MetaKey::Variant.as_record_id()`.

**Tests.** Существующий test в `meta/namespace.rs::tags_are_short_and_distinct`
расширяется на новые варианты. Plus один новый: collision-check между
ALL tags (sorted dedup'd набор имеет ту же длину что исходный).

**Acceptance.**
- `rg 'RecordId::system\(\"' crates/shamir-engine/src/` возвращает 0
  совпадений вне `meta/namespace.rs`.
- Существующие 770+ тестов engine зелёные (refactor без поведенческих
  изменений).

---

## Подэтап 0.2 — `PrefixKey` enum для dynamic Tier 2 keys

**Срок:** 2-3 часа.

**Что.** Уровень system metadata это fixed 16-byte `RecordId::system`.
А для **variable-length keys** (dynamic collections с per-id encoding)
нужен параллельный typed encoding. Создать в
`shamir-engine/src/meta/prefix_key.rs`:

```rust
//! Variable-length key encoding for dynamic collections.
//!
//! Where `MetaKey` covers fixed singletons (16-byte RecordIds),
//! `PrefixKey` covers everything that needs:
//! - byte-level prefix iteration (`scan_prefix_stream`)
//! - per-id key encoding (txn_id, version, etc.)
//! - sort-stable layout (BE encoded numerics)

pub enum PrefixKey {
    /// WAL active marker: `b"__wal_active_" || txn_id_be_u64` = 21 bytes
    WalActive { txn_id: u64 },

    /// Migration shadow log entry: `b"__shadow_" || ...`
    Shadow { payload: Bytes },
}

impl PrefixKey {
    pub fn to_bytes(&self) -> Bytes;
    pub fn prefix_only(kind: PrefixKind) -> Bytes;  // for scan_prefix_stream
}
```

Заменить existing literals:
- `crates/shamir-wal/src/wal_manager.rs:14` — `const ACTIVE_PREFIX: &[u8]` →
  использует `PrefixKey::WalActive` encoding.
- `crates/shamir-engine/src/migration/shadow_log.rs:10` — `SHADOW_PREFIX`
  → использует `PrefixKey::Shadow`.

**Tests.**
- `prefix_key_round_trip` — encode → decode → original.
- `wal_active_prefix_layout` — кодирование совместимо с existing on-disk
  data (важно для backward compat — нельзя сломать recovery existing
  WAL entries).

**Acceptance.**
- `rg 'b"__' crates/shamir-engine/src crates/shamir-wal/src` возвращает
  совпадения только внутри `prefix_key.rs`.
- WAL recovery test с pre-existing data (старая on-disk layout) — зелёный.

**Где живёт.** `shamir-engine::meta::prefix_key` — рядом с `MetaKey`.
`shamir-wal` потребляет через `pub use` re-export ИЛИ через copy of
constants (TBD на момент imp — гляну циклические deps).

---

## Подэтап 0.3 — Transactional-future MetaKey варианты

**Срок:** 1 час.

**Что.** Добавить в `MetaKey` варианты, которые будут наполняться
смыслом в этапах 2-3, но **сейчас** просто резервируем tag namespace:

```rust
pub enum MetaKey {
    // ... existing ...
    /// `u64 BE` — last committed MVCC version (durable recovery marker).
    /// Read on repo open to seed `RepoTxGate::last_committed_version`.
    /// Written on every tx commit (Phase 6).
    LastCommittedVersion,   // → "_t.lcv"

    /// `u64 BE` — periodic snapshot of next tx id. Written every N
    /// commits (config, default 100). Recovery picks `max(last_seen
    /// _wal_active txn_id, NextTxId)` to seed counter.
    NextTxId,               // → "_t.nti"
}
```

Plus helper'ы в новом `shamir-engine/src/meta/recovery_marker.rs`:

```rust
pub async fn load_last_committed(info_store: &Arc<dyn Store>) -> DbResult<Option<u64>>;
pub async fn save_last_committed(info_store: &Arc<dyn Store>, v: u64) -> DbResult<()>;
pub async fn load_next_tx_id_snapshot(info_store: &Arc<dyn Store>) -> DbResult<Option<u64>>;
pub async fn save_next_tx_id_snapshot(info_store: &Arc<dyn Store>, v: u64) -> DbResult<()>;
```

Encoding — `MetaEnvelope` (8 byte body = `u64::to_be_bytes`).

**Tests.**
- Round-trip `save_last_committed(42) → load → Some(42)`.
- Missing key → `Ok(None)`.
- Tag length check (`"_t.lcv".len() <= 12`).

**Acceptance.**
- Helpers callable, типы согласованы с `RepoTxGate` сигнатурами из
  плана 03-repo-coordinator.md.
- Yet-not-used (никто не вызывает helpers в production коде), но
  компилируется и тестируется в изоляции.

---

## Подэтап 0.4 — `Store::transact` trait method + default impl

**Срок:** 0.5 дня.

**Что.** Расширить `Store` trait в `shamir-storage/src/types.rs`:

```rust
pub enum KvOp {
    Set(RecordKey, Bytes),
    Remove(RecordKey),
}

#[async_trait]
pub trait Store: Send + Sync {
    // ... existing methods ...

    /// Atomic mixed-op batch. Default impl is sequential — NOT atomic.
    /// Backends with native write tx override.
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        for op in ops {
            match op {
                KvOp::Set(k, v) => { self.set(k, v).await?; }
                KvOp::Remove(k) => { self.remove(k).await?; }
            }
        }
        Ok(())
    }
}
```

**Tests** (trait-level в `shamir-storage/tests/transact_default.rs`):
- `transact_empty_ops_noop`
- `transact_single_set` — equivalent to bare `set`.
- `transact_mixed_ops_applied_in_order` (default impl).

**Acceptance.**
- Type-checking workspace зелёный.
- Default impl используется всеми backends на этом этапе (native
  override приходит в 0.5).

**НЕ делаем в 0.4.** Native impls — это отдельный этап (0.5).

---

## Подэтап 0.5 — Native `transact` impls для 7 backends

**Срок:** 0.5-1 день (8 backends, ~30 минут каждый).

**Что.** Native override `transact` для:

- `redb` — `WriteTxn` начать → серия `tab.insert/remove` → `commit`.
- `sled` — `sled::Batch::insert/remove` → `tree.apply_batch`.
- `fjall` — `WriteBatch::insert/remove` → `commit`.
- `persy` — `Tx::insert/delete` (есть native CAS API) → `commit`.
- `nebari` — `WriteTransaction::set/remove` → `commit_with`.
- `canopy` — native batch API.
- `in_memory` — `parking_lot::Mutex` lock на DashMap, sequential
  apply.
- `cached` — proxy на inner (ALL ops через cached path, no caching for
  transactional batch — pause cache writes для consistency).
- `membuffer` — proxy на inner с pending-buffer pause.

**Tests** для каждого backend (один и тот же property test
параметризован по backend):

```rust
async fn transact_is_atomic_under_observer<S: Store>(store: Arc<S>) {
    let key_a = ...;
    let key_b = ...;
    store.set(key_a.clone(), val_old).await.unwrap();

    let observer = tokio::spawn({
        let s = Arc::clone(&store);
        async move {
            // 50 ms hammer reads to catch partial state
            for _ in 0..1000 {
                let a = s.get(key_a.clone()).await;
                let b = s.get(key_b.clone()).await;
                // a must be either val_old OR val_new — never торн state
                // a и b consistent: if a is new, b should exist
                ...
            }
        }
    });

    store.transact(vec![
        KvOp::Set(key_a, val_new),
        KvOp::Set(key_b, val_new_2),
    ]).await.unwrap();

    observer.await.unwrap();
}
```

**Benchmarks.** Расширить `crates/shamir-storage/benches/store_raw.rs`:

```rust
// New scenarios
bench_transact_set_10
bench_transact_set_100
bench_transact_mixed_100  // 50% Set + 50% Remove
```

Compare против baseline `set_many` на каждом backend. Acceptance: native
impl не медленнее чем равноценный `set_many` на том же backend.

**Acceptance.**
- 8 backends имеют native transact.
- Property test проходит на каждом.
- Bench не показывает regression vs `set_many` (≤ 5% разница допустима).

---

## Подэтап 0.6 — `Store::raw_backend()` для bypass wrappers

**Срок:** 0.5 дня.

**Что.** Без этого нельзя реализовать «tx writes минуют MemBuffer»
из 06-reconciliation.md.

```rust
#[async_trait]
pub trait Store: Send + Sync {
    // ... existing ...

    /// Return the unwrapped underlying backend, bypassing any wrapper
    /// layers (MemBuffer, Cached). Default: `None` — this Store is
    /// already raw. Wrappers override and return `Some(inner.clone())`.
    ///
    /// Used by MvccStore at construction to obtain a write path that
    /// bypasses write-back caches. Tx commits go straight to the
    /// durable backend; durability IS the commit point.
    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        None
    }
}
```

Override:
- `MemBufferStore::raw_backend` → `Some(Arc::clone(&self.inner))`.
- `CachedStore::raw_backend` → `Some(Arc::clone(&self.inner))`.

Если inner — это другой wrapper, рекурсия (raw_backend на MemBuffer
inside Cached даст оригинальный backend). Helper:

```rust
pub async fn fully_unwrap(store: &Arc<dyn Store>) -> Arc<dyn Store> {
    let mut cur = Arc::clone(store);
    while let Some(inner) = cur.raw_backend().await {
        cur = inner;
    }
    cur
}
```

**Tests.**
- `raw_backend_unwraps_membuffer` — `MemBufferStore::new(redb)` → `raw_backend`
  возвращает redb.
- `raw_backend_unwraps_cached_membuffer` — `Cached::new(MemBuffer::new(redb))`
  → `fully_unwrap` возвращает redb.
- `raw_default_none` — raw backends возвращают None.

**Acceptance.**
- Тесты зелёные.
- Yet-not-used (MvccStore не создан), но API готов для Этапа 3.

---

## Подэтап 0.7 — `WalOpV2` / `WalEntryV2` через `MetaEnvelope`

**Срок:** 0.5 дня.

**Что.** В `shamir-wal/src/wal_entry_v2.rs` (новый файл):

```rust
use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalOpV2 {
    Put { rid: RecordId, body: bytes::Bytes },
    Delete { rid: RecordId },
    IndexPut { idx_id: u32, key: bytes::Bytes, value: bytes::Bytes },
    IndexDel { idx_id: u32, key: bytes::Bytes },
    InternerOverlayMerge { entries: Vec<(u64, String)> },
    CounterDelta { table_id_interned: u64, delta: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntryV2 {
    pub txn_id: u64,
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    pub ops: Vec<WalOpV2>,
}

impl WalEntryV2 {
    /// Encode through MetaEnvelope (magic + version + bincode body).
    pub fn encode(&self) -> DbResult<Bytes>;
    pub fn decode(b: &[u8]) -> DbResult<Self>;
}
```

`MetaEnvelope` живёт в `shamir-engine`, но `shamir-wal` его не видит
(не должно быть циклической зависимости). Два варианта:

1. **Extract `MetaEnvelope`** в `shamir-types` или новый `shamir-codec`.
   Чище, но требует extra refactor.
2. **Inline** envelope logic в shamir-wal (8 lines: magic + version_u8
   + bincode). Менее DRY.

**Выбираю (2)** — inline тут, и **TODO** extract MetaEnvelope в
`shamir-types` отдельным cleanup-этапом позже. Не блокировать
transactional работу cleanup рефактором.

**Tests.**
- `wal_entry_v2_round_trip` — `decode(encode(entry)) == entry`.
- `wal_entry_v2_magic_check` — corrupted magic → ошибка.
- `wal_entry_v2_with_100_ops_size_bound` — entry с 100 inline-body ops
  по 1KB каждая → encoded < 110 KB (overhead < 10%).

**Acceptance.**
- Серде round-trip на всех 6 op-variant'ах.
- Размерный bound подтверждён бенчмарком.

---

## Подэтап 0.8 — `WalManager` dual-version read

**Срок:** 0.5 дня.

**Что.** `WalManager` сейчас умеет только V1. После 0.7 V2 entries
могут жить рядом (в той же key prefix space). Дополнить
`list_inflight`/`commit` чтобы:

```rust
pub enum WalEntryAny {
    V1(WalEntry),
    V2(WalEntryV2),
}

impl WalManager {
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntryAny>> {
        // Scan prefix as before. For each value, peek at the first byte:
        // - If matches V2 magic → decode as V2.
        // - Otherwise → decode as V1 (existing bincode shape).
        ...
    }
}
```

Existing `begin/commit/commit_async` остаются V1-only. New
methods `begin_v2(WalEntryV2) / commit` появятся в `RepoWalManager`
(этап 03-repo-coordinator).

**Tests.**
- `mixed_v1_v2_listing` — записать одну V1 и одну V2, `list_inflight`
  возвращает обе с правильными variant'ами.
- `legacy_v1_only_still_works` — на DB без V2 entries existing recovery
  flow зелёный.

**Acceptance.**
- 0 регрессии в existing WAL tests.
- Mixed-version listing верифицирован.

---

## Подэтап 0.9 — Документация принятых decisions

**Срок:** 30 минут.

**Что.** В `architectural-decisions.md` добавить **D6** —
version_codec separator decision:

```markdown
## D6. version_codec separator = `0xFF`

**Проблема.** Физический ключ для MVCC history — `key || sep ||
version`. Какой byte использовать как separator?

**Выбранное решение.** `0xFF` (одиночный byte).

**Почему не иначе.**
- `0x00` — встречается в `RecordId::system("name")` (4-byte zero prefix
  + tag). Collision likely.
- `\\` или `:` (ASCII) — могут встретиться в interner-encoded keys.
- Length-prefix encoding (`varint(key_len) + key + version`) — чище
  семантически, но 2-5 байт overhead вместо 1.

`0xFF` крайне редок в RecordId (uniformly random crypto bytes) и
никогда не встречается в `system` RecordIds (4-byte zero prefix
+ ASCII tag, ASCII never reaches 0xFF). History store отдельный от
main, поэтому decode_version_key вызывается **только** на keys,
которые мы сами туда положили — invariant контролируем.

**Реализовано в** commit `a9791b4` (`shamir-tx/src/version_codec.rs`).

**Test + bench coverage.**
- `round_trip` — encode → decode → original (5 значений version).
- `sort_order_matches_version` — BE encoding sorts naturally.
- `different_keys_dont_interleave` — `aaa::MAX < aab::0`.
- `missing_separator_decodes_to_none` — corruption detection.

Не требуется отдельный benchmark — функция тривиальная (BytesMut +
4 instructions).
```

**Acceptance.** Commit'нуто, decision matrix в `architectural-decisions.md`
расширена строкой D6.

---

## Итоговый порядок Этапа 0

| # | Что | Срок |
|---|---|---|
| 0.1 | `MetaKey` extension #1: существующие inline-tags | 1-2 ч |
| 0.2 | `PrefixKey` enum для dynamic Tier 2 keys | 2-3 ч |
| 0.3 | Transactional-future `MetaKey` варианты + recovery_marker helpers | 1 ч |
| 0.4 | `Store::transact` trait method + default impl | 0.5 дня |
| 0.5 | Native `transact` impls для 7 backends + bench | 0.5-1 день |
| 0.6 | `Store::raw_backend()` + MemBuffer/Cached override | 0.5 дня |
| 0.7 | `WalOpV2` / `WalEntryV2` types + envelope encoding | 0.5 дня |
| 0.8 | `WalManager` dual-version `list_inflight` | 0.5 дня |
| 0.9 | Doc decision D6 (version_codec separator) | 30 мин |

**Итого ~3 дня.** Каждый подэтап — отдельный коммит, отдельный PR,
независимый, не блокирует другие в Этапе 0 (кроме линейной зависимости
0.7 → 0.8 в коде).

Подэтапы можно делать **параллельно** в принципе (0.1+0.2+0.3 трогают
metadata namespace, 0.4-0.5 — Store trait, 0.7-0.8 — WAL). На практике
один разработчик идёт линейно по списку.

## Что НЕ делаем в Этапе 0 (резюме)

- **CAS** — отложено до multi-process scenarios. YAGNI.
- **Index2 hook rewrite** — это Этап 1.
- **TxContext / RepoTxGate** — это Этап 2.
- **MvccStore** — это Этап 3.
- **Use of `WalOpV2` в production write paths** — это Этап 4 (executor
  commit). В Этапе 0 только types + tests.
