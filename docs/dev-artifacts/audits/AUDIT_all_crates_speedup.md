# S.H.A.M.I.R. — полный аудит ускорений по всем крейтам

Аудит всех 23 воркспейс-крейтов на возможность **резкого** ускорения
(алгоритмическое O(N²)→O(N), устранение аллокаций в hot-loops, lock-free,
FxHash, батчинг). Только runtime-кост; proc-macro/TS-биндинги пропущены.

Найдено **40+ точек**, из них **5 HIGH** (крупные асимптотические выигрыши),
**14 MED** (hot-path коммита/чтения), остальное микро.

---

## TIER 1 — HIGH (алгоритмические, делают ×N)

### H1. Interner: reverse-vec O(N) clone-and-swap на каждый новый ключ
**`crates/shamir-types/src/core/interner/interner.rs:112-124`**
```rust
let cur = self.reverse.load_full();
let mut new_rev = (*cur).clone();        // O(N) клон ВСЕГО вектора
new_rev[new_id] = Some(key.clone());
self.reverse.compare_and_swap(&cur, Arc::new(new_rev));
```
Каждая cold-key вставка клонирует весь reverse-vec (все `UserKey(String)`).
На 10k ключей 10001-я вставка клонирует 10k String. Под конкуренцией CAS-ретраи
умножают стоимость.
**Фикс:** `boxcar::Vec` (lock-free append без клона) ИЛИ сегментированный
`Vec<Arc<[Option<UserKey>; 1024]>>` — аллоцируется только новый чанк.
**Импакт:** HIGH — доминирует cold-start / bulk-insert interner-путь.

### H2. `UserKey(String)` — heap-alloc, не дедуплицируется, глубокий клон
**`crates/shamir-types/src/core/interner/user_key.rs:3`**
```rust
pub struct UserKey(pub String);
```
Каждая запись в reverse-vec владеет `String`. При клоне на CAS-swap (H1) —
deep-clone. `Arc<str>` делает клон O(1) (refcount).
**Фикс:** `pub struct UserKey(pub Arc<str>);` — `Borrow<str>` impl работает.
**Импакт:** HIGH — мультипликативно с H1.

### H3. WalSegment.append_batch: 3N сисколов → 1 коалесированный write
**`crates/shamir-wal/src/wal_segment.rs:79-84`**
```rust
for p in &payloads {
    f.write_all(&(p.len() as u32).to_le_bytes())  // syscall 1
        .and_then(|_| f.write_all(p))             // syscall 2
        .and_then(|_| f.write_all(&crc.to_le_bytes()))?;  // syscall 3
}
```
3 `write_all` = 3 `write()` сисков на payload. N entries → 3N сисков.
**Фикс:** коалесировать все кадры в один `BytesMut`/`Vec<u8>` в цикле, затем
один `f.write_all(&coalesced)`. 3N→1.
**Импакт:** HIGH — сисколы доминируют в batch-WAL append (group-commit write path).

### H4. Per-row sequential DELETE (нет батча)
**`crates/shamir-engine/src/table/write_exec.rs:595-601`**
```rust
for id in to_delete {
    let (removed, ver) = self.delete_returning_version(id).await?;  // 6+ I/O на строку
```
Каждый delete: get + mvcc.delete_versioned + counter + 4 index hooks — всё
секвениально. INSERT имеет `insert_many`, UPDATE — `update_many`, DELETE — нет.
**Фикс:** `delete_many_returning_version` — один get_many, батченный MVCC-delete,
counter-=`count`, батченные index hooks.
**Импакт:** HIGH — O(N) секвенциальный I/O на bulk DELETE.

### H5. CachedStore/InMemoryStore iter_stream: collect ВСЕХ entries в Vec перед стримингом
**`crates/shamir-storage/src/storage_cached.rs:279-295`** (и in_memory:153)
```rust
let entries: Vec<(RecordKey, Bytes)> = {
    let g = scc::ebr::Guard::new();
    self.cache.iter(&g).map(|(k,v)| (k.clone(), v.clone())).collect()
};  // потом стримит из Vec батчами
```
Весь датасет клонируется в Vec (O(N) Bytes-клонов), потом дрейнит батчами.
Memory spike 2× датасета; latency до первого батча ∝ размеру.
**Фикс:** держать `scc::ebr::Guard` через жизнь стрима, выдавать батчи напрямую
из итератора, клонируя по `batch_size` за раз.
**Импакт:** MED-HIGH — удваивает память на full-scan, добавляет upfront-latency.

### H6. BruteForce snapshot клонирует все векторы на каждый publish
**`crates/shamir-index/src/vector/brute_force.rs:174-181`**
```rust
fn clone_snap(src) -> BruteSnap {
    vecs: src.vecs.clone(),   // Vec<Vec<f32>> deep-copy O(N×dim)
    ...
}
```
Каждый батч записей = полный клон BruteSnap. 10k×128-dim ≈ 5MB heap на publish.
**Фикс:** COW-срезы / double-buffer / `Arc<[Vec<f32>]>` (refcount-only clone).
**Импакт:** HIGH для brute-force vector-адаптера под write-нагрузкой.

---

## TIER 2 — MED (hot-path коммита / чтения)

### M1. changefeed: per-record `table.clone()` String + redundant `snapshot_ops` double-pass
**`crates/shamir-tx/src/changefeed.rs:448,453`** — `table.clone()` (String) на
КАЖДУЮ запись в батче. 1000 строк × 1 таблица = 1000 идентичных String-клонов.
**Фикс:** `Arc<str>` для `RecordChange::table` ИЛИ группировка изменений по таблице.
**`changefeed.rs:445`** — `snapshot_ops()` клонирует все keys+values, а `drain()`
потом делает то же снова. **Фикс:** переиспользовать `Vec<KvOp>` из project_event
для drain/WAL.
**Импакт:** MED.

### M2. mvcc_store: N индивидуальных `record_ts` записей в батч-пути + per-write vacuum
**`crates/shamir-tx/src/mvcc_store/mod.rs:317-319`** — `for &v in &new_versions { self.record_ts(v).await }`:
N отдельных `history.set` для таймстампов рядом с одним батченным `history.transact`.
**Фикс:** слить ts-записи в тот же `transact` (один `Vec<KvOp>` с data + ts).
**`:255,262`** — non-batch set_versioned: record_ts + vacuum_key = 2 доп. round-trip.
**Импакт:** MED — на fsync-бэкендах N×5-10ms.

### M3. O(M²) проверка конфликтов в group-commit leader
**`crates/shamir-engine/src/tx/group_commit.rs:112-114`**
```rust
let conflicts = accepted_wsk.iter().any(|a|
    a.intersection(&f.write_set_keys).next().is_some());
```
Для каждого follower итерирует ВСЕ принятые write-sets с `intersection` — O(M²).
**Фикс:** running `HashSet<(u64, Bytes)>` всех принятых ключей; проверка
`f.write_set_keys.iter().any(|k| merged.contains(k))` — O(N) на follower.
**Импакт:** MED (батчи ≥3 concurrent committers).

### M4. `intern_field_path` per-row для `FieldRef` фильтров
**`crates/shamir-engine/src/query/filter/resolve.rs:114-116`** — на каждую
сканируемую строку с `$ref`-сравнением: `intern_field_path` аллоцирует `Vec<u64>`
+ N interner-lookup'ов. Compiled-путь (`Compare` node) использует `CompactPath`,
но динамический `FilterValue::FieldRef` — нет.
**Фикс:** pre-intern один раз при компиляции (как `compile_compare`), кешировать
`CompactPath`.
**Импакт:** MED (WHERE с `$ref`).

### M5. `merge_inner_maps` клонирует всю карту на каждую UPDATE-строку
**`crates/shamir-engine/src/table/write_exec.rs:970-983`** — `orig_map.clone()`
(все 20 полей) чтобы перезаписать 2. **Фикс:** persistent-map (COW) / `im::HashMap`.

### M6. `inner_to_json_value` per UPDATE-row (INSERT уже оптимизирован)
**`crates/shamir-engine/src/table/write_exec.rs:360-363,506-509`** — O(fields)
String-аллокаций на строку при `return_result`. INSERT использует
`QueryRecord::Direct`; UPDATE — нет. **Фикс:** тот же `Direct`-путь.

### M7. `staged_vectors` deep clone в `promote_vectors`
**`crates/shamir-engine/src/tx/commit_phases.rs:294-299`** — `v.clone()` глубокий
копи `Vec<(RecordId, Vec<f32>)>` всех staged векторов. `tx` consumed после.
**Фикс:** `std::mem::take` вместо clone.

### M8. HNSW brute-force search клонирует все векторы на каждый запрос
**`crates/shamir-index/src/vector/hnsw_adapter.rs:243`** — `self.vectors.scan(|i,v| pairs.push((*i, v.clone())))`.
256 векторов × 512B = 128KB heap на запрос. **Фикс:** считать distances внутри
scan-closure, пушить `(internal, distance)`.

### M9. FTS ranked `plan_update` токенизирует old запись 2×
**`crates/shamir-index/src/fts_ranked_backend.rs:168-171`** — `tokenize_set(old)`
(внутри tokenize_with_freq) + `tokenize_with_freq(old)` снова. **Фикс:**
`(old_freq, old_doc_len) = tokenize_with_freq(old); old_set = old_freq.keys()`.

### M10. WalGroupCommit `Mutex<Vec>` на append hot-path
**`crates/shamir-wal/src/wal_group_commit.rs:96,118-119`** — все concurrent
committer'ы сериализуются на async-мьютексе push'а. **Фикс:** lock-free MPMC
(`crossbeam-queue::SegQueue` / `tokio::mpsc::try_send`).

### M11. payload.rs `make_event_data`: msgpack→JSON Value→JSON bytes double conversion
**`crates/shamir-server/src/subscriptions/payload.rs:32-41`** — декодирует
`change.key` через `rmp_serde::from_slice::<serde_json::Value>`, потом
`serde_json::to_vec`. Двойная конверсия на event. **Фикс:** прямой cursor-декод
key в `serde_json::Value`-writer без промежуточного дерева (или кешировать).

### M12. `begin_grouped_many`: per-entry encode + per-entry append
**`crates/shamir-tx/src/repo_wal_manager.rs:137-141`** — N encode + N append
вместо одного батча. **Фикс:** `append_batch(Vec<Vec<u8>>)` — один push в очередь.

### M13. `apply_data_phase` (async) секвениален per-table, `materialize` — `join_all`
**`crates/shamir-engine/src/tx/commit_phases.rs:66-91`** — AsyncIndex-путь
секвенциален, lockfree-путь параллелен. **Фикс:** `join_all` и тут.

### M14. MemBuffer `drain_once` double-clone Slot values
**`crates/shamir-storage/src/storage_membuffer.rs:347-363`** — clone в `snapshots`,
потом снова в `sets`. **Фикс:** move из snapshots (owned).

---

## TIER 3 — LOW (микро, патч- cuando)

- WalEntryV2::encode свежий Vec per entry (`wal_entry_v2.rs:211`) — thread-local scratch как V1
- `record_conflicts` `format!("{:?}", dep)` per phantom (`pre_commit.rs:220`)
- FTS ranked `HashSet<u64>` default RandomState (`fts_ranked_backend.rs:81`) → THasher
- Stopword HashSet SipHash (`tokenizer.rs:181`) → sorted-array binary search
- `InMemoryStore::set` 3× B+tree traversal on update (`storage_in_memory.rs:120`)
- `RecordId::to_bytes()` 16B alloc (`record_id.rs:69`)
- `wal_entry.clone()` в group-commit (`group_commit.rs:294`)
- `ops.clone()` per retry attempt (`commit_phases.rs:72`) → `Arc<Vec<KvOp>>`
- LayeredInterner `get_str` O(N) overlay scan (`layered_interner.rs:142`)
- `compute_write_set_keys` `Bytes::copy_from_slice` per key (`group_commit.rs:32`)
- HNSW upsert double-clone vector (`hnsw_adapter.rs:186`)
- All-tables interner checkpoint scan (`materialize.rs:339`)
- `wal_segment.rs:122` `read_to_end` whole-file (stream для больших)
- repo_tx_gate `std::sync::Mutex<Vec>` pending_commits (`repo_tx_gate.rs:105`)

---

## Уже оптимально (подтверждено) ✅

| Крейт/путь | Состояние |
|---|---|
| InternerKey | inline `u64`, zero heap-alloc ✅ |
| Forward map interner | `DashMap<_, _, THasher>` (FxHash) ✅ |
| CachedStore cache | `scc::TreeIndex` (не DashMap+sort) ✅ |
| InMemoryStore | `scc::TreeIndex` (не Vec scan) ✅ |
| completion_tracker watermark | lock-free scc::HashMap + AtomicU64 CAS, O(1) advance ✅ |
| MVCC version cache | O(1) lock-free `scc::HashMap::read`, probe `&[u8]` без Bytes-аллок ✅ |
| shamir-collections TMap/TSet | FxHash везде ✅ |
| index registry/postings | `scc::HashMap<_, _, THasher>` ✅ |
| HNSW rid-maps | `scc::HashMap<_, _, THasher>` ✅ |
| SIMD kernels | AVX2/AVX512/NEON dot/L2 ✅ |
| msgpack decode | cursor-based zero-copy (не rmpv::Value tree) ✅ |
| BruteForceAdapter top-K | bounded BinaryHeap O(N log k) ✅ |
| index write ops | батч через `Store::transact` ✅ |
| **subscription fanout** | shared decode-cache (Stage 25) + deliver-cache (Stage 29) + borrow-based fan-out (`PushEnvelopeRef<'a>` zero-copy) + InnerValue filter ✅ |
| WASM engine | disk compilation cache + pooling allocator + CoW memory ✅ |
| funclib regex | OnceLock+Mutex<HashMap<String,Regex,THasher>> cache (компилится один раз) ✅ |
| tunables | AtomicU64/AtomicUsize, lock-free O(1) read ✅ |
| transport framing | `read_frame_into` caller-supplied buf (zero-alloc steady), TCP write reused scratch ✅ |
| changefeed broadcast | `Arc<ChangelogEvent>` shared refcount ✅ |

---

## Рекомендованный порядок (impact / effort)

| # | Фикс | Импакт | Effort | Зона |
|---|---|---|---|---|
| 1 | **H3** WalSegment 3N→1 write coalesce | HIGH | ~10 строк | shamir-wal |
| 2 | **H4** `delete_many_returning_version` батч | HIGH | средне | shamir-engine/write |
| 3 | **H1+H2** Interner boxcar + `Arc<str>` key | HIGH | средне | shamir-types |
| 4 | **H5** iter_stream lazy-batch вместо collect | MED-HIGH | средне | shamir-storage |
| 5 | **M2** record_ts слить в transact | MED | мало | shamir-tx/mvcc |
| 6 | **M1** changefeed Arc<str> table + reuse KvOp | MED | мало | shamir-tx/changefeed |
| 7 | **M3** O(M²)→O(M) group-commit conflict | MED | мало | shamir-engine/group_commit |
| 8 | **M6** UPDATE `QueryRecord::Direct` | MED | мало | shamir-engine/write |
| 9 | **M7** staged_vectors mem::take | MED | тривиально | shamir-engine |
| 10 | **H6** BruteForce COW snapshot | HIGH | средне | shamir-index/vector |

H3 — наивысший ROI: ~10 строк, убирает 3N−1 лишних сисков на WAL-батч.
