בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V5.1 — SQ8-квантайзер + int8 SIMD dot-ядро

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 5.1 (#410, фаза P5, первый). ТОЛЬКО квантайзер + SIMD-ядро,
> STANDALONE — интеграция в HNSW-граф/rescoring/DDL — это #411, НЕ здесь.
> Прод-код с `unsafe` SIMD: соблюдай дисциплину SAFETY-комментариев и
> обязательный тест «ядро == скалярный референс».

## Зачем
SQ8 (scalar quantization, 8 бит/компонента) режет память вектора в 4× (f32→u8)
ценой малой потери recall. Нужен: (1) квантайзер с per-dim asymmetric min/max,
(2) быстрое int8-dot SIMD-ядро для скоринга квантованных кодов. DoD: ядра
бит-в-бит равны скалярному integer-референсу; recall-drop ≤2% на датасете.

## Контекст (проверенные факты)
- `crates/shamir-index/src/vector/simd.rs` (485 строк) — ЭТАЛОН СТИЛЯ. Паттерн:
  `has_avx512f()/has_avx2()/has_neon()` (OnceLock-кэш `std::is_x86_feature_detected!`),
  публичная `pub(crate) fn dot_product` диспетчит в `unsafe fn *_avx2/avx512/neon`
  (каждая с `#[target_feature(enable=...)]` + `// SAFETY:` на месте вызова) и
  `*_scalar` fallback (chunked, multi-accumulator). ПОВТОРЯЙ этот паттерн для
  int8-ядра. НЕ дублируй f32-ядра.
- `crates/shamir-index/src/vector/mod.rs` — добавь `pub mod sq8;`.
- Регистрация тестов: раскладка `tests/` (новый `tests/sq8_tests.rs` +
  `tests/simd_tests.rs` если ещё нет, через `tests/mod.rs`).

## Задача

### 1. `vector/sq8.rs` — `Sq8Quantizer` (per-dim asymmetric)
- Обучение: `Sq8Quantizer::fit(vectors: &[Vec<f32>], dim: usize) -> Self` —
  вычисляет per-dimension `min_i`, `max_i` по обучающему набору. Храни
  `mins: Vec<f32>`, `scales: Vec<f32>` где `scale_i = (max_i - min_i) / 255.0`
  (при `max_i==min_i` → scale=0, деквант даёт `min_i`).
- `quantize(&self, v: &[f32]) -> Vec<u8>`: `q_i = round((v_i - min_i)/scale_i)`
  clamp в `[0,255]` (при scale_i==0 → 0). Проверка dim.
- `dequantize(&self, q: &[u8]) -> Vec<f32>`: `min_i + q_i as f32 * scale_i`.
- **Аппрокс-dot из двух кодов** (для rescoring в #411, здесь — метод + тест):
  `approx_dot(&self, qx: &[u8], qy: &[u8]) -> f32`. Разложи
  `x_i≈min_i+qx_i·s_i`, `y_i≈min_i+qy_i·s_i`; тогда
  `dot ≈ Σ min_i² + Σ min_i·s_i·(qx_i+qy_i) + Σ s_i²·qx_i·qy_i`. Целочисленный
  член `Σ qx_i·qy_i` считается int8-SIMD-ядром (п.2), линейные члены `Σ qx_i`,
  `Σ qy_i` и константы — предвычислить/скаляр. Обоснуй математику в докстроке.
  (Достаточно корректной аппроксимации; точная арифметика term-by-term.)

### 2. `simd.rs` — целочисленное dot-ядро `dot_u8(a: &[u8], b: &[u8]) -> u32`
Считает ТОЧНО `Σ (a_i as u32) * (b_i as u32)`.
- **Скалярный референс** `dot_u8_scalar` — chunked multi-accumulator, `u32`.
  Это ЭТАЛОН, с которым ядра сверяются БИТ-В-БИТ (целые числа → строгое `==`,
  без float-допуска).
- **AVX2**: используй `_mm256_madd_epi16` над расширенными до i16 половинами
  ИЛИ `_mm256_maddubs_epi16`. ⚠️ КРИТИЧНО: `maddubs` даёт НАСЫЩЕННЫЙ i16 для
  суммы двух произведений u8·i8 — при больших значениях НЕ равно чистой сумме
  (saturation) и второй операнд трактуется как i8 (u8 code >127 станет
  отрицательным!). Если используешь maddubs — докажи отсутствие переполнения/
  насыщения для диапазона u8[0,255] ИЛИ выбери безопасный путь (расширение
  u8→u16/i32 и `madd`/накопление в i32/u32). Ядро ОБЯЗАНО == скаляру на ЛЮБЫХ
  u8-входах, включая крайние 255·255. Проверь тестом на насыщающих значениях.
- **AVX-512 VNNI** (если доступно, `has_avx512f`+проверь vnni-гейт отдельно
  через `is_x86_feature_detected!("avx512vnni")`, НЕ полагайся на avx512f):
  `_mm512_dpbusd_epi32(acc, u8, i8)` — но снова signedness (второй операнд i8).
  Учти: code u8>127. Либо VNNI-путь только когда безопасно, либо разложи. Если
  сомнения — оставь VNNI на #411 и ЗАФИКСИРУЙ в финале, что VNNI отложен;
  AVX2+scalar достаточно для DoD.
- **NEON aarch64**: `vdotq_u32` (udot, u8·u8→u32-accumulate) если фича `dotprod`
  доступна (`is_aarch64_feature_detected!("dotprod")`), иначе NEON-расширение
  или scalar. udot — беззнаковый, ровно наш случай.
- Диспетчер `dot_u8` зеркалит `dot_product` (детект → unsafe ядро → scalar).
- Каждый `unsafe`-блок/`fn` — `// SAFETY:` называющий гарантирующий фичу-гейт.

## Тесты (TDD red-first)
- **ЯДРО == СКАЛЯР (обязательно, DoD)**: для random u8-векторов разных dim
  (1,7,8,15,16,31,32,64,127,128) и КРАЙНИХ (все 255, все 0, смешанные)
  `dot_u8` (диспетчер + каждое доступное ядро напрямую) == `dot_u8_scalar`
  БИТ-В-БИТ. Тест на насыщение: векторы из 255 длиной ≥32 (сумма велика) —
  проверь что нет saturation-расхождения.
- **квантайзер round-trip**: `dequantize(quantize(v))` в пределах
  `scale_i/2 + eps` покомпонентно (ошибка квантования ≤ полшага).
- **fit корректность**: per-dim min/max равны реальным min/max обучающего
  набора; scale согласован.
- **approx_dot vs f32 dot**: на квантованных кодах `approx_dot` близок к
  истинному `dot_product` (относит. ошибка мала; статистич.).
- **recall ≤2% drop (DoD)**: датасет (напр. `clustered_vectors` из
  `shamir_bench_utils` — dev-dep, или синтетика) 1–5k, dim 64/128; top-k по
  f32-dot vs top-k по approx_dot(квантованные) — recall@k ≥ 0.98. Строй
  данные через существующие хелперы.
- edge: dim mismatch → паника/Err по контракту; scale_i==0 (константная
  размерность) не делит на ноль.

## Дисциплина + гейт
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector --full`; `cargo clippy -p shamir-index
  --all-targets -- -D warnings`; `cargo fmt -p shamir-index -- --check`.
- Пиллары: SIMD-ядра по образцу simd.rs; НИКАКИХ `unsafe` без `// SAFETY:`;
  фича-детект кэшируй (OnceLock) как существующие; scalar-fallback обязателен
  (портируемость). Импорты в шапке. Один основной экспорт на файл (sq8.rs =
  Sq8Quantizer; dot_u8 — в simd.rs рядом с dot_product). НЕ трогай HNSW-граф/
  адаптер/DDL (это #411).
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- `vector/sq8.rs` `Sq8Quantizer` (fit/quantize/dequantize/approx_dot,
  per-dim asymmetric); `simd.rs` `dot_u8` (scalar-референс + ≥AVX2 + NEON;
  VNNI опц., отложи если рискованно) с фича-детектом и SAFETY.
- Ядро==скаляр бит-в-бит (вкл. saturation-edge), round-trip, approx_dot,
  recall ≤2% drop — зелёные.
- `./scripts/test.sh @vector --full` + clippy + fmt зелёные.
- Финал: тронутые файлы, как решён saturation/signedness в AVX2 (и почему
  ядро==скаляр гарантировано), какие ядра реализованы (VNNI отложен?),
  измеренный recall-drop, вывод гейта, что оставлено на #411 (граф+rescoring+DDL).
