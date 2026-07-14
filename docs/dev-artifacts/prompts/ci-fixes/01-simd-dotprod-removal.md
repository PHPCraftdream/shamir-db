# Brief: remove dead `dotprod` NEON path blocking stable-Rust macOS ARM64 CI (task #588)

## Контекст

`crates/shamir-index/src/vector/simd.rs` содержит `dot_u8_neon_udot` —
NEON-кернел, использующий `vdotq_u32` (интринсик `udot`), гейтящийся
через `#[target_feature(enable = "dotprod")]`. На стабильном Rust для
macOS ARM64 CI-раннеров эта фича приводит к ошибке компиляции
(nightly-only intrinsics path).

Сам `dot_u8` (и транзитивно `dot_u8_neon_udot`) уже помечен
`#[allow(dead_code)]` с комментарием, подтверждающим, что единственный
реальный вызов в продакшене (`Sq8Quantizer::approx_dot`) был удалён
ранее (VR-7/#429) — кернел существует только как тестовая
инфраструктура для будущего weighted-SQ8 SIMD (задача #614, заблокирована
именно этим тикетом).

Уже существует портативный fallback `dot_u8_neon_wide` (использует
`vmull_u8`, только baseline `neon`, без нестабильной зависимости) —
он покрывает тот же функционал без проблемного интринсика.

## Сделано

- Удалена функция `has_dotprod()` (была единственным потребителем —
  проверка feature detection для `dotprod`).
- Удалена ветка диспатча `if has_dotprod() { return unsafe {
  dot_u8_neon_udot(a, b) }; }` внутри `dot_u8`.
- Удалена сама функция `dot_u8_neon_udot` (использовала `vdotq_u32`).
- `dot_u8_neon_wide` остался единственным aarch64-путём внутри `dot_u8`
  (обновлён doc-comment: убрана фраза "CPUs WITHOUT dotprod", теперь
  просто "portable NEON widening path for aarch64 CPUs").

## Почему это безопасно

`dot_u8` и всё его дерево вызовов — dead code в продакшене (подтверждено
существующим `#[allow(dead_code)]` аннотацией и комментарием). Удаление
недостижимого кода не может сломать ничего работающего; корректность
подтверждается существующими SIMD-тестами
(`crates/shamir-index/src/vector/tests/`), которые продолжают проверять
`dot_u8`/`dot_u8_neon_wide` через публичный (в рамках крейта) API.

Кросс-компиляция под aarch64 недоступна в этом окружении (нет cross
`cc` для blake3), поэтому фикс не верифицирован эмпирически на реальном
macOS ARM64 target — обоснование чисто статическое (устранение
недостижимого кода не может привнести новую ошибку компиляции).

## Прогон проверок (выполнено)

- `cargo fmt -p shamir-index -- --check` — чисто.
- `cargo clippy -p shamir-index --all-targets -- -D warnings` — чисто.
- `./scripts/test.sh -p shamir-index --full` — 475/475 passed.
