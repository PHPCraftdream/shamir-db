בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V3.3 — selectivity-бенч filtered ANN (pre/co/post) + отчёт

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 3.3 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P3, третий/последний лист). V3.1 (post-filter, коммит dbc46b05) и V3.2
> (pre-filter + co-filter + cost-based выбор, коммит 43c0b9cc) уже реализованы
> и покрыты юнит-тестами. Здесь — БЕНЧ, который измеряет три пути на разных
> селективностях и ОБОСНОВЫВАЕТ cost-based пороги цифрами + короткий отчёт.

## Зачем

Cost-based выбор в `read_exec.rs` (V3.2) переключает pre/co/post по порогам
`PRE_FILTER_MAX_CANDIDATES` (=4096) и `CO_FILTER_MAX_SELECTIVITY` (=0.20).
Сейчас пороги обоснованы аналитически, не измерены. Нужен бенч, который на
фиксированном HNSW прогоняет три пути при разной доле проходящих фильтр
записей (селективность 0.1%, 1%, 5%, 10%, 25%, 50%) и показывает КРОССОВЕР —
где pre-filter перестаёт выигрывать у co-filter, где co-filter уступает
post-filter. Это валидирует (или корректирует) пороги.

## Контекст кода (проверенные факты)

- Существующий бенч: `crates/shamir-engine/benches/vector_search.rs` —
  паттерн build HNSW через `upsert_batch`, `bu::tune_tiered(&mut group,
  sample, meas, warm, wall_guard)`, `clustered_vectors(n,dim,K,σ,seed)` из
  `shamir_bench_utils::vector_data`, `rt()`, `rid_from(i)`. ИЗУЧИ его —
  повторяй стиль (QUICK-default через tune_tiered, wall-guard, детерм. seed).
- Три пути на уровне адаптера (`crates/shamir-index/src/vector/hnsw_adapter.rs`):
  - `search_prefilter(&query, k, &candidates: &[RecordId])` — точный SIMD по
    candidate-set (pre-filter).
  - `search_cofilter(&query, k, ef_search, &candidates: &[RecordId])` —
    HNSW search_filter с allow-set (co-filter).
  - post-filter: голый `search(&query, k', SearchOpts, None)` с
    oversample-множителем + ручное отсечение по allow-set (эмулируй V3.1
    оверсэмпл-цикл, ИЛИ прогони через engine-путь — на твоё усмотрение, но
    ПРОЩЕ и стабильнее на уровне адаптера).
  - пороги-константы `PRE_FILTER_MAX_CANDIDATES`, `CO_FILTER_MAX_SELECTIVITY`,
    `CO_FILTER_EF_MULTIPLIER` экспортированы из hnsw_adapter — используй их в
    подписи/отчёте, НЕ хардкодь заново.

## Задача

1. **Новый бенч** `crates/shamir-engine/benches/filtered_vector_search.rs`
   (зарегистрируй в `crates/shamir-engine/Cargo.toml` как `[[bench]]` с
   `harness = false`, по образцу `vector_search`). Фиксированный HNSW
   (n=10_000, dim=128, Cosine; при желании добавь ступень n=100_000 под
   env-gate как в ladder()). Для каждой селективности s ∈
   {0.001, 0.01, 0.05, 0.10, 0.25, 0.50}:
   - построить allow-set = детерминированная выборка ⌈s·n⌉ rid (стабильный
     seed);
   - бенчить ТРИ пути (pre/co/post) как отдельные `bench_with_input` с
     `BenchmarkId::new("<path>", format!("sel{permille}"))`;
   - `tune_tiered` для QUICK-default + wall-guard.
   Группа `filtered_vector_search`.
2. **tune** обязателен (иначе минуты на вариант). Детерминизм seed'ов.
   Пиллары: без O(N²) в подготовке, allow-set строй один раз на срез.
3. **Отчёт** `docs/benchmarks/vector/2026-07-05-filtered-ann.md`: таблица
   path × selectivity (mean latency), выделенные кроссоверы, вывод —
   подтверждают ли измерения пороги 4096 / 0.20, или их стоит скорректировать
   (если да — ТОЛЬКО рекомендация в отчёте, код порогов НЕ трогай в этом
   листе). Ссылка на бенч + как воспроизвести (CARGO_TARGET_DIR + команда).
   Обнови `docs/benchmarks/vector/README.md` (одна строка про новый отчёт).

## Прогон бенча (изоляция кэша — ОБЯЗАТЕЛЬНО)

```
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench \
  cargo bench -p shamir-engine --bench filtered_vector_search
```
QUICK-default (быстрый). Цифры для отчёта снимай этой командой; НЕ гоняй
BENCH_FULL (нам нужны быстрые прогоны, не релиз-сигнал).

## Дисциплина + гейт

- Бенч ДОЛЖЕН компилироваться и отрабатывать за секунды (проверь прогоном).
- Гейт: `cargo fmt -p shamir-engine -- --check`; `cargo clippy -p shamir-engine
  --benches -- -D warnings` (бенч — это таргет, clippy его проверит);
  `./scripts/test.sh @vector @engine` (лёгкий — убедиться, что ничего не
  сломал; бенч не трогает прод-код). Запросы/данные — через существующие
  хелперы, не хардкодь JSON.
- Импорты в шапке. Один основной экспорт на файл. НЕ трогать прод-код вне
  задачи (пороги, адаптер) — только новый бенч + отчёт + Cargo.toml [[bench]].
- stray-логи в корне — ОТМЕТЬ их в финальном отчёте, НЕ удаляй сам.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- Новый бенч `filtered_vector_search` (pre/co/post × 6 селективностей),
  зарегистрирован в Cargo.toml, tune_tiered + wall-guard, детерминизм.
- Прогон отработал за секунды (приложи вывод в финале).
- Отчёт `2026-07-05-filtered-ann.md` с таблицей, кроссоверами и вердиктом по
  порогам (4096 / 0.20) + строка в README.
- fmt/clippy(benches)/`./scripts/test.sh @vector @engine` зелёные.
- Финал: тронутые файлы, форма allow-set, наблюдаемые кроссоверы, рекомендация
  по порогам, вывод прогона, что оставлено (если что-то).
