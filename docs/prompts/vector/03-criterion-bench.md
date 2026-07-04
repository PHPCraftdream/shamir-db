בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V0.3 — criterion-бенч rewrite (tune_tiered, ступени, dim, clustered data)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 0.3 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (раздел «Бенч-фундамент (P0)»).

## Контекст

- `crates/shamir-engine/benches/vector_search.rs` — существующий бенч
  (BruteForce vs HNSW, n=1000/10000, dim=128, Cosine). НАРУШАЕТ правило CLAUDE.md:
  НЕ вызывает `shamir_bench_utils::tune*` (чистый criterion → минуты на вариант).
- **V0.1 дал генератор** `shamir_bench_utils::vector_data::clustered_vectors(n,
  dim, k_clusters, sigma, seed) -> ClusteredDataset { vectors, centroids }`
  (кластеризованный, seeded, детерминированный). Используй ЕГО (не текущий
  локальный uniform-LCG в бенче).
- **V0.2 дал** `HnswAdapter::upsert_batch` / `VectorAdapter::upsert_batch` —
  строй граф батчем (иначе 100K по-одному = очень долго).
- `shamir_bench_utils::tune_tiered(&mut group, sample_size, measurement_secs,
  warm_up_secs, max_wall_secs)` — QUICK по умолчанию. Сверь точную сигнатуру в
  `crates/shamir-bench-utils/src/lib.rs`.

## РЕШЕНИЯ (не отклоняйся)

- **Только QUICK-режим.** FULL-заглушку НЕ трогать. Каждая bench-функция ОБЯЗАНА
  звать `tune_tiered` с `max_wall_secs > 0` (обязательный wall-guard).
- Ступени **10K / 100K** (НЕ 1M в дефолте). 1M — опционально за env
  `BENCH_VECTOR_1M=1` (если добавляешь — только как доп. точка, не в основном
  прогоне; можно и вовсе не добавлять, отметь решение).
- dim **{128, 768}**, metric **{Cosine, L2}**.

## Задача

Переписать `crates/shamir-engine/benches/vector_search.rs`:
1. Данные — `clustered_vectors` из shamir-bench-utils (общий генератор). Seed
   фиксированный, параметры (k_clusters, sigma) — разумные (напр. k=64,
   sigma=0.1), в комментарии.
2. Построение графа/адаптера — через `upsert_batch`.
3. Меряется latency top-k (k=10) search: группы по (n ∈ {10K,100K}, dim ∈
   {128,768}, metric ∈ {Cosine,L2}); BruteForce vs HNSW где уместно (BruteForce
   на 100K — опустить или только маленькую точку, чтобы не висло; обоснуй).
4. КАЖДАЯ группа: `tune_tiered(&mut group, 100, 5, 3, 120)` (или уточни числа —
   QUICK всё равно зарежет; главное wall-guard). `Throughput::Elements`.
5. Убедись, что бенч КОМПИЛИРУЕТСЯ и smoke-прогон в QUICK/SMOKE отрабатывает за
   секунды на вариант.

## Дисциплина + гейт

- НЕ запускай полный `cargo bench` без изоляции. Smoke-прогон:
  `BENCH_SMOKE=1 CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench vector_search -- --profile-time 1`
  ИЛИ просто убедись, что компилируется + пара вариантов проходит быстро.
  (НЕ гоняй 100K на каждом варианте вручную помногу — QUICK-дефолт короткий.)
- fmt `cargo fmt -p shamir-engine -- --check` чист; clippy
  `cargo clippy -p shamir-engine --benches -- -D warnings` чист.
- Бенч-код — не unit-тест; `./scripts/test.sh -p shamir-engine` не должен
  сломаться (бенч не в test-таргете, но проверь, что крейт компилируется).
- Импорты в шапке; НЕ трогать код вне бенча.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи.

## Definition of done

- `vector_search.rs` использует общий clustered-генератор + upsert_batch +
  tune_tiered(wall-guard) на каждой группе; ступени 10K/100K, dim {128,768},
  metric {Cosine,L2}; QUICK-дефолт короткий.
- Компилируется, fmt/clippy чисты, smoke-вариант отрабатывает за секунды.
- Финал: тронутые файлы, структура групп, параметры генератора, что с 1M/
  BruteForce-на-100K решено, вывод smoke-прогона (время на вариант).
