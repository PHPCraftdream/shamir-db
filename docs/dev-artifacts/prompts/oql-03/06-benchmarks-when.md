# Brief: OQL Epic 03 / Phase F — бенчмарки conditional execution (task #649)

## Контекст

Фазы A-E (#644-#648) реализованы. **ВАЖНОЕ ОГРАНИЧЕНИЕ** (найдено в Фазе E,
задача #651, ЕЩЁ НЕ ИСПРАВЛЕНО): `when`-фильтры на основе полевых сравнений
(`Eq`/`Gt`/`Gte`/etc.) сегодня СТРУКТУРНО не работают — `resolve_skip`
использует пустой синтетический record + scratch-interner, а `Filter`'s
сравнения требуют реального field-lookup, которого для `when` физически
нет. Работают только `IsNull`/`IsNotNull` (используемые как a `$query`-
presence guard, см. `crates/shamir-client/tests/batch_when_e2e.rs`).

**Для этой бенч-задачи**: используй ТОЛЬКО `IsNull`/`IsNotNull`-based
`when`-условия (единственный сегодня реально РАБОТАЮЩИЙ путь) — как в e2e
тестах. НЕ пытайся бенчить `Gte`/`Eq`-based `when` (они всегда дают
фиксированный результат независимо от данных, замер был бы бессмысленным).

**Важно (repeatedly-forgotten mistake)**: бенчмарки используют
`bench_scale_tool::Harness`, НЕ Criterion. Смотри CLAUDE.md секцию "Benches
use `bench_scale_tool::Harness`" и `crates/shamir-engine/benches/
batch_stage_parallelism.rs`/`cond_expr_eval.rs` как образцы.

## Задача

Новый бенчмарк `crates/shamir-engine/benches/when_skip_eval.rs`:

- Батч с 50 op, у половины `when: IsNotNull(...)` (условие false → skip),
  у другой половины без `when` (безусловное исполнение) — подтверди, что
  skip реально дешевле полного исполнения (нет скана/read-set для
  пропущенного op).
- Батч БЕЗ единого `when`-поля (все 50 op безусловны) — подтверди
  отсутствие регрессии по сравнению с существующим
  `batch_stage_parallelism.rs`'s `reads_50` кейсом (Epic01/E) — сравни
  абсолютные числа, не обязательно точное совпадение, но не должно быть
  drastически хуже.
- Каскадный skip цепочки из 5 op (A skipped → B,C,D,E каскадно skipped) —
  подтверди, что каскад дешевле, чем 5 полных исполнений.

## Прогон и вывод

```
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench when_skip_eval
```

Получи реальные числа, задокументируй в докстринге бенч-файла.

## Прогон проверок

- `cargo fmt -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine --all-targets --benches -- -D warnings`
- Бенч реально запускается и печатает числа.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ используй Criterion API.
- НЕ пытайся исправить баг #651 (фиктивный interner) — только измеряй с
  учётом ограничения (используй `IsNull`/`IsNotNull`).
- Если появится ошибочная директория `devrust.cargo-target-bench` из-за
  бага экранирования backslash в Git Bash — удали её, не включай в диф.

## Проверка (сделает оркестратор)

- Новый файл `crates/shamir-engine/benches/when_skip_eval.rs`.
- fmt/clippy чисты (включая `--benches`); бенч реально прогоняется.
