# Brief: vectorize weighted SQ8 bilinear term (task #614)

## Контекст

`crates/shamir-index/src/vector/sq8.rs:221-259` — `Sq8Quantizer::approx_dot`
считает приближённый dot product двух quantized-кодов через разложение:

```text
x·y ≈ Σ min_i² + Σ min_i·s_i·(qx_i + qy_i) + Σ s_i²·qx_i·qy_i
```

Константа `Σ min_i²` уже предвычислена в `fit()` (`self.min_sq_sum`).
Линейный член `min_scale[i]·(qx_i+qy_i)` и билинейный член
`scales_sq[i]·qx_i·qy_i` сейчас считаются в ОДНОМ скалярном цикле
(строки 252-257):

```rust
let mut acc = self.min_sq_sum;
for i in 0..self.dim {
    let qx_i = qx[i] as f32;
    let qy_i = qy[i] as f32;
    acc += self.min_scale[i] * (qx_i + qy_i) + self.scales_sq[i] * qx_i * qy_i;
}
acc
```

Задача аудита (10 июля, HIGH, частично исправлено) — векторизовать этот
цикл. Проблема (см. doc-comment строки 210-216): веса `s_i²` per-dimension,
НЕ uniform 1·1, поэтому существующий `dot_u8` (`simd.rs`, integer u8×u8
дот-продукт с uniform весами) сюда НЕ подходит напрямую — нужен НОВЫЙ f32
SIMD-кернел с per-lane весами.

Задача #588 (снятие блокера) уже решена: `crates/shamir-index/src/vector/simd.rs`
больше не содержит `dot_u8_neon_udot`/`has_dotprod` (нестабильный ARM
`dotprod` intrinsic удалён) — единственный aarch64 путь в `dot_u8` теперь
`dot_u8_neon_wide` (baseline `neon`, `vmull_u8` widening, стабильный Rust).
Ориентируйся на его стиль (feature-detect через `OnceLock`, chunked
multi-accumulator, scalar tail) как на образец для нового кернела —
НЕ добавляй новых nightly-only/unstable-feature intrinsics.

## Задача

Написать в `crates/shamir-index/src/vector/simd.rs` новый экспортируемый
(в рамках крейта, `pub(crate)`) кернел, вычисляющий:

```text
weighted_bilinear_f32(min_scale: &[f32], scales_sq: &[f32], qx: &[u8], qy: &[u8]) -> f32
```

т.е. `Σ_i ( min_scale[i]*(qx[i]+qy[i]) + scales_sq[i]*qx[i]*qy[i] )` — ОБА
члена сразу, по образцу существующего скалярного цикла (сохрани
bit-exact порядок накопления там, где это разумно — тесты сравнивают с
скалярной reference с допуском на f32 rounding, см. ниже).

Покрытие:
1. **x86 AVX2** — `#[target_feature(enable = "avx2")]`, конвертация u8→f32
   (`_mm256_cvtepu8_epi32`-style widening, по 8 лейнов за раз при
   256-бит регистрах, либо 8-wide f32 через промежуточный i32), FMA если
   доступна (`_mm256_fmadd_ps`, гейтить отдельным `has_fma()` — не
   обязательна, если `_mm256_mul_ps`+`_mm256_add_ps` проще и корректна).
2. **x86 AVX512** (опционально, если время позволяет — не блокирующее
   требование) — либо явный кернел, либо пропусти с комментарием
   "TODO: avx512 path, AVX2 fallback used" — НЕ обязательно для приёмки
   этой задачи, но AVX2 обязателен.
3. **ARM NEON** — `#[target_feature(enable = "neon")]`, аналогично
   `dot_u8_neon_wide`: widen u8→u16→u32→f32 (`vmovl_u8`/`vcvtq_f32_u32`
   или аналог), умножение на f32 веса, накопление в f32x4.
4. **Scalar fallback** — версия существующего цикла, вынесенная как
   отдельная функция `weighted_bilinear_scalar` для переиспользования как
   reference в тестах и как fallback-путь диспетчера.
5. **Диспетчер** `weighted_bilinear_f32` — тот же паттерн, что у
   `dot_product`/`dot_u8`: `OnceLock`-кэшированный feature-detect,
   ветвление AVX2 → NEON → scalar.

## Интеграция

`Sq8Quantizer::approx_dot` (sq8.rs:221-259) — замени скалярный цикл
(строки 252-257) на вызов нового кернела:

```rust
let mut acc = self.min_sq_sum;
acc += simd::weighted_bilinear_f32(&self.min_scale, &self.scales_sq, qx, qy);
acc
```

(Проверь точное имя модуля/пути импорта — `simd::` или полный путь, как
уже импортируется в файле.)

## Тесты — эквивалентность обязательна

Новый файл или секция в `crates/shamir-index/src/vector/tests/simd_tests.rs`
(или туда, где уже лежат тесты на `dot_u8`/`dot_product` — grep по
образцу): для случайных `dim` (включая 0, 1, не кратные ширине SIMD-
регистра, > одного chunk), случайных `min_scale`/`scales_sq`/`qx`/`qy` —
проверь, что `weighted_bilinear_f32` совпадает со
`weighted_bilinear_scalar` с разумным f32-допуском (например
`(a - b).abs() < 1e-3 * a.abs().max(1.0)` — согласуй допуск с тем, что уже
используется в существующих f32 SIMD-тестах этого файла, если такие есть,
для консистентности). Прогони на этой машине (x86_64) реальные AVX2-тесты
(feature detection должен реально задействовать AVX2-путь, не просто
scalar fallback — добавь тест, который явно проверяет `has_avx2()` перед
уверенным заявлением "AVX2 path exercised").

Также добавь/обнови `Sq8Quantizer`-уровневый тест (в
`crates/shamir-index/src/vector/tests/sq8_tests.rs`), сравнивающий
`approx_dot` до/после рефакторинга даёт тот же результат (в пределах
допуска) на нескольких fit-конфигурациях.

## Прогон проверок

- `cargo fmt -p shamir-index -- --check`
- `cargo clippy -p shamir-index --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-index --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `dot_u8`/`dot_product`/`dot_u8_neon_wide` — не связаны с этой
  задачей (uniform-weight kernels, отдельная забота).
- НЕ добавляй nightly-only/unstable target features (это ровно то, что
  #588 только что убрал для NEON — не повторяй ту же ошибку на новом
  коде).
- Если AVX512-путь не успеваешь — явно пропусти с комментарием, НЕ
  оставляй недописанный/частично работающий unsafe-код.

## Проверка (сделает оркестратор)

- Диф ограничен `simd.rs`, `sq8.rs`, `tests/simd_tests.rs` (или где
  реально лежат тесты), `tests/sq8_tests.rs` — все в `shamir-index`.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-index --full` зелёный, включая новые
  тесты на эквивалентность weighted_bilinear_f32 vs scalar reference.
