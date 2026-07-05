בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V0.4 — vector_report (recall/RSS/build) + scripts/bench-vector.sh

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 0.4 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (раздел «Бенч-фундамент (P0)»).

## Контекст

- **V0.1** дал `shamir_bench_utils::vector_data::clustered_vectors(n, dim,
  k_clusters, sigma, seed) -> ClusteredDataset { vectors, centroids }`.
- **V0.2** дал `VectorAdapter::upsert_batch`.
- **V0.3** — criterion-бенч меряет ТОЛЬКО latency/QPS. Этот лист — ОТДЕЛЬНЫЙ
  инструмент для метрик КАЧЕСТВА (recall) и памяти (RSS), которым criterion не
  подходит (recall детерминирован; RSS не его забота).
- `crates/shamir-index/src/vector/{hnsw_adapter,brute_force}.rs` — адаптеры.

## Задача

### 1. `crates/shamir-engine/examples/vector_report.rs` (пример-бинарь)
(Если `examples/` в shamir-engine нет — создай; сверь, что `Cargo.toml`
подхватывает examples автоматически.)

Инструмент печатает markdown-таблицу метрик КАЧЕСТВА по ступеням:
- Датасет — `clustered_vectors` (общий генератор), ступени по env/аргументам
  (дефолт скромный: n=10K, dim∈{128,768}; параметры seed/k/σ — печатать в отчёт).
- **Recall@1 / Recall@10**: ground truth — брутфорс (точный top-k SIMD или
  `BruteForceAdapter`) по подвыборке ~100 seeded запросов; recall = доля
  HNSW-top-k, совпавших с exact-top-k. Печатать recall@1 и recall@10.
- **Build time**: время построения HNSW-графа через upsert_batch (wall).
- **RSS**: пиковая/текущая resident set size. Используй dev/example-dep
  (напр. `memory-stats` — добавь в `[dev-dependencies]` shamir-engine, сверь
  доступность; ЕСЛИ dep недоступен/тяжёл — отметь и выведи RSS как "n/a" с
  TODO, не блокируйся). RSS — грубая, для порядка величины.
- Вывод — markdown-таблица, готовая вставиться в
  `docs/benchmarks/vector/<date>-baseline.md`. Плюс шапка с reproducibility
  key (версии/seed/параметры) — по Release Benchmark Checklist из плана-дока.
- Детерминизм: без глобального RNG; seed фиксирован.

### 2. Каталог `docs/benchmarks/vector/`
Создать (с `.gitkeep` или README, описывающим формат отчёта и как гонять).

### 3. `scripts/bench-vector.sh`
Единый вход: гоняет criterion-бенч (V0.3) в QUICK + vector_report, собирает в
отчёт-заготовку. ОБЯЗАТЕЛЬНО `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench`
(изоляция кэша — CLAUDE.md). Пробрасывать `BENCH_SMOKE`/`BENCH_VECTOR_1M`.
Скрипт должен быть исполняемым (chmod +x или shebang) и работать в bash.

## Дисциплина + гейт

- Инструмент — example-бинарь, НЕ тест. Запусти его руками на 10K и убедись,
  что печатает валидную markdown-таблицу за разумное время (секунды-десятки).
- fmt `cargo fmt -p shamir-engine -- --check` чист; clippy
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` чист (пример
  попадает под --all-targets).
- `./scripts/test.sh -p shamir-engine` не должен сломаться.
- Импорты в шапке; НЕ трогать код вне задачи; запросы к БД тут не строятся
  (это низкоуровневый adapter-бенч, query-builder не нужен).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи.

## Definition of done

- `examples/vector_report.rs` печатает markdown recall@1/@10 + build + RSS по
  ступеням, общий генератор + upsert_batch, детерминизм по seed.
- `docs/benchmarks/vector/` создан (с README формата).
- `scripts/bench-vector.sh` — единый вход с CARGO_TARGET_DIR-изоляцией.
- fmt/clippy чисты; ручной прогон на 10K печатает таблицу.
- Финал: тронутые файлы, пример вывода таблицы (recall-числа), как решён RSS-dep,
  время прогона.
