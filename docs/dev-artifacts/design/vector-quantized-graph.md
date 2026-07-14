# SQ8-квантизация в HNSW векторном индексе

**Лист:** #411 (V5.2)  
**Зависимость:** #410 (sq8.rs, simd.rs — примитивы)  
**Продолжение:** #412 (snapshot квант-параметров)

---

## 0. Почему НЕ Вариант C (f32-граф + u8-storage)

Вариант C отвергнут как бессмысленный:

1. **Память не экономится.** hnsw_rs хранит полный `Vec<T>` в каждом `DataPoint` внутри графа. При `Hnsw<f32, _>` — f32-копии в графе. Наша `vectors`-мапа (u8) — малая доля; граф доминирует. Итого: ~0% реальной экономии.
2. **Recall деградирует без выгоды.** Путь `f32->quantize->u8->dequantize->lossy f32->insert` = граф на lossy-векторах = потеря recall. При этом дистанция при traversal по-прежнему на f32 (медленно) и не экономит память. Худшее из миров.
3. **Rescoring невозможен.** Смысл квант-ANN: дешёвая дистанция на кодах (целочисленная, быстрая) + rescore. Вариант C не использует быструю целочисленную дистанцию вообще — только добавляет шум.

---

## 1. Решённые развилки

### 1.1 Тип графа — Вариант A: `Hnsw<'static, u8, ShamirDistU8>`

**Решение:** граф на u8-кодах с кастомной дистанцией.

**Feasibility:** спайк #393 (`hnsw_rs_contract_tests.rs:460`, тест `hnsw_i8_compiles_and_searches`) доказал: `Hnsw<'static, i8, I8L2>` с пользовательским `impl Distance<i8>` компилируется, вставляет и ищет. hnsw_rs 0.3.4 generic по `T: Clone + Send + Sync` и `Distance<T>` — `u8` подходит. Встроенные `DistL1`/`DistL2` покрывают `u8` через макрос. Кастомный `Distance<u8>` компилируется без проблем.

**Архитектура:**

```rust
/// Quantized distance function holding frozen quantizer params.
/// `eval(&[u8], &[u8]) -> f32` computes distance on codes using
/// precomputed per-dim weights from Sq8Quantizer.
#[derive(Clone)]
pub struct ShamirDistU8 {
    /// Frozen quantizer parameters (Arc — shared with adapter).
    params: Arc<Sq8Quantizer>,
    metric: VectorMetric,
}

impl Distance<u8> for ShamirDistU8 {
    fn eval(&self, a: &[u8], b: &[u8]) -> f32;
}
```

**Distance<u8>.eval() реализация по метрике:**

- **Dot:** `1.0 - approx_dot(a, b) / (norm_a * norm_b)` — approx_dot из #410 (SIMD `dot_u8` ядро + per-dim scale^2 * qi*qj + linear terms). Нормы предвычисляются на dequant или аппроксимируются.
- **Cosine:** аналогично Dot с нормализацией (dequant-based norms, precomputable per-vector и кешируемые).
- **L2:** `approx_l2_sq(a, b)` — аналог approx_dot: `sum_i (min_i + a_i*s_i - min_i - b_i*s_i)^2 = sum_i s_i^2 * (a_i - b_i)^2`. Целочисленное ядро `sum (a_i - b_i)^2` через SIMD (u8 diff squared sum), масштабирование per-dim `s_i^2`.

**Нюанс: params внутри Distance.** `Distance<u8>` — trait object в hnsw_rs, `eval` принимает `&self`. Держим `Arc<Sq8Quantizer>` в `ShamirDistU8` — при `Clone` (hnsw_rs клонирует Distance) Arc дешёв. Params замораживаются после fit, дальше read-only через Arc.

**Память:** hnsw_rs хранит `Vec<u8>` в DataPoint — 4x экономия vs f32. Наша `vectors`-мапа тоже u8. Итого: РЕАЛЬНАЯ 4x экономия на всём хранимом.

**Скорость:** целочисленная дистанция на u8 (`dot_u8` SIMD AVX2: 32 байта за такт) значительно быстрее f32 dot (8 float за такт AVX2). Traversal ускоряется ~2-3x.

### 1.2 Rescoring — dequant-based (без retained f32)

**Решение:** graph traversal с overscan возвращает top-`2k+10` кандидатов по approx-дистанции на кодах. Финальный top-k пересчитывается через `dequantize(codes) + точная ShamirDist(f32).eval()`.

**Обоснование:**
- Retain f32 = 0% экономии памяти (весь смысл квантизации потерян).
- #410 DoD: approx_dot recall@10 >= 0.98 — это БЕЗ rescore. С dequant-rescore recall ещё выше (dequant точнее, чем approx на кодах для ranking).
- Dequant cost: O(dim) scalar ops per candidate * `2k+10` кандидатов — ничтожно (~50us для k=10, dim=128).

**Rescore flow:**
1. `hnsw.search(&query_codes, overscan, ef)` — traversal на u8-кодах, дешёвая целочисленная дистанция.
2. Получаем `overscan` кандидатов (internal_ids + approx distances).
3. Для каждого кандидата: `dequantize(codes[id])` -> `ShamirDist(f32).eval(query_f32, dequant_vec)` -> точная дистанция.
4. Sort по точной дистанции, truncate to k.

**Query path:** query вектор (f32 от клиента) квантуется в u8 для graph traversal; оригинальный f32 query сохраняется для rescore.

### 1.3 Fit-тайминг квантайзера — deferred fit + rebuild

**Решение:** двухфазность с порогом:

1. **Pre-fit (< FIT_THRESHOLD=256):** адаптер работает в f32-режиме. `vectors_f32` хранит оригиналы. Brute-force path (BRUTE_FORCE_MAX=256 совпадает) — точный, 100% recall. Граф НЕ строится (brute-force и так используется для малых индексов).

2. **Fit trigger (next_id >= 256):** происходит при `upsert`/`upsert_batch` пересекающем порог:
   - `Sq8Quantizer::fit(&accumulated_f32_vectors, dim)` — вычисляет mins/scales.
   - Создаётся `ShamirDistU8 { params: Arc::new(quantizer), metric }`.
   - Создаётся НОВЫЙ `Hnsw<'static, u8, ShamirDistU8>` с тем же config.
   - Все 256 f32-векторов квантуются -> `parallel_insert` кодов в u8-граф.
   - `vectors_f32` дропается, `vectors_u8` заполняется кодами.
   - `AtomicBool is_fitted` = true.
   - Аналог rebuild-aside из #408: создание нового графа в spawn_blocking, затем atomic swap Arc.

3. **Post-fit:** все новые вставки: `quantize(vec) -> insert codes в u8-граф + store в vectors_u8`.

4. **Refit:** НЕ в #411. Params замораживаются. #412 добавит refit при snapshot/compaction rebuild (если распределение дрейфует).

### 1.4 Opt-in протягивание — `VectorConfig.quantization: Option<VectorQuantization>`

```rust
// crates/shamir-index/src/kind.rs
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VectorQuantization {
    Sq8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    pub dim: u32,
    pub metric: VectorMetric,
    pub backend: VectorBackendRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<VectorQuantization>,
}
```

**Wire (serde back-compat):** `#[serde(default, skip_serializing_if = "Option::is_none")]` — старые сообщения без поля = None = f32-путь бит-в-бит.

**DDL op:** `vector_quantization: Option<String>` (`"sq8"` | None).

**Rust-билдер:** `.vector_quantization("sq8")`.

**TS ddl.ts:** `opts.vector_quantization?: "sq8"`.

### 1.5 Взаимодействие с существующим

| Компонент | Поведение при quantization=Sq8 |
|---|---|
| **Brute-force (< 256, pre-fit)** | f32 as-is, точный. Квантизация ещё не активна. |
| **Delete/tombstone** | Без изменений: `deleted` set на internal ids. `vectors_u8.remove()` удаляет коды. |
| **Compaction (#408 rebuild-aside)** | `collect_live_vectors()` возвращает u8-коды. Rebuild target строит новый u8-граф из кодов (re-insert, не re-quantize — params frozen). Quantizer params копируются Arc::clone. |
| **Snapshot (#412)** | Сериализуются: quantizer params (mins, scales, dim) + все u8-коды + graph dump. Version byte: 0=f32, 1=sq8+u8. |
| **Staged tx-вектора** | f32 от клиента. При in-tx search: staged квантуются ad-hoc для scoring (quantize -> approx dist, или brute-force dequant + exact dist для малого staged set). НЕ вставляются в граф до promote. При promote (commit): quantize -> insert в u8-граф. |
| **search_prefilter (brute-force over candidates)** | Post-fit: dequant(codes) + exact dist. Или approx dist на кодах (быстрее, recall достаточен). |
| **search_cofilter (HNSW search_filter)** | Работает на u8-графе напрямую — `hnsw.search_filter(&query_codes, k, ef, Some(&pred))`. |

---

## 2. Изменения по уровням

### 2.1 `crates/shamir-index/src/kind.rs`

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VectorQuantization {
    Sq8,  // ordinal 0 — append-only
}

// VectorConfig: +quantization field
#[serde(default, skip_serializing_if = "Option::is_none")]
pub quantization: Option<VectorQuantization>,
```

### 2.2 `crates/shamir-index/src/vector/hnsw_adapter.rs`

Новая структура дистанции:

```rust
#[derive(Clone)]
pub struct ShamirDistU8 {
    params: Arc<Sq8Quantizer>,
    metric: VectorMetric,
}

impl Distance<u8> for ShamirDistU8 {
    fn eval(&self, a: &[u8], b: &[u8]) -> f32;
}
```

Новые/изменённые поля `HnswAdapter`:

```rust
pub struct HnswAdapter {
    dim: u32,
    metric: VectorMetric,
    ef_search: usize,
    quantization: Option<VectorQuantization>,

    // === f32 path (unquantized OR pre-fit) ===
    hnsw_f32: Option<Arc<Hnsw<'static, f32, ShamirDist>>>,
    vectors_f32: scc::HashMap<usize, Vec<f32>, THasher>,

    // === u8 path (post-fit, quantized) ===
    hnsw_u8: Option<Arc<Hnsw<'static, u8, ShamirDistU8>>>,
    vectors_u8: scc::HashMap<usize, Vec<u8>, THasher>,
    quantizer: Option<Arc<Sq8Quantizer>>,
    is_fitted: AtomicBool,

    // === shared ===
    rid_map: scc::HashMap<usize, RecordId, THasher>,
    rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
    deleted: scc::HashMap<usize, (), THasher>,
    deleted_count: AtomicUsize,
    next_id: AtomicUsize,
    compaction_deleted_rids: Option<Arc<scc::HashMap<RecordId, (), THasher>>>,
}
```

Ключевые сигнатуры:

```rust
impl HnswAdapter {
    pub fn new(dim: u32, metric: VectorMetric, config: HnswConfig, quantization: Option<VectorQuantization>) -> Self;

    /// Attempt fit when threshold reached. Builds u8 graph, migrates vectors.
    async fn try_fit_and_rebuild(&self) -> Result<(), VectorError>;

    /// Quantize f32 -> u8 using frozen quantizer. Panics if not fitted.
    fn quantize(&self, vec: &[f32]) -> Vec<u8>;

    /// Dequantize u8 -> f32 for rescore. Panics if not fitted.
    fn dequantize(&self, codes: &[u8]) -> Vec<f32>;

    /// Rescore candidates: dequant + exact f32 distance.
    fn rescore(&self, query_f32: &[f32], candidates: &[(usize, f32)]) -> Vec<(usize, f32)>;

    // Snapshot accessors:
    pub(crate) fn quantizer(&self) -> Option<&Arc<Sq8Quantizer>>;
    pub(crate) fn is_quantized(&self) -> bool;
    pub(crate) fn for_each_vector_u8<F: FnMut(usize, &[u8])>(&self, f: F);
    pub(crate) fn hnsw_u8_handle(&self) -> Option<&Arc<Hnsw<'static, u8, ShamirDistU8>>>;

    // Extended from_parts for quantized snapshot load:
    pub(crate) fn from_parts_quantized(
        dim: u32, metric: VectorMetric, ef_search: usize,
        hnsw_u8: Arc<Hnsw<'static, u8, ShamirDistU8>>,
        quantizer: Arc<Sq8Quantizer>,
        rid_map: scc::HashMap<usize, RecordId, THasher>,
        rid_to_internal: scc::HashMap<RecordId, usize, THasher>,
        vectors_u8: scc::HashMap<usize, Vec<u8>, THasher>,
        deleted: scc::HashMap<usize, (), THasher>,
        next_id: usize,
    ) -> Self;
}
```

### 2.3 DDL (`table_manager_index_mgmt.rs`)

```rust
let quantization = match op.vector_quantization.as_deref() {
    Some("sq8") => Some(VectorQuantization::Sq8),
    _ => None,
};
// -> VectorConfig { dim, metric, backend, quantization }
// -> HnswAdapter::new(dim, metric, config, quantization)
```

### 2.4 Wire (serde)

- `VectorConfig.quantization`: optional, default None. Old messages parse fine.
- Create-index op: `vector_quantization: Option<String>`, default None.

### 2.5 Rust-билдер

```rust
pub fn vector_quantization(mut self, q: &str) -> Self {
    self.vector_quantization = Some(q.to_string());
    self
}
```

### 2.6 TS ddl.ts

```typescript
interface CreateVectorIndexOpts {
    vector_dim?: number;
    vector_metric?: "l2" | "cosine" | "dot";
    vector_quantization?: "sq8";
}
```

---

## 3. Фазовый план реализации (#411)

### Фаза 1: Rust-core

**Тронутые файлы:**
- `crates/shamir-index/src/kind.rs` — VectorQuantization enum, поле в VectorConfig
- `crates/shamir-index/src/vector/hnsw_adapter.rs` — ShamirDistU8, dual graph (f32/u8), fit/rebuild, rescore, все методы VectorAdapter
- `crates/shamir-index/src/vector/sq8.rs` — добавить `approx_l2_sq()` и `approx_cosine()` (аналоги approx_dot для L2/Cosine метрик)
- `crates/shamir-index/src/vector/simd.rs` — `diff_sq_sum_u8()` SIMD ядро для L2 на u8-кодах
- `crates/shamir-query-types/src/...` — поле `vector_quantization` в create-index op
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` — парсинг op.vector_quantization
- `crates/shamir-query-builder/src/...` — .vector_quantization() метод

**Тесты:**
- `serde_back_compat_vector_config` — VectorConfig без quantization -> None
- `serde_back_compat_create_op` — create-index op без vector_quantization -> None
- `opt_in_disabled_is_f32` — без quantization = текущее f32 поведение бит-в-бит
- `recall_sq8_vs_f32` — 1024 random 128-d cosine top-10: recall@10 >= 0.98
- `fit_transition` — pre-fit brute-force (f32), post-fit u8-граф + rescore
- `delete_quantized` — tombstone на u8-графе работает
- `upsert_replace_quantized` — replace вектора корректно перетомбстоунит
- `staged_not_in_graph` — in-tx staged = f32, merged correctly at search
- `distance_u8_matches_approx` — ShamirDistU8.eval() согласуется с approx_dot/approx_l2
- `ddl_roundtrip_sq8` — create index sq8 -> VectorConfig.quantization == Some(Sq8)

**DoD:**
- `./scripts/test.sh @oracle @types` зелёный
- `cargo clippy --workspace --all-targets -- -D warnings` чисто
- Неквантованные индексы работают бит-в-бит как до #411 (регрессионный тест)

### Фаза 2: TS

**Тронутые файлы:**
- `crates/shamir-client-ts/src/core/builders/ddl.ts` — vector_quantization в opts
- `crates/shamir-client-ts/src/core/builders/__tests__/vector_filter_parity.test.ts` — parity

**Тесты:**
- vitest: create vector index с `vector_quantization: "sq8"` -> op payload содержит поле
- vitest: create vector index БЕЗ quantization -> op payload НЕ содержит поле
- parity: Rust == TS сериализация create-index op с quantization

**DoD:**
- vitest pass
- Parity Rust<->TS

---

## 4. Тесты (summary)

| Тест | Что проверяет |
|---|---|
| `recall_sq8_vs_f32` | 1024 random 128-d cosine top-10: recall@10 >= 0.98 |
| `opt_in_disabled_is_f32` | Без quantization -> f32-граф, vectors_f32 populated |
| `serde_back_compat_vector_config` | VectorConfig без `quantization` -> None |
| `serde_back_compat_create_op` | Create-index op без `vector_quantization` -> None |
| `ddl_roundtrip_sq8` | Create vector index sq8 -> VectorConfig.quantization == Some(Sq8) |
| `fit_transition` | Pre-fit=brute-force f32; post-fit=u8-граф + rescore |
| `delete_quantized` | Tombstone на quantized index работает |
| `staged_not_in_graph` | In-tx staged = f32, search merges correctly |
| `distance_u8_metrics` | ShamirDistU8 для L2/Cosine/Dot корректна |
| `parity_ts_rust_sq8_op` | TS и Rust op payload идентичны |

---

## 5. Memory/Recall trade-off (честная оценка)

| | f32 (текущее) | SQ8 u8-граф (Вариант A) |
|---|---|---|
| Память на 1M 128-d | ~512MB (граф) + ~512MB (vectors) = ~1GB | ~128MB (граф) + ~128MB (vectors) = ~256MB |
| Экономия | baseline | **4x** |
| Graph traversal speed | f32 SIMD (8 lanes AVX2) | u8 integer (32 lanes AVX2) — **~2-3x faster** |
| Recall@10 (graph only, no rescore) | 0.95-0.99 (HNSW approx) | 0.93-0.97 (approx distance on codes) |
| Recall@10 (with dequant-rescore) | same | **0.97-0.99** (rescore recovers ~2%) |
| Fit overhead | none | one-time O(N*dim) at threshold crossing |
| Insertions post-fit | O(1) amortized | O(1) + quantize (O(dim) scalar) |

**Вывод:** 4x память + 2-3x скорость traversal при recall loss < 2% (с rescore). Единственная цена — одноразовый rebuild при fit (256 vectors, <1ms).

---

## 6. Риски и острые углы для impl-агентов

1. **Distance<u8> params size.** `ShamirDistU8` хранит `Arc<Sq8Quantizer>` — при Clone (hnsw_rs клонирует Distance для thread-local в rayon) = Arc::clone = cheap. НО `eval()` на каждом hop читает mins/scales — они должны быть в кеше. Для 128-d: 128*4=512 bytes mins + 512 bytes scales = 1KB — помещается в L1. Для 1536-d (OpenAI): 12KB — L2. Проверить bench.

2. **approx_l2_sq для L2-метрики.** #410 дал только `approx_dot`. Нужен аналог для L2: `sum_i s_i^2 * (a_i - b_i)^2`. SIMD ядро: `diff_sq_sum_u8(a, b) -> u32` (разность u8, квадрат, сумма). Затем per-dim scale: `sum s_i^2 * (a_i-b_i)^2` — не факторизуется в один integer kernel (s_i per-dim). Два варианта: (a) dequant оба -> f32 L2 (медленнее, но проще), (b) precompute `s_i^2` weights, scalar loop с integer diff. Для #411: вариант (a) допустим если L2 не доминирует; оптимизация в #413.

3. **Cosine metric.** Требует нормы vectorов. Норма dequant-вектора: `||x|| = sqrt(sum (min_i + q_i*s_i)^2)`. Можно предвычислить per-vector norm при insert и хранить рядом с кодами (4 bytes extra per vector). Или: нормализовать векторы ДО квантизации (клиент или при insert) — тогда Cosine -> Dot. Рекомендация для #411: требовать normalized input для Cosine+SQ8 (как текущий ShamirDist doc: "callers must normalize for Dot"), либо хранить precomputed norm.

4. **Fit под concurrent upsert.** Момент fit = rebuild. Синхронизация: fit+rebuild в spawn_blocking, создаёт новый `Arc<Hnsw<u8>>`. Atomic swap. Concurrent upserts во время fit идут в f32 buffer; после swap — drain buffer -> quantize -> insert в u8-граф. Порог 256 = BRUTE_FORCE_MAX — до fit все searches brute-force anyway, так что fit блокирует только на 256 vectors (<1ms).

5. **`from_parts_quantized` / snapshot load (#412 scope).** #411 оставляет сигнатуру стабильной. Snapshot codec — #412. Но from_parts_quantized должна быть вызываема (не todo!).

6. **search_cofilter на u8-графе.** `hnsw.search_filter(&query_codes, k, ef, Some(&pred))` — pred замыкание `|id: &usize| allow_set.contains(id)` — работает identically для `Hnsw<u8, _>` (generic по T, filter по id).

7. **VectorQuantization enum: append-only.** Bincode ordinal stability (как StemLanguage). Sq8 = ordinal 0. Future: PQ(1), BQ(2).

8. **Нет известных блокеров Варианта A в hnsw_rs 0.3.4.** Distance<u8> компилируется (спайк #393). Graph generic по T: Clone+Send+Sync — u8 удовлетворяет. parallel_insert, search, search_filter — все generic. Единственное ограничение: встроенные Distance impls (DistL1/DistL2) для u8 считают integer L1/L2 без per-dim scaling — поэтому нужен кастомный ShamirDistU8 (что мы и делаем).
