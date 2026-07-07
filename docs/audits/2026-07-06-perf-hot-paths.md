בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Перф-ревью горячих путей S.H.A.M.I.R. (engine / storage / types / index / server)

_Агент: @fxx (max effort), 2026-07-06. Часть панели из 5 агентов ревью проекта после завершения векторной кампании._

Все файлы из незакоммиченного набора (`crates/shamir-index/src/vector/hnsw_adapter.rs`, `crates/shamir-engine/src/tx/*`) анализировались **по HEAD** (`git show HEAD:`), номера строк для них — HEAD-овские. Известное из таски VR-7 (мёртвый `let _int_core = dot_u8(...)` в `sq8.rs::approx_dot` + отсутствующий норм-кэш Cosine для кандидатов) — не переоткрываю, смежные, но отдельные вещи помечены «смежно VR-7».

---

## 1. ОБЯЗАНЫ УЛУЧШИТЬ (перф-баги на per-op пути)

**1.1 [CRITICAL] FTS/index2-запрос с пустым результатом проваливается в полный скан**
`D:\dev\rust\shamir-db\crates\shamir-engine\src\table\read_exec.rs:344` — `if !rids_vec.is_empty() { …return }`: пустой ответ индекса (частый случай для поисковых запросов «ничего не найдено») не возвращается, а падает сквозь весь каскад планировщика в full scan, где `FilterNode::FtsMatch` брутфорсит **каждую строку** (токенизация + `to_lowercase()`-аллокации на строку, `filter_node.rs:604`). Для `VectorSimilarity` хуже: он компилируется в `FilterNode::True` (`compile.rs:123`) — пустой индекс отдаст *все* строки.
Платим: O(N × tokenize) на каждый miss-запрос. **Это уже видно в собственном бенче**: `crates/shamir-engine/benches/fts_indexed.rs:200-227` — `indexed_and` (0 hits) ≈ `brute_and` (~22 ms на N=10k); комментарий бенча ошибочно утверждает «indexed avoids get_many entirely (empty rid set → early-return)» — такого early-return в коде **нет**.
Как ускорить: различать «индекс авторитетно ответил: пусто» (→ вернуть пустой `QueryResult`) и «индекса нет» (→ fallback). Проверить, что build при CREATE INDEX синхронен (build_backend.rs) — тогда пустой ответ авторитетен.
Выигрыш: **~400× на miss** при N=10k (22 ms → ~50 µs), растёт с N.

**1.2 [HIGH] Keyset-seek (`Pagination::After`) фетчит всю полуплоскость**
`D:\dev\rust\shamir-db\crates\shamir-engine\src\table\read_index_scan.rs:443-531`: `lookup_range(seek_key, +∞)` возвращает `BTreeSet<RecordId>` **всех** записей за ключом (`crates/shamir-index/src/legacy/sorted_index_manager.rs:546-569` — стрим собирается в set, теряя value-порядок), затем `get_many_bytes` на всё, проекция всего, полная сортировка `apply_order_by_qv`, и только потом `truncate(limit)`.
Платим: O(остаток таблицы) fetch+decode+project+sort на страницу; полная прокрутка пагинацией — O(N²). Keyset-пагинация существует ровно чтобы этого не было.
Как ускорить: `lookup_range_first_k(name, lo, hi, k, direction)` — индекс уже упорядочен по значению; идти по стриму, отбрасывать равные seek-ключу, остановиться на limit. Убирает и сортировку, и полный фетч.
Выигрыш: **до N/limit× на страницу** (1M строк, limit=100 → ~10 000× на ранних страницах). Бенча на keyset-seek нет вообще (см. «покрытие»).

**1.3 [HIGH] UPDATE: полная де-интернизация old/new на каждую изменённую строку даже без валидаторов**
`D:\dev\rust\shamir-db\crates\shamir-engine\src\table\write_exec.rs:521-560`: для каждой изменённой строки безусловно строятся `old_qv` (полный `record_view_to_query_value` — String-аллокация на каждый ключ + owned-значения), затем `old_qv.clone()` (глубокий клон) + оверлей — и всё это лишь чтобы вызвать `run_validators_qv`, который при нуле валидаторов мгновенно выходит (`table_manager_validators.rs:150`). DELETE-путь уже гейтится (`has_delete_validators`, строка 630), UPDATE — нет. Вдобавок при `wants_records && changed` те же байты де-интернируются **второй раз** (строки 533-538 и 580-585).
Платим: 2-3 полных материализации записи на строку впустую.
Как ускорить: hoisted `has_update_validators` (как в delete); при RETURNING переиспользовать уже построенный `old_qv`.
Выигрыш: массовый UPDATE без валидаторов/RETURNING **~1.5-2×**; с RETURNING — минус одна материализация на строку. То же в `execute_set_tx` MERGE-ветке (строки 942-969).

**1.4 [MED] Оверлейный probe аллоцирует копию ключа на каждое точечное чтение**
`D:\dev\rust\shamir-db\crates\shamir-tx\src\versioned_overlay.rs:286-288` (`peek`), `:127-128` (`newest_visible` — две копии), `:101` (`remove`): `Bytes::copy_from_slice(key)` для сборки composite-ключа `(Bytes, u64)`. Probe стоит **перед** history на каждом `get_current_bytes`/`resolve_read`/`get_current_many`-ключе — т.е. на каждом MVCC point-read. Ирония: III.2 (`mvcc_store/mod.rs:1086-1095`) убрал ровно такую же аллокацию для `cells`, а оверлей её вернул.
Платим: +1..2 alloc + memcpy(16B) на каждое чтение.
Как ускорить: ключ оверлея — фикс-массив `([u8;16], u64)` / `(u64,u64)` из RecordId (без heap).
Выигрыш: −1..2 alloc/op, ~3-5% на point-read (виден в `read_path_matrix`).

**1.5 [MED] `ops.clone()` в замыкании ретрая Phase 5a — O(N)-клон на каждый коммит**
`crates/shamir-engine/src/tx/commit_phases.rs:68-77` и `crates/shamir-engine/src/tx/materialize.rs:79-88` (оба HEAD): `retry_materialize(…, || apply_data_batch(…, base.clone(), ops.clone(), …))` клонирует весь вектор ops при **каждом** вызове замыкания, включая первый успешный. `mvcc.apply_committed_visible` уже принимает `&ops`; Vec нужен только редкому non-MVCC `base.transact`.
Платим: alloc + N×enum-copy (Bytes-refcount) на коммит; для батча 10k строк — 10k-элементный клон.
Как ускорить: `apply_data_batch(…, ops: &[KvOp], …)`, клон только в non-MVCC ветке.
Выигрыш: −1 O(N) клон на коммит (×3 при ретраях).

**1.6 [MED] `promote_vectors` глубоко клонирует все staged-эмбеддинги на коммит**
`crates/shamir-engine/src/tx/commit_phases.rs:266-273` (HEAD): `.map(|(t, v)| (*t, v.clone()))` — полный memcpy всех векторов (N×dim×4B), плюс `staged_vector_deletes_for(…).to_vec()`. `apply_vector_batch` принимает `&[(RecordId, Vec<f32>)]`, а `tx: &TxContext` жив весь вызов — клон не нужен.
Платим: например 1k векторов dim=768 → ~3 MB memcpy на коммит.
Выигрыш: −O(N·dim) memcpy на векторный коммит.

---

## 2. ГДЕ ОСТОРОЖНОСТЬ УПУСКАЕТ ПЕРФ (деградации под нагрузкой)

**2.1 [HIGH] `MvccStore.ts_index` растёт неограниченно**
`D:\dev\rust\shamir-db\crates\shamir-tx\src\mvcc_store\mod.rs:163` — in-memory `TreeIndex<(Reverse<ts>, Reverse<version>), ()>`; вставка на **каждую** закоммиченную версию (`:627, :726, :824, :885`, `mvcc_history.rs:320,506`), удаления **нет нигде** (grep подтверждает: только insert/query/rebuild). Vacuum/purge чистят history, а этот индекс — нет.
Платим: ~50-64B на версию навсегда; при 5k writes/s — гигабайты в месяц, плюс деградация O(log N) по мёртвому N.
Как ускорить: прунить в `gc_overlay_to`/vacuum/purge (range-remove по ts ≤ purge-watермark; ключи уже реверсные — диапазон хвостовой), либо периодический rebuild.

**2.2 [HIGH] `MvccStore.cells` никогда не эвиктится**
`mvcc_store/mod.rs:118` — по одному `RecordCell` на каждый когда-либо тронутый ключ, включая удалённые; `cells.remove`/`retain` в крейте отсутствуют. Queue-подобная нагрузка (insert+delete) копит мёртвые cells (~100B/ключ) бессрочно.
Как ускорить: снимать cell при vacuum tombstone-версии (когда version ≤ durable и нет снапшотов) — cold-start-путь (`seek_latest_version`) корректно обработает отсутствие cell.

**2.3 [HIGH] MemBuffer: любой скан принудительно дренирует dirty на диск**
`D:\dev\rust\shamir-db\crates\shamir-storage\src\storage_membuffer.rs:521-605`: `iter_stream`/`scan_prefix_stream`/`iter_range_stream*` сначала крутят `drain_once` до пустоты, затем читают **inner** (мимо moka). Так как `BoxRepo::MemBuffer` оборачивает *все* сторы (`crates/shamir-engine/src/repo/repo_types.rs:57-60`), под смешанной нагрузкой каждый full-scan запрос, каждый FTS-token prefix-scan (2.4) и каждый sorted-index range-scan = форс-флаш записи (read-triggered write amplification) + потеря батчинга fsync (500ms-интервал обесценивается).
Как ускорить: не дренировать, а мерджить снапшот dirty (маленькая сторона) поверх inner-стрима — тот же приём, что `current_stream` уже делает с overlay (`mvcc_store/mod.rs:1030-1046`).
Выигрыш: стабильный p99 сканов под записью; меньше fsync-ов. Бенча на scan-under-writes нет (`membuffer_pump` меряет insert/get).

**2.4 [MED] FTS: prefix-scan хранилища на каждый токен каждого запроса, без кэша постингов**
`crates/shamir-index/src/fts_backend.rs:95-110`, `fts_ranked_backend.rs:271-276`: `scan_prefix_stream` в info_store per token per query. Legacy `IndexManager` кэш имеет (и hook инвалидации при коммите уже есть — `commit_phases.rs:434` HEAD), FTS-бэкенды — нет. В сочетании с 2.3 каждый токен-скан ещё и дренирует буфер.
Выигрыш: горячие термы из RAM — ×5-50 на запрос против дискового скана.

**2.5 [MED] BM25 AndAll: полные BTreeSet-ы по каждому токену до пересечения**
`fts_ranked_backend.rs:280-288`: O(Σ|postings|·log) даже когда пересечение пусто. Сортировать по df, пересекать от наименьшего с early-exit; скоринг — только по пересечению.

**2.6 [LOW-MED] Full-scan через MVCC = проход по всем версиям + ts-ключам**
`mvcc_store/mod.rs:1016-1081`: `current_stream` итерирует весь version-log (все версии + ts-записи) со streaming group-by. При `Retention::keep_history` или отстающем vacuum скан деградирует O(total versions), а не O(live rows). Зафиксировать как ограничение; при history-heavy профиле нужен отдельный live-набор.

**2.7 [LOW] Duplex request loop: свежий `Vec::new()` на каждый фрейм**
`crates/shamir-server/src/connection/request_loop.rs:218` — транспорт спроектирован под reuse (`shamir-transport-tcp/framing.rs` даже избегает zero-fill), но duplex-петля теряет это, т.к. буфер уезжает в spawn-таск. Пул буферов (возврат через канал/ArrayQueue) = −1 alloc(размер фрейма)/запрос.

---

## 3. УЛУЧШИТЬ (батчинг, мёртвые вычисления)

**3.1 [MED] `execute_set_tx`: полный энкод записи выбрасывается на MERGE-ветке** (класс `_int_core`)
`crates/shamir-engine/src/table/write_exec.rs:894-899`: `new_bytes_fresh = query_value_to_storage_bytes(...)` считается **всегда**, но используется только в INSERT-ветке; для upsert-обновления (частый случай) — мёртвый полный энкод записи. Перенести в INSERT-ветку. −1 энкод записи на upsert-merge.

**3.2 [MED] Тройной проход по staged ops на коммит + String-клон имени таблицы на каждое изменение**
`crates/shamir-engine/src/tx/commit.rs:198-222` (HEAD, `wal_ops_from_tx` → `snapshot_ops`), `commit_phases.rs:338-355` (`collect_data_batches` → `drain`), `crates/shamir-tx/src/changefeed.rs:451-475` (`project_event` → снова `snapshot_ops` + `table.clone()` String на **каждое** изменение; событие строится на каждый коммит — журнал пишется всегда). Fix: `RecordChange.table: Arc<str>`; строить событие из уже собранных `data_batches` за тот же проход. −1 полный проход + −N String-аллокаций на коммит.

**3.3 [MED] Covering-read: `interner.touch_ind(имя_поля)` на каждую строку × поле**
`crates/shamir-engine/src/table/read_index_scan.rs:120-131`: имена полей одинаковы для всех строк — а резолвятся через sharded-map на каждой строке. Hoist в map имя→ключ до цикла. −(rows×fields) DashMap-лукапов (~50-100ns каждый).

**3.4 [LOW] `SelectProjection::project_value`: пересборка `SmallVec<[InternerKey;4]>` per row per field**
`crates/shamir-engine/src/query/read/select_projection.rs:90-97`. Хранить готовый SmallVec в `SelectProjection::new`. −1 collect (и alloc при path>4) на строку/поле.

**3.5 [LOW] `format!("{:?}", key)` на каждый NotFound/tombstone-хит в MemBuffer**
`storage_membuffer.rs:472, 484` — Debug-формат + аллокация на каждом промахе get. Для exists-check нагрузок — заметно. Ленивое/статическое сообщение.

**3.6 [LOW] S-write id-msgpack: `Bytes::copy_from_slice(buf)` на запись**
`write_exec.rs:303` — memcpy тела записи, хотя `ByteBuf` уже владеет Vec. Дать `InsertOp` отдавать владение → `Bytes::from(vec)`. −1 memcpy(record)/строку. Аналогично `read_filtered_vector_scan` клонирует вектор запроса на каждую итерацию retry (`read_exec.rs:1427`).

---

## 4. УСКОРИТЬ (SIMD / zero-copy / прекомпьют)

**4.1 [HIGH] Fused rescore для SQ8: деквант-аллокация и пере-норма на каждого кандидата**
`crates/shamir-index/src/vector/quantized_dist.rs::rescore_f32` (HEAD): `params.dequantize(codes)` — **Vec<f32> (dim×4B) на кандидата**, плюс `dot(query,query)` пересчитывается на каждого кандидата. Overscan квантованного поиска = `16k+64` (`hnsw_adapter.rs:1642` HEAD) → при k=10 это 224 деквант-аллокации + 224 клона кодов (`:1658`; в `search_prefilter` — до **4096** кандидатов, `:1117-1122` → ~12 MB аллокаций на запрос при dim=768).
Fix: прекомпьют на запрос: `qm = dot(query, mins)`, `qs[i] = query[i]·scales[i]`, `q_norm`; тогда `dot(query, dequant(c)) = qm + Σ qs[i]·c[i]` — один SIMD-проход по u8-кодам (u8→f32 convert + FMA), ноль аллокаций; L2 раскладывается аналогично. Рескорить внутри `read`-замыкания scc (без клона кодов).
Выигрыш: rescore **×2-4**, −(2×overscan) alloc/поиск; prefilter-путь — на порядок меньше аллокаций. (Смежно с VR-7, но это про rescore-аллокации/fused-ядро, не про норм-кэш кандидатов.) Покрыто бенчем `quantization_f32_vs_sq8` — эффект будет виден.

**4.2 [HIGH] Ядро обхода u8-графа — скалярный per-dim цикл с пересчётом констант**
`crates/shamir-index/src/vector/sq8.rs::approx_dot / approx_l2_sq` (HEAD): вызывается на **каждом ребре** HNSW-обхода (≈ef×M раз на поиск и вставку); в цикле пересчитываются `m*s` и `s*s`. Шаг 1 (тривиальный): прекомпьют в `fit()` `scales_sq[i]`, `min_scale[i]` → −2 fmul/dim, ~×1.3-1.5 скалярно. Шаг 2: SIMD-ядра weighted-dot/weighted-L2 (u8→f32 convert + FMA; каркасы уже есть в `simd.rs` — `dot_product_avx2/avx512/neon`).
Выигрыш: traversal-ядро **×3-6** (AVX2) → квантованный поиск/вставка приближаются к memory-bound. (VR-7 закрывает только мёртвый `dot_u8` и норм-кэш Cosine — SIMD-изация weighted-ядер отдельна.)

**4.3 [MED] Brute-force арм search клонирует весь векторный набор на каждый запрос**
`hnsw_adapter.rs:1609` (u8: 512×dim B) и `:1673` (f32: 256×dim×4 B — при dim=1536 это 1.5 MB memcpy на поиск) — `scan(|i, v| pairs.push((*i, v.clone())))`. Считать дистанцию **внутри** scan-замыкания (чистый CPU, блокировки нет): `pairs: Vec<(usize, f32)>`. Малые индексы (≤256/512 векторов — все свежие таблицы) ×1.5-3, ноль копий. Аналогично f32-ветка `search_prefilter` (`:1126`) — eval внутри `read_async`-замыкания вместо клона.

**4.4 [MED] 3 async-лукапа на кандидата в rescore-циклах**
`hnsw_adapter.rs:1654-1664` (HEAD): `deleted.contains_async` + `vectors_u8.read_async(clone)` + `rid_map.read_async` — три await-точки на кандидата × 224 кандидата. Sync `contains`/`read` (без await — гварды не держатся через await) + rescore в замыкании: минус future-poll оверхед и клоны.

**4.5 [LOW-MED, смежно VR-7] Cosine f32: три прохода по векторам на каждый eval**
`hnsw_adapter.rs:76-90` (HEAD): `dot(a,b)`, `dot(a,a)`, `dot(b,b)` — три отдельных SIMD-прохода на каждом ребре f32-графа. VR-7 кэширует нормы кандидатов; дополнительный шаг — fused single-pass ядро (dot+обе нормы за один проход) для оставшегося: ещё ×1.5-2 на Cosine-traversal.

---

## Что горячо, но не измеряется (пробелы бенчей)

- **Keyset-seek** (`Pagination::After`) — ни одного бенча; находка 1.2 сейчас невидима.
- **FTS-miss** — измерен (`fts_indexed`, mode=and, 0 hits), но данные противоречат комментарию бенча: full-scan fall-through уже задокументирован цифрами (~22 ms на N=10k) и неверно объяснён.
- **MemBuffer scan-under-writes** (drain-before-scan) — `membuffer_pump` меряет insert/get, скан под записью не покрыт.
- **Массовый UPDATE** (10k строк, с/без валидаторов) — `engine_perf` покрывает только точечный update_by_id.
- **Рост `ts_index`/`cells`** — memory-профиль долгой работы не измеряется ничем (нужна метрика/soak-тест).

## Топ-5 самых сочных ускорений

| # | Находка | Файл | Выигрыш | Сложность |
|---|---------|------|---------|-----------|
| 1 | FTS/index2 пустой результат → full scan | `read_exec.rs:344` | **~400×** на miss-запрос (N=10k), растёт с N | Низкая (гейт + каветка про backfill) |
| 2 | Keyset-seek фетчит полуплоскость | `read_index_scan.rs:443` + `sorted_index_manager.rs:546` | до **N/limit×** на страницу; глубокая пагинация O(N²)→O(N) | Средняя (новый ordered-`first_k` API) |
| 3 | UPDATE: мёртвая де-интернизация old/new без валидаторов | `write_exec.rs:521-560` | **1.5-2×** массовый UPDATE; −2-3 материализации/строку | Низкая (гейт как в delete) |
| 4 | Fused SQ8-rescore + SIMD weighted-ядра + прекомпьют `scales_sq` | `quantized_dist.rs`, `sq8.rs`, `hnsw_adapter.rs:1642-1664` | поиск/вставка в квант. индексе **×2-6**, −сотни alloc/запрос | Средняя (ядра по образцам simd.rs) |
| 5 | MemBuffer drain-before-scan (read-triggered write amplification) | `storage_membuffer.rs:521-605` | стабильный p99 сканов под записью; сохранение fsync-батчинга | Средняя (merge-overlay вместо drain) |
