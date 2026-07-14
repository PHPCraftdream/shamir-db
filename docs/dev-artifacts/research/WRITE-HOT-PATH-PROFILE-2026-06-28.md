בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Write hot-path profile — `tx_pipeline indexed/tx/1000`

> **Дата:** 2026-06-28. **Среда:** WSL2 Ubuntu 24.04 на Windows 10
> (kernel 6.18-microsoft, perf 6.8.12). **Бенч:** `shamir-engine` /
> `tx_pipeline` / `tx_overhead/batch_pipeline/indexed/tx/1000` × 30 s.
> **SVG:** `D:\dev\rust\shamir-db\.flamegraphs\shamir-engine-tx_pipeline-symbols.svg`
> (1.1 MB). **perf.data:** `/tmp/perf-tx_pipeline.data` (16 MB, **3741 сэмплов**).

> **Зачем:** первый targeted flamegraph-pass на write hot-path, чтобы понять,
> куда РЕАЛЬНО уходит время. Результат изменил мою рабочую гипотезу
> «over-allocation» на куда более интересную картину — см. §3.

---

## 1. Методология (для воспроизведения)

### 1.1 Скрипт
`scripts/wsl-flame-bench.sh` (запушен): `cargo flamegraph --bench <name> -p <crate> --no-inline -c "record -F 99 --call-graph dwarf,4096 -g ..."`.

### 1.2 Критические настройки (которые я угадывал по очереди)

| Настройка | Зачем | Что было до фикса |
|---|---|---|
| `CARGO_PROFILE_RELEASE_DEBUG=1` + `_STRIP=false` (и BENCH-зеркало) | Cargo.toml ставит `strip=true,debug=false` в `[profile.release]`; bench-профиль inherits → Rust-символы выпиливаются на линковке | Все имена `[unknown]` / адреса в SVG |
| `--call-graph dwarf,4096` (не default 8192) | WSL2 6.18 vs perf-6.8 расхождение mmap-протокола: stack 8192 → `Bad address` | `failed to write perf data` |
| `-F 99` + один узкий бенч + `--profile-time 30` (вместо всех 16 конфигураций) | Управляемый размер `perf.data` (16 MB vs 110 MB) + достаточно сэмплов | Часы post-processing |
| `--no-inline` у `cargo-flamegraph` | Inline-frame резолв через addr2line на Rust-binary с LTO — главное узкое место post-processing | `perf script` 30+ минут (!) |
| `rm -rf ~/.debug` перед запуском | Битый build-id symbol-кэш блокирует addr2line | Бесконечные `could not read first record` warnings |
| `perf` через `/usr/lib/linux-tools-6.8.0-124/perf` (не `/usr/bin/perf`) | wrapper ищет perf под current kernel, для WSL2 Microsoft 6.18 пакета нет; ABI-совместимость 6.8 ≈ 6.18 для record/report | `WARNING: perf not found for kernel 6.18.33.2-microsoft` |

> **Урок:** WSL2 + Rust LTO + full DWARF + perf — это слой за слоем плохо
> совместимостей. Каждый из них надо обходить отдельно. Скрипт `wsl-flame-bench.sh`
> теперь содержит все эти обходы.

### 1.3 Анализ

```bash
/usr/lib/linux-tools-6.8.0-124/perf report \
  --input=/tmp/perf-tx_pipeline.data \
  --stdio --no-children -g none
```

(Завёрнуто в `scripts/wsl-perf-report.sh` для удобства.)

---

## 2. TOP 35 self-time (Rust символы видны)

```
     7.70%  libc.so.6                     [.] __memcmp_evex_movbe
     3.38%  tx_pipeline                   [.] core::sync::atomic::atomic_load
     3.22%  libc.so.6                     [.] __memmove_evex_unaligned_erms
     3.20%  tx_pipeline                   [.] <dashmap::iter::Iter as Iterator>::next
     2.40%  tx_pipeline                   [.] core::sync::atomic::atomic_compare_exchange_weak
     2.26%  tx_pipeline                   [.] <alloc::sync::Weak as Drop>::drop
     2.14%  tx_pipeline                   [.] core::sync::atomic::AtomicUsize::fetch_add
     1.96%  tx_pipeline                   [.] core::sync::atomic::AtomicUsize::fetch_sub
     1.80%  tx_pipeline                   [.] <alloc::sync::Arc as Drop>::drop
     1.62%  tx_pipeline                   [.] hashbrown::raw::inner::RawTableInner::iter
     1.61%  tx_pipeline                   [.] core::sync::atomic::AtomicUsize::fetch_sub  (другой call-site)
     1.53%  tx_pipeline                   [.] scc::tree_index::node::Node::search_entry
     1.36%  tx_pipeline                   [.] <Range<T> as RangeIteratorImpl>::spec_next
     1.32%  tx_pipeline                   [.] bytes::bytes::Bytes::as_slice
     1.17%  libc.so.6                     [.] _int_free
     1.16%  tx_pipeline                   [.] core::core_arch::x86::sse2::_mm_movemask_epi8
     1.13%  libc.so.6                     [.] cfree
     1.01%  libc.so.6                     [.] _int_malloc
     0.96%  tx_pipeline                   [.] <A as SliceOrd>::compare
     0.94%  tx_pipeline                   [.] core::sync::atomic::fence
     0.89%  tx_pipeline                   [.] <dashmap::lock::RawRwLock as RawRwLock>::lock_shared
     0.86%  libc.so.6                     [.] malloc
     0.79%  tx_pipeline                   [.] core::core_arch::x86::sse2::_mm_set1_epi8
     0.78%  tx_pipeline                   [.] bytes::fmt::debug::...::BytesRef::fmt
     0.78%  tx_pipeline                   [.] scc::tree_index::leaf::Leaf::min_greater_equal
     0.68%  tx_pipeline                   [.] alloc::alloc::Global::alloc_impl
     0.67%  tx_pipeline                   [.] TryFrom for [T; N]::try_from
     0.64%  libc.so.6                     [.] malloc_consolidate
     0.61%  libc.so.6                     [.] unlink_chunk.isra.0
     0.57%  tx_pipeline                   [.] scc::tree_index::node::Node::insert
     0.52%  tx_pipeline                   [.] alloc::alloc::exchange_malloc
     0.51%  tx_pipeline                   [.] hashbrown::raw::inner::RawIterRange::new
     0.46%  tx_pipeline                   [.] _mm_loadu_si128
     0.46%  tx_pipeline                   [.] _mm_movemask_epi8  (другой call-site)
     0.41%  tx_pipeline                   [.] alloc::sync::Arc::new
     0.40%  tx_pipeline                   [.] fxhash::write64
     0.38%  tx_pipeline                   [.] scc::tree_index::internal_node::InternalNode::insert
```

---

## 3. Группировка и интерпретация

### Главный сюрприз: **НЕ heap, а concurrency**

Гипотеза перед прогоном (на основе libc-only профиля без символов): write hot-path
**alloc-bound** (~11% memory). Реальная картина — alloc даёт ~10%, а в 2 раза
больше времени уходит на **синхронизацию**:

| # | Категория | % суммарно | Главные символы |
|---|---|---|---|
| 🔴 **1** | **Atomics** (ref-count + counters + CAS) | **~12.4%** | atomic_load 3.38 + compare_exchange_weak 2.40 + fetch_add 2.14 + fetch_sub (×2) 3.57 + fence 0.94 |
| 🔴 **2** | **DashMap iteration** | **~6.2%** | Iter::next 3.20 + RawTableInner::iter 1.62 + lock_shared 0.89 + RawIterRange::new 0.51 |
| 🟡 **3** | **memcmp** (libc, бо́льшая часть → scc::TreeIndex compare) | **7.70%** | __memcmp_evex_movbe 7.70 + SliceOrd::compare 0.96 |
| 🟡 **4** | **Arc/Weak Drop** | **~4.1%** | Weak::drop 2.26 + Arc::drop 1.80 |
| 🟡 **5** | **scc::TreeIndex** (sorted index nav) | **~3.3%** | Node::search_entry 1.53 + Leaf::min_greater_equal 0.78 + Node::insert 0.57 + InternalNode::insert 0.38 |
| 🟡 **6** | **malloc/free family** | **~6.6%** | _int_free 1.17 + cfree 1.13 + _int_malloc 1.01 + malloc 0.86 + malloc_consolidate 0.64 + unlink_chunk 0.61 + Global::alloc_impl 0.68 + exchange_malloc 0.52 |
| 🟡 **7** | **memmove** (Vec realloc / Bytes copy / msgpack encode) | **3.22%** | __memmove_evex_unaligned_erms |
| 🟢 | hashbrown SIMD probing (SwissTable) — норма | ~2.9% | _mm_movemask_epi8 + _mm_set1_epi8 + _mm_loadu_si128 |
| 🟢 | fxhash::write64 | 0.40% | THasher — дёшево как и должно быть |
| 🟢 | Range::next (hot for-loops) | 1.36% | где-то много `for i in 0..N` |

**Сводка:** ~22% на синхронизацию и concurrent-data-structures (#1+#2+#4+#5) vs
~10% на heap (#6+#7). Перед патчем `with_capacity` массово — стоит сначала
закрыть концурренси.

### 3.1 Что пахнет особенно плохо

- **DashMap::iter (3.20%) в hot-path** — это **anti-pattern**. `DashMap` по design
  для индексированного доступа (`.get`, `.insert`), не для итерации; `.iter()`
  берёт shared-lock на **всех** шардах. Если это в горячем цикле — заведомо
  ускоряемо.

- **2×AtomicUsize::fetch_sub (3.57%)** — это, скорее всего, **rc-decrement при
  drop'е Arc и Weak**. Подтверждается соседними `Weak::drop 2.26%` + `Arc::drop
  1.80%`. Похоже на множество `ArcSwap::load_full()` per record.

- **scc::TreeIndex search 1.53% + memcmp 7.7%** — sorted index lookup доминирует
  `memcmp`. Если key — `Vec<u8>` (encoded RecordId/field bytes), сравнение в
  разы дороже чем `u64`-key.

---

## 4. Actionable targets (ранжированы по `expected gain × confidence × risk`)

| # | Target | % потолок | Confidence | Risk | Task |
|---|---|---|---|---|---|
| **A** | Сократить `Arc::clone` / `ArcSwap::load_full` в write hot-path | ~16.5% (atomics 12.4 + Arc/Weak Drop 4.1) | High — load_full() prograшно в `run_validators_qv` per call, гарантированно горячо | Low — `load()` Guard вместо `load_full()` Arc в местах где Arc не нужен outside scope | **#289** |
| **B** | Удалить `DashMap::iter()` из hot-path | ~6.2% | High — anti-pattern в hot-path | Low-Med — снэпшот через `ArcSwap` или per-shard scan | **#290** |
| **C** | Сократить key-compare в `scc::TreeIndex` (memcmp 7.7%) | ~5-7% | Med — нужно знать, что за key | High — структурное изменение индекса | (потенциальная #291) |
| **D** | `with_capacity()` на горячих местах (malloc + memmove) | ~5-8% | Low до прогона `capacity-telemetry` | Low | После **#288** |
| **E** | Hot `for 0..N` loop (1.36% Range::next) | ~1-1.5% | Low — call-site неизвестен | Low | После #289/#290 |

### Реалистичный план «двух волн»

**Волна 1 — концурренси** (#289 → #290):
1. Прибить точечно все `load_full()` в `run_validators_qv` / `run_validators_loop`
   на `load()`. Замерить.
2. Найти и убрать `DashMap::iter()` в hot-path. Замерить.
3. Cumulative потолок ~22%; реальная экономия (по опыту) обычно 30-60% от потолка,
   т.е. ~7-13% на оба патча.

**Волна 2 — память** (#288 → analysis → patches):
4. Реализовать capacity-telemetry (`docs/dev-artifacts/design/capacity-telemetry.md`).
5. Инструментировать топ-N аллокаторов по фламграфу (callgraph → конкретные
   call-sites).
6. Прогнать `--features capacity-telemetry`, получить точные peak'и.
7. Поставить data-driven `with_capacity(peak)`. Потолок ~6-7%, реальный
   выигрыш ~2-4%.

**Суммарный realistic target по результатам обеих волн: 10-17% throughput
улучшения на write hot-path.** Замерить criterion compare.

---

## 5. Caveat'ы / границы валидности

1. **Bench-profile НЕ release-profile.** `[profile.bench]` в `Cargo.toml`
   inherits release, но `opt-level=0` (комментарий: «для итеративной /opti,
   измерения 2-5× pessimistic vs opt-3»). Абсолютные % могут немного сдвинуться
   в opt-3, но **относительная картина горячих точек устойчива** — concurrency
   primitives и DashMap iter не уйдут от opt-3.

2. **Один бенч.** Профиль с `indexed/tx/1000` — это «батч из 1000 строк в
   индексированную таблицу под tx». Может НЕ покрыть:
   - read hot-path (read_planner / filter eval) — нужен отдельный flamegraph
     на `read_path_matrix`/`filter_eval`.
   - non-indexed insert — `tx_overhead/batch_pipeline/tx/100` без `indexed/`.
   - sub-batch + `$query` cross-ref — `engine_perf`.

   Перед широкими структурными изменениями стоит прогнать минимум 3 разных
   бенча (write+read+batch) и сравнить картину.

3. **3741 сэмплов — статистически грубо.** Сэмпл с долей <0.3% не доверять.
   Топ-15 — solid; xвост — индикативно.

4. **WSL2 perf — это всё-таки WSL2.** `--call-graph dwarf,4096` упрощённый,
   возможны пропущенные кадры. `--no-inline` не резолвит inline-функции (часть
   времени может быть приписана внешней функции). Полноценный профиль на
   Linux-host или Windows ETW дал бы другие нюансы. Но **горячие точки в
   `tx_pipeline` совпадают** между прогонами.

---

## 6. Артефакты

- `.flamegraphs/shamir-engine-tx_pipeline-symbols.svg` — кликабельный SVG с
  Rust-символами.
- `.flamegraphs/shamir-engine-tx_pipeline-bench.svg` — старый прогон без
  символов (можно удалить — историческое).
- `.flamegraphs/shamir-engine-lib.svg` — первый прогон по lib-тестам
  (бесполезен для горячих путей — доминирует test-обвязка, как пользователь
  правильно заметил).
- `scripts/wsl-flame-bench.sh` — параметризованный bench-flamegraph с фиксами.
- `scripts/wsl-perf-report.sh` — извлечение плоского self-time из perf.data.
- `docs/dev-artifacts/design/capacity-telemetry.md` — дизайн capacity-telemetry для волны 2.

---

## 7. Следующие шаги

1. ✅ **#290** — реализован коммитом `c463eb3b` (snapshot unique-defs раз на батч,
   устраняет 2×N DashMap-iter). Post-fix flamegraph — см. §8 ниже.
2. **#289** — `validator_bindings.load_full()` per-record; AtomicUsize-mirror
   для fast-path при пустых bindings.
3. Полный criterion compare (baseline vs after-both-fixes) после #289.
4. Потом — **#288** реализовать, инструментировать, наполнить data-driven
   `with_capacity()`.

---

## 8. After #290 — сравнение профилей (2026-06-28)

**Прогон:** `tx_overhead/batch_pipeline/indexed/tx/1000`, 30s, 3552 сэмплов
(до фикса 3741). SVG: `.flamegraphs/shamir-engine-tx_pipeline-symbols-post-290.svg`.

| Symbol | До #290 | После #290 | Δ | Заметка |
|---|---|---|---|---|
| **Заметно упало:** | | | | |
| `core::sync::atomic::atomic_load` (главный call-site) | 3.38% | 2.13% | **−1.25** | + другой site +0.48 = net −0.77% |
| `Weak::drop` | 2.26% | 1.82% | **−0.44** | меньше Arc-clone в hot path |
| `AtomicUsize::fetch_add` | 2.14% | 1.57% | **−0.57** | rc-increment'ов на def-clone |
| **memmove** | 3.22% | 2.82% | **−0.40** | Vec realloc меньше |
| **malloc/free family** (cum) | ~6.7% | ~5.6% | **−1.1** | def.paths.clone() убраны |
| `scc::Node::search_entry` | 1.53% | 1.30% | −0.23 | |
| **Не сдвинулось / сюрприз:** | | | | |
| `dashmap::Iter::next` | 3.20% | **3.38%** | +0.18 | ⚠ ДРУГОЙ источник DashMap-iter |
| `dashmap::lock_shared` | 0.89% | 1.05% | +0.16 | вслед за Iter::next |
| `memcmp` (libc) | 7.70% | 7.54% | ~ | scc::TreeIndex compare |
| `Arc::drop` | 1.80% | 1.85% | ~ | |
| `fetch_sub` (×2 sites) | 3.57% | 3.69% | ~ | |

**Cumulative подвинулось: ~−3.8% (net heap + atomics).** Wall-clock criterion
не мерял (требует ещё одного полного прогона); будет после #289.

### 8.1 Сюрприз — `dashmap::Iter::next` не упал

Мой фикс точно убрал 2×N DashMap-iter в unique-path (доказано чтением кода
+ зелёным гейтом). Но `Iter::next` остался на 3.38% → **есть другой DashMap,
итерируемый в hot path**, который доминирует. Скоринг кандидатов после грепа:

- `crates/shamir-storage/src/storage_membuffer.rs:350` — `state.dirty.iter().take(batch_size)`
  в `flush_dirty_batch` — **главный подозреваемый**. `dirty: DashMap<RecordKey, Slot>`
  собирается во время writes, периодически flush'ится в background. Под нагрузкой
  insert 1000 строк × несколько итераций бенча — flush работает интенсивно.
- `crates/shamir-index/src/legacy/index_manager.rs:430` — `for def in self.indexes.iter()`
  в `plan_records_created_batch` — **раз на батч**, должно быть дёшево.
  В принципе ОК.
- `crates/shamir-storage/src/storage_in_memory.rs:54` — `self.stores.iter()` —
  DDL-уровень, не hot.

**Кандидат №1 для следующей таски (после #289):** заменить `dirty.iter().take(batch_size)`
в membuffer на ленивый drain через `dirty.shards_iter()` или вообще на channel-based
flush queue. Это вторая волна.

---

## 9. After #289 — пересмотр гипотезы об остаточном `Iter::next`

**Прогон:** тот же бенч, 3448 сэмплов. SVG: `.flamegraphs/shamir-engine-tx_pipeline-symbols-post-289.svg`.

**Δ#289** (effekt только #289 поверх #290): практически нулевой.
- `Iter::next 3.38 → 3.11` (-0.27), `Arc::Drop 1.85 → 1.63` (-0.22),
  `memcmp 7.54 → 6.91` (-0.63) — слабые улучшения в шуме.
- `memmove +0.63`, `atomic_load +0.45`, `Weak::Drop +0.34` — встречный шум.
- Net ≈ 0.

**Объяснение:** в `tx_pipeline` бенче `validator_registry` скорее всего **None**
(бенч валидаторы вообще не настраивает). Мой fast-skip в `run_validators_qv`
не достигается — ранний выход уже срабатывает на `validator_registry == None`
(стр.127). Фикс корректен и нужен (low-risk, инвариант), но **выигрыш будет
на бенче с `Some(registry)` + пустыми bindings** — типовом prod-сценарии
пользовательской таблицы без валидаторов. Не вредит — не помогает на этом бенче.

### 9.1 Сюрприз №2 — остаточный `Iter::next` не из membuffer

В trace post-#289 виден `drop_in_place<Option<(Arc<RwLockReadGuard<...>>,
RawIter<(u64, SharedValue<shamir_index::legacy::index_definition::IndexDefinition>)>)>>`
0.44% — тип значения `IndexDefinition`, не `Slot`. Значит **остаточный `Iter::next`
3.11% — НЕ membuffer dirty, а опять shamir-index `DashMap<u64, IndexDefinition>`** (вариант
`indexes`/`indexes_unique`/`sorted_indexes::indexes`).

**Кандидат #291 пересматривается** — membuffer flush, возможно, всё ещё hot, но
не главный. **Главный реальный target** — заменить read-mostly DashMap'ы
`IndexManager::indexes` / `IndexManager::indexes_unique` / `SortedIndexManager::indexes`
на `ArcSwap<Arc<Vec<IndexDefinition>>>` (как уже сделано для `validator_bindings`).

Расчёт стоимости: каждый `iter()` берёт lock_shared на **всех 16-32 шардах**.
В бенче ~50 batches × 3 iter-вызова на батч (regular + unique + мой snapshot)
× 32 шарда = ~4800 lock-acquire/release за 30s → достаточно для 3.11% профиля.

ArcSwap-read = atomic-incr + Arc-clone, **на порядок дешевле**. И index-defs
read-mostly (мутируются только DDL — `create_index`/`drop_index`), идеальный
кейс для ArcSwap (как `validator_bindings`).

→ Новая таска **#292** (ArcSwap для index DashMap). Скоуп M (3 DashMap'а в
`shamir-index`, эхо в caller'ах). Ожидаемый выигрыш ~2-3.5%.

### 8.2 Не считать фикс провалом

Sample-size относительно мал (3552), индивидуальные символы шумят ±2-3%.
Cumulative shift в правильную сторону по 6 категориям — реальный сигнал. И главное —
**#290 был обязательным шагом** (правильный фикс anti-pattern'а), даже если в
конкретном бенче доля unique-iter была меньше, чем доля membuffer-iter. На бенче
без membuffer (или с большим batch_size flush'а) выигрыш был бы больший.

---

## 10. #291 closed — membuffer был ложным кандидатом (исследование)

**Дата:** 2026-06-28 (после #292). Чисто статическое исследование по коду,
без нового flamegraph (доказательство не требует прогона).

### 10.1 Вопрос

Горяч ли `storage_membuffer.rs:350` `dirty.iter().take(batch_size)` —
кандидат №1 на остаточный `dashmap::Iter::next` 3.38/3.11% (§8.1, §9.1)?

### 10.2 Ответ: НЕТ — membuffer физически не на пути профилированного бенча

`tx_pipeline indexed/tx/1000` (на котором снимались ВСЕ три flamegraph'а):
- `make_repo()` → `InMemoryRepo::new()` напрямую (`tx_pipeline.rs:45-46`).
  **Без MemBuffer-обёртки.** MemBuffer применяется только когда table
  config несёт `buffer_config` (`repo/repo_types.rs:64`
  `MemBufferStore::new(...)`); bench его не задаёт.
- Индексы бенча (`tx_pipeline.rs:227-229`): `create_unique_index` +
  `create_index` — обычные (IndexInfo). **Sorted-index НЕ создаётся.**

Следствие: `MemBufferStore::drain_once` (и его `dirty.iter().take(...)`)
**не вызывается ни разу** во время этого бенча. Символ не мог появиться
в трейсе. #291-кандидат — артефакт грепа «найди все `DashMap::iter` в
hot-ish коде», а не профиля.

### 10.3 Что было реальной причиной — подтверждено эмпирически

Остаточный `Iter::next` был **IndexInfo DashMap** (regular + unique),
как и предсказано в §9.1 (тип значения в трейсе = `IndexDefinition`, не
`Slot`). **#292** заменил его на `ArcSwap<Vec<IndexDefinition>>`:

```
tx_pipeline indexed/tx/1000 (criterion --baseline, BENCH_FULL):
  before (DashMap):     132.56 ms
  after  (ArcSwap+rcu):  81.07 ms   → −38.84%, 1.63×, p=0.00
```

Если бы доминантой был membuffer, #292 не дал бы такого сдвига. Выигрыш
−38.84% на замене именно IndexInfo доказывает, что он и был горячим
DashMap'ом. **#291 закрывается как not-the-cause.**

### 10.4 Два живых вывода на будущее (НЕ в этом бенче)

**A. `SortedIndexManager::indexes` — тот же anti-pattern, что #292 пофиксил,
но НЕ тронут.** `sorted_index_manager.rs:52` —
`Arc<DashMap<u64, SortedIndexDefinition, THasher>>`, read-mostly
(мутируется только DDL `register`/`drop`). На write hot-path:
`table_manager_tx_ops.rs:183/203/229/251/487/645` → `plan_record_*` →
`iter_indexes()` (`sorted_index_manager.rs:91` `self.indexes.iter()...collect()`)
на КАЖДЫЙ план. Защищён `is_empty()` ранним выходом (`:249`,`:278`), поэтому
в `indexed/tx/1000` (без sorted-index) не активен. **В prod-сценарии с
sorted/covering-index — следующий прямой ArcSwap-кандидат** (зеркало #292:
`ArcSwap<Vec<SortedIndexDefinition>>`, `iter_indexes` → `load()` snapshot,
`register`/`drop` → `rcu()` COW). Ожидаемый профиль аналога #292.

**B. Membuffer `dirty.iter().take(batch_size)` — реальный anti-pattern,
горячий ТОЛЬКО под membuffer write-нагрузкой** (таблица с `buffer_config`).
Неэффективности самого паттерна:
- `DashMap::iter()` берёт read-lock пошардно → contention с конкурентными
  `dirty.insert` (каждый write notify'ит flusher → `drain_once`).
- `.take(batch_size)` всегда начинает с head-of-shard-0 → неравномерный
  дренаж (поздние шарды голодают под устойчивой нагрузкой; eventual,
  не корректностный баг).
- двойной `dirty.is_empty()` (`:344`,`:387`) — каждый O(шарды).
Чтобы измерить — нужен membuffer-specific bench (`membuffer_pump` /
`membuffer_concurrent` / `durable_concurrent_commit`), НЕ `tx_pipeline`.
Альтернатива дренажа: FIFO-очередь dirty-ключей (`SegQueue<RecordKey>`)
параллельно карте-дедупу → pop без iter/shard-lock. Отдельная задача,
со своим baseline на membuffer-бенче. **Не блокирует ничего; trade-off
сложности (double-bookkeeping) против выигрыша требует профиля сначала.**

### 10.5 Итог

- #291 (membuffer как причина остаточного 3.38%) — **закрыта, ложный след**.
- Реальная причина (IndexInfo DashMap) — **#292, пофикшена, −38.84%**.
- Производные кандидаты: **A** (SortedIndexManager ArcSwap — прод-сценарий
  с sorted-index) и **B** (membuffer drain-очередь — membuffer-нагрузка).
  Оба со своим baseline'ом, ни один не на `tx_pipeline indexed/tx/1000`.

---

## 11. Исследование производных кандидатов #304 / #305

**Дата:** 2026-06-29. Статический анализ кода + bench-инвентаризация.
Цель — оценить форму фикса, ROI и предусловия для каждого, ДО /opti.

### 11.1 #304 — SortedIndexManager::indexes → ArcSwap

**Форма фикса = зеркало #292 + одна доп. сложность.**

Use-sites `self.indexes` (`sorted_index_manager.rs`):

| Метод | Строка | Тип | Hot? |
|---|---|---|---|
| `is_empty()` | 77, 216, 249, 278, 334, 408 | read | ⚡ ранний выход на КАЖДОМ plan_* |
| `iter()...collect()` (iter_indexes) | 91 | read | ⚡ на каждом plan_* + persist |
| `get()` (find_by_name_interned) | 106 | read | read-path |
| `iter().find()` (find_by_field) | 98 | read | read-path |
| `iter().any()` (has_covering_indexes) | 84 | read | bootstrap |
| `insert()` | 113, 165, 860 | write | DDL/load |
| `remove()` | 120, 162 | write | DDL |
| **`alter()`** | **184** | **write-in-place** | **intern_included_paths** |

Reads → `load()` snapshot; writes (insert/remove) → `rcu()` COW. Всё как #292.

**Доп. сложность vs #292:** `intern_included_paths` (:184) использует
`DashMap::alter` — **in-place мутацию значения** (переписывает
`included_fields_interned` существующего def). В ArcSwap-Vec модели →
`rcu(|cur| { clone, для каждого def с непустым included_fields пересчитать,
вернуть новый Vec })`. Не на hot path (вызывается после `register` с
interner ИЛИ после `load` на bootstrap), поэтому COW-стоимость приемлема.
IndexInfo (#292) этого метода не имел — единственное отличие портирования.

**⚠ ROI ниже #292 — две причины:**

1. **Batch-путь уже амортизирован.** `plan_records_created_batch` (:240)
   снапшотит `iter_indexes()` **ОДИН раз на весь батч** (явный комментарий
   :237 "snapshotting iter_indexes() ONCE"). Так что tx batch-insert
   (главный write hot-path) дёргает sorted-iter раз на батч, не на запись.
   Sorted-iter горяч только на **singular** `plan_record_created` (:219) и
   на `plan_record_updated`/`plan_record_deleted` (:281,:337 — singular, раз
   на запись) → горяч на **update/delete-heavy** sorted-нагрузке.
2. **Одна карта vs две-три у #292.** #292 убирал regular+unique (+ unique
   итерировался отдельно в crud/tx_ops для валидации) — 2-3 iter на батч.
   Sorted — одна карта. Линейный выигрыш меньше.

**⚠ НЕТ готового write-path baseline с sorted-index.** `read_path_matrix.rs`
создаёт sorted-index (`create_sorted_index` :128), но мерит **READ** shapes
(ORDER BY / WHERE range); insert — в setup, ВНЕ timed loop. `tx_pipeline
indexed/*` мерит write, но без sorted (только hash-index). **Для /opti #304
нужно сперва добавить sorted-вариант в `tx_pipeline`** (create_sorted_index
в setup + insert/update в timed loop) — иначе baseline нечем снять.

**Вывод #304:** делать стоит (anti-pattern, консистентность с #292,
устраняет shard-lock на sorted read-path), НО: (а) нужна bench-инвестиция,
(б) ROI скромнее #292, (в) +1 метод (`alter`→rcu). Микро-бонус: `iter_indexes`
может вернуть `Arc<Vec<...>>` (zero-clone) вместо `Vec` by-value, если
поменять сигнатуру — но это эхо по всем caller'ам (`for def in &defs`).

### 11.2 #305 — membuffer drain FIFO-очередь

**Baseline ЕСТЬ** (в отличие от #304):
- `membuffer_pump.rs` — конфиг `frequent_flush` (flush_interval_ms=10,
  flush_batch_size=64) реально гоняет background pump → `drain_once`.
- `membuffer_concurrent.rs` — concurrent reader+writer, batch=256.

**⚠ Три предостережения против немедленной FIFO-замены:**

1. **FIFO имеет дубликат-проблему.** Текущая `dirty: DashMap<RecordKey, Slot>`
   дедуплицирует по ключу (повторный write того же ключа → один entry,
   last-write-wins). FIFO-очередь ключей (`SegQueue<RecordKey>`) НЕ
   дедуплицирует: ключ записан N раз → N записей в очереди. Дренаж pop'ает K
   несколько раз; со 2-го раза `dirty[K]` уже снят (или новое значение) →
   нужна проверка-skip "ещё ли K в dirty с тем же snapshot". Это
   double-bookkeeping (очередь + карта) и доп. ветка в hot drain-loop.
2. **В ПРОДЕ выигрыш тонет в I/O.** `membuffer_pump` использует
   `InMemoryStore` как inner → `set_many`/`remove_many` почти бесплатны, и
   доля `dirty.iter()` в профиле велика. В проде inner = sled/fjall →
   `set_many` это реальный batched write + (на sealed) fsync, который
   **доминирует** над iter. Оптимизация iter, заметная в in-memory bench,
   будет мала в проде. Профилировать надо с РЕАЛЬНЫМ inner, чтобы не
   оптимизировать то, что в проде не главное.
3. **Не подтверждено профилем.** §10 закрыл membuffer как НЕ горячий в
   tx_pipeline. Здесь нагрузка другая (membuffer-specific), но горяч ли
   именно `iter().take()` — а не `set_many`/`remove_if`-cleanup — НЕ
   измерено. Нужен flamegraph `membuffer_pump/frequent_flush` (in-memory)
   И с sled-inner, прежде чем менять структуру.

**Дешёвый промежуточный win (без структурных изменений):** двойной
`dirty.is_empty()` (:344 вход + :387 после дренажа) — каждый O(шарды).
Первый можно заменить на `dirty_nonempty.load(Acquire)` (атомик-флаг уже
есть, :162), второй оставить как стабильную post-drain проверку. Микро, но
бесплатно и безопасно.

**Вывод #305:** НЕ начинать с FIFO. Порядок: (1) flamegraph
`membuffer_pump/frequent_flush` + вариант с sled-inner → подтвердить долю
`iter().take()` vs `set_many`; (2) если iter подтверждён горячим —
дешёвый is_empty()-fix первым; (3) FIFO/SegQueue только если профиль
оправдывает trade-off сложности. Иначе риск оптимизировать in-memory
артефакт, невидимый в проде.

### 11.3 Сводный приоритет

| | baseline готов | форма фикса | ROI | предусловие |
|---|---|---|---|---|
| **#304** | ✗ (нужен sorted-вариант в tx_pipeline) | известна (зеркало #292 + alter→rcu) | скромный (batch амортизирован, 1 карта) | добавить bench |
| **#305** | ✓ (membuffer_pump/concurrent) | СПОРНА (FIFO дубликаты) | неясен (в проде тонет в I/O) | профиль СНАЧАЛА |

**Рекомендация:** ни один не «бери и делай». #304 — чище по форме, но нужен
bench и ROI скромный. #305 — есть bench, но нужен профиль и FIFO спорна.
Если двигаться — **#304 первым** (известный доказанный паттерн), добавив
sorted-вариант в tx_pipeline; **#305 — только после flamegraph** с РЕАЛЬНЫМ
inner, начав с дешёвого is_empty()-fix. Ни один не блокирует основную
кампанию; оба — «вторая волна» точечных шлифовок.

---

## 12. #305 closed by profile — drain НЕ hot path

**Дата:** 2026-06-29. Практический профиль (WSL perf, dwarf,4096 stack,
99Hz × 15s, 5265 samples). SVG: `.flamegraphs/membuffer-pump-frequent-flush-2026-06-29.svg`.

**Bench:** `membuffer_pump/insert_single/frequent_flush` + `get_single/frequent_flush`
(flush_interval_ms=10, flush_batch_size=64 — самый агрессивный drain-режим;
in-memory inner для чистоты атрибуции).

### 12.1 Drain-симоволы в self-time

| Symbol | self-time |
|---|---|
| `drain_once` (наш предполагаемый hot spot) | **0.07%** |
| `dashmap::_remove_if` (CAS-cleanup) | 0.14% |
| `dashmap::RawRwLock::lock_shared` | 0.11% |
| `dashmap::DashMap::insert` | 0.07% |
| `dashmap::_entry` | 0.08% |
| **Σ всё drain + DashMap** | **~0.5%** |

Для сравнения: #292 (IndexInfo DashMap) был **3.11%** на tx_pipeline →
**#305 в ~30 раз меньше**. Полностью в шуме измерения.

### 12.2 Где реально время

| Категория | ~self-time |
|---|---|
| `memmove`/`memcmp` libc (Bytes copy/compare) | ~10% |
| `Arc::hash` + `AtomicUsize::fetch_sub` + `atomic_load` + sip-hash | ~12-15% |
| malloc/free/consolidate | ~5% |
| moka cache (`cht::map::bucket::*`, `scc::tree_index`) | ~5-10% |
| tokio scheduler / Range-iter / адаптеры | ~5% |

Доминанта — **moka cache + Arc/Bytes hash-операции на каждый write/get**,
не drain. Это естественная цена hashmap-операций под этой нагрузкой;
снижать её — другая задача (заменить moka на что-то легче, или убрать
Arc-обёртку вокруг ключа), и она НЕ относится к `dirty.iter().take()`.

### 12.3 Что насчёт sled-inner (прод-сценарий)?

В §11.2 предполагалось: «в проде I/O доминирует над iter». Это **усиливает**
вывод #12.1, не ослабляет. С sled/fjall inner:
- `set_many` в drain становится ещё дороже (batched write + потенциальный
  fsync на rotation сегмента).
- Соотношение drain-iter / set_many сдвигается ЕЩЁ дальше от drain-iter.

Доп. measurement с sled-inner не выполнен, но направление однозначно:
если в самом «выгодном» для #305 сценарии (in-memory inner, frequent_flush)
drain-iter = 0.07%, в проде он будет ещё меньше.

### 12.4 Cheap is_empty-fix — был unsafe, остаётся unsafe

Анализ §11.2: `dirty.is_empty()` на line 344 серилизуется shard read-locks
с in-flight `dirty.insert` от writer'а. Замена на `dirty_nonempty.load(Acquire)`
не даёт этой синхронизации → foreground `drain_all` может выйти, пока
writer держит lock между `store(true)` и `insert(key, slot)` → потерянная
запись в `flush()` API. Constraint «многопоточная работа не должна
замедлиться» исключает этот fix. И ROI оказался бы 0.07% при идеальной
замене — не стоит риска.

### 12.5 Решение

**#305 закрывается как ложный кандидат — зеркало #291.** Профиль не
подтверждает membuffer drain как hot path даже в самом агрессивном
flush-режиме (10ms interval, in-memory inner — самый «дешёвый» inner,
максимизирующий относительную долю iter).

Реальный hot path membuffer'а — moka cache + Arc-hash/refcount — **другая
задача**, за рамками #305. Если в будущем понадобится — отдельный профиль
+ /opti цикл, начиная с подозрений на moka, а не на DashMap.

### 12.6 Итоговый счёт перф-кампании ②

| Задача | Результат |
|---|---|
| #289 validator_bindings AtomicUsize mirror | landed, neutral на этом бенче (бенч без валидаторов) |
| #290 unique-index snapshot per batch | landed, −3.8% cum |
| #292 IndexInfo DashMap → ArcSwap | landed, **−38.84% / 1.63×** на tx_pipeline |
| #303 Windows WAL TOCTOU race | landed (correctness fix) |
| #304 SortedIndexManager DashMap → ArcSwap | landed, multi-thread **−13.09% / +15% thrpt** |
| #291 membuffer iter (false candidate) | closed by static analysis (бенч не на пути) |
| #305 membuffer drain FIFO (false candidate) | closed by profile (0.07% self-time) |

Главные победы — #292 и #304 (оба ArcSwap для read-mostly registries);
оба сошлись на одной форме фикса, которую теперь стоит держать как
workspace-конвенцию (см. CLAUDE.md §3 «scc/DashMap for concurrent maps» →
дописать «ArcSwap<Vec<...>> для read-mostly с редкими mutations»).
