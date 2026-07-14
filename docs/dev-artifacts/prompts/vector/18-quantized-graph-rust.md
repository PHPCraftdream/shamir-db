בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V5.2 Фаза A — квантованный HNSW-граф (Rust core) + DDL/wire/builder

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Реализуешь
> ФАЗУ A листа #411 (V5.2) строго по дизайну `docs/dev-artifacts/design/vector-quantized-graph.md`
> (прочитай ЦЕЛИКОМ — он источник истины по механизму, Вариант A). Фаза A —
> ТОЛЬКО Rust: config + ShamirDistU8 + dual-graph adapter + fit + rescore +
> DDL + wire + Rust-билдер + Rust-тесты. TS (ddl.ts + parity vitest) — ФАЗА B,
> НЕ трогай. #410 УЖЕ дал `sq8.rs` (Sq8Quantizer) и `simd.rs::dot_u8`.

## Ключевые решения дизайна (следуй)
- **Граф на u8**: `Hnsw<'static, u8, ShamirDistU8>` — hnsw_rs хранит коды внутри
  (реальная 4× экономия). Feasibility доказана спайком #393
  (`hnsw_rs_contract_tests.rs:460`, `Hnsw<i8,I8L2>` работает).
- **ShamirDistU8** impl `Distance<u8>`, держит `Arc<Sq8Quantizer>` (Clone дёшев;
  hnsw_rs клонирует Distance; params frozen после fit).
- **Opt-in**: `VectorConfig.quantization: Option<VectorQuantization>` +
  `#[serde(default)]` → неквантованные индексы работают БИТ-В-БИТ как раньше.
- **Deferred fit**: <256 (FIT_THRESHOLD=BRUTE_FORCE_MAX) — f32 as-is
  (brute-force точный); при пересечении 256 — fit + построить u8-граф +
  atomic swap; post-fit — все insert через quantize.
- **Rescore**: graph traversal (overscan 2k+10) на кодах → top-k пересчитать
  через dequant + точная f32-дистанция.

## ⚠️ Тонкости, которые дизайн оставил impl'у — НЕ срежь
1. **eval per-metric МАТЕМАТИКА (корректность).** `ShamirDistU8::eval(&[u8],&[u8])->f32`:
   - **L2**: `Σ s_i²·(a_i-b_i)²`. s_i² PER-DIM — НЕ выносится из суммы. Нельзя
     «один integer-Σ(a-b)² × один scale». Нужен per-dim цикл: integer diff
     `(a_i-b_i)` (в i32, т.к. u8-u8 может быть отриц.), возвести, умножить на
     `s_i²`, накопить f32. Integer-часть можно векторизовать, но s_i²-веса
     обязательны поэлементно. Докажи в докстроке, что eval == точная
     дистанция на dequant-векторах (в пределах округления квантизации).
   - **Dot/Cosine**: через `Sq8Quantizer::approx_dot` (#410, term-by-term
     с per-dim s_i²·qx·qy + линейные члены) + нормировка для Cosine (нормы
     на dequant, кешируй per-vector если можно). Сверь с dequant-эталоном.
   - **hnsw_rs требует** `Distance: Clone+Send+Sync`; eval детерминирован.
2. **Конкурентность при fit-переходе (класс #408).** Переход f32→u8-граф
   (fit + rebuild + swap) идёт под нагрузкой мутаций. Мутация, пришедшая во
   время построения u8-графа, НЕ должна потеряться. Индекс мал (~256), окно
   короткое, но НЕ игнорируй: single-flight-guard на fit (AtomicBool
   `fit_in_flight` + drop-guard), и либо кратко-эксклюзивный swap, либо
   double-write в строящийся граф (как #408), либо после swap доинсертить
   дельту. Выбери минимально-корректный, обоснуй отсутствие потерь. Тест на
   это (конкурентные upsert через порог).
3. **Opt-in back-compat**: без quantization весь путь — прежний f32
   `Hnsw<f32,ShamirDist>`, бит-в-бит. Ни одной регрессии в неквантованных
   тестах (существующие 1724 @vector — зелёные).

## Задача (уровни Фазы A)
1. **kind.rs**: `pub enum VectorQuantization { Sq8 }` (или с параметрами, если
   нужно); `VectorConfig.quantization: Option<VectorQuantization>` +
   `#[serde(default)]`. Не ломать существующую сериализацию (старые снапшоты/
   wire без поля → None).
2. **vector/quantized_dist.rs** (новый, один экспорт): `ShamirDistU8` +
   `impl Distance<u8>` с per-metric eval (см. тонкость 1). Юнит-тест
   eval==dequant-эталон.
3. **hnsw_adapter.rs**: dual-graph поля (`hnsw` f32 + `hnsw_u8: Option<...>`,
   `codes` map, `quantizer: ArcSwapOption<Sq8Quantizer>` или `Option`+флаг,
   `is_fitted: AtomicBool`, `fit_in_flight`). `new` учитывает
   `quantization`-опцию. upsert/upsert_batch: pre-fit копят f32; при пороге
   `try_fit_and_rebuild`; post-fit quantize+insert коды. search: post-fit —
   traversal u8-граф (overscan) + dequant-rescore; pre-fit/unquantized —
   прежний путь. delete/tombstone, `collect_live_vectors` (для #408 —
   возвращает коды post-fit), snapshot-аксессоры — не ломать (полную
   сериализацию квантизации доделает #412, но структуры не ломай, оставь
   задел; можно `todo!`-заглушку в snapshot ТОЛЬКО если она не на пути
   существующих тестов — иначе адаптируй чтобы неквантованный путь работал).
4. **DDL** `table_manager_index_mgmt.rs` (~160-190): пробросить
   `op.vector_quantization` → `VectorConfig.quantization`.
5. **wire create-index op**: найди struct с `vector_dim`/`vector_metric`
   (Rust-сторона), добавь `vector_quantization: Option<String>` (или enum) с
   serde-back-compat (старое сообщение без поля парсится).
6. **Rust query-builder**: метод/поле для quantization в create-vector-index.
7. **Rust-тесты** (раскладка tests/): eval-корректность; opt-in (без
   quantization = f32 путь, back-compat); fit-переход (индекс через 256 →
   is_fitted, u8-граф активен); recall квант vs f32 ground-truth (≤2% drop,
   recall@10≥0.98) на ≥1k dim 128; конкурентный fit-переход (без потерь);
   wire serde back-compat (старый create-op без поля → None); DDL round-trip
   (создать квант-индекс → конфиг несёт Sq8).

## Дисциплина + гейт
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector @engine --full`; `cargo clippy -p shamir-index
  -p shamir-engine --all-targets -- -D warnings`; `cargo fmt` тронутых `-- --check`.
- Пиллары: unsafe только в #410-ядрах (переиспользуй, не дублируй); lock-free
  (ArcSwap/atomics), spawn_blocking для graph build, guard не через await, без
  O(N²). Импорты в шапке. Один основной экспорт на файл. НЕ трогай TS.
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- `VectorConfig.quantization` (opt-in, serde-default); `ShamirDistU8`
  (per-metric eval корректна); dual-graph HnswAdapter (deferred fit + u8-граф
  + dequant-rescore); DDL+wire+Rust-builder протянуты; конкурентный fit-переход
  без потерь.
- Тесты (eval, opt-in back-compat, fit-переход, recall≤2%, конкурентность,
  wire back-compat, DDL round-trip) зелёные; существующие 1724 @vector не
  сломаны.
- `./scripts/test.sh @vector @engine --full` + clippy + fmt зелёные.
- Финал: тронутые файлы, как реализована per-metric eval и доказательство
  eval==dequant, как закрыта конкурентность fit-перехода (без потерь), опции
  памяти (4× подтверждено?), измеренный recall-drop, вывод гейта, что
  оставлено на Фазу B (TS+parity) и #412 (snapshot квантизации).
