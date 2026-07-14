בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V0.5 — baseline-отчёт + гигиена доков (закрывает фазу P0)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 0.5 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`.

## Контекст (что уже готово, коммиты)

- V0.0 (5ec84564) — контракт-тесты hnsw_rs 0.3.4. Установленные ФАКТЫ API:
  file_dump/HnswIo::load_hnsw_with_dist ЕСТЬ; parallel_insert ЕСТЬ;
  search_filter/FilterT ЕСТЬ (но при жёстком фильтре может вернуть <knbn →
  нужен overscan ef); **Distance<i8> ОТСУТСТВУЕТ** (u8 — только DistL1/L2, НЕ
  Dot/Cosine); Box::leak(HnswIo)→'static.
- V0.1 (c4603693) — clustered_vectors генератор + @vector scope.
- V0.2 (c777eea1) — upsert_batch (parallel_insert).
- V0.3 (b92e04ee) — criterion vector_search (tune_tiered).
- V0.4 (a603cacc) — vector_report (recall/RSS) + scripts/bench-vector.sh +
  docs/dev-artifacts/benchmarks/vector/README.md.

## Задача

### 1. Baseline-отчёт
Собрать baseline-отчёт качества/латентности и закоммитить в
`docs/dev-artifacts/benchmarks/vector/2026-07-05-baseline.md`.
- Запусти vector_report НАПРЯМУЮ (perimeter-guard блокирует `cargo run`!):
  `cargo build --release -p shamir-engine --example vector_report` (с
  `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench`), затем выполни бинарь
  `D:/dev/rust/.cargo-target-bench/release/examples/vector_report` — он печатает
  готовый markdown-блок. Вставь его в отчёт.
- Для латентности — можно приложить SMOKE-числа criterion (по желанию, если
  быстро): `BENCH_SMOKE=1 CARGO_TARGET_DIR=... cargo bench -p shamir-engine
  --bench vector_search -- <одна-ячейка>`; либо сослаться на способ получения.
- Отчёт по формату из `docs/dev-artifacts/benchmarks/vector/README.md` (reproducibility key:
  версии/seed/HNSW-params/host). ВАЖНО: recall@10 сейчас ~0.5–0.73 — это
  ЧЕСТНЫЙ сигнал при ef_search=50 (дефолт); зафиксируй это явно в отчёте с
  припиской «per-query ef_search (V1.1, task #399) позволит поднять recall».

### 2. Гигиена доков — статус-строки
- `docs/dev-artifacts/roadmap/EMBEDDINGS_AND_VECTORS.md` строка 3: статус «planned, no code
  yet» УСТАРЕЛ — Layer 0 реализован. Обнови на что-то вроде «Layer 0
  implemented (HNSW in-memory + brute-force + SIMD + tx-staging); persistence/
  filtered/quantization in progress — см. VECTOR_PRODUCTION_EXECUTION.md». Плюс
  сверь имена типов в этом доке с кодом: дизайн говорит `Filter::VectorSearch`
  и `CreateVectorIndexOp`, а в коде — `Filter::VectorSimilarity` и DDL через
  `index_type("vector")` + `VectorConfig`. Добавь краткую пометку об этом
  расхождении (не переписывай весь дизайн-док, только зафиксируй факт, что
  реальные имена другие — ориентир код+EXECUTION-план).

### 3. Гигиена доков — K1..K8 в план-док
`docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_PLAN.md` — исходный план разошёлся с кодом.
Добавь секцию (или пометки) с ключевыми поправками, которые уже зафиксированы
в `VECTOR_PRODUCTION_EXECUTION.md` (см. её таблицу «Поправки K1–K8»): FTS уже в
дереве → hybrid разблокирован; hnsw_rs 0.3.4 API подтверждён (file_dump/
parallel_insert/search_filter есть, Distance<i8> нет); chunk-persist паттерн
(InternerManager) вместо atomic-rename; скрытый O(N) в HnswAdapter::len()
(чинится в V4.1/#407). НЕ дублируй весь EXECUTION-план — краткая отсылка +
ключевые факты, чтобы старый план не вводил в заблуждение.

## Дисциплина + гейт

- Это преимущественно docs + запуск инструмента. Rust-код не трогаешь (если
  вдруг тронул — fmt/clippy на затронутый крейт).
- Проверь, что markdown валиден и ссылки/пути верны.
- Тесты гонять не требуется (доки), но `./scripts/test.sh` не должен ломаться —
  ты код не меняешь.
- НЕ трогать код/доки вне задачи.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи; если
инструмент оставил stray-логи в корне — НЕ удаляй их сам (оркестратор уберёт),
просто отметь.

## Definition of done

- `docs/dev-artifacts/benchmarks/vector/2026-07-05-baseline.md` — реальные recall/RSS числа
  + reproducibility key + пометка про ef_search=50.
- `EMBEDDINGS_AND_VECTORS.md` статус обновлён + пометка о расхождении имён.
- `VECTOR_PRODUCTION_PLAN.md` — поправки K1..K8 (кратко, с отсылкой).
- Финал: тронутые файлы, вставленные baseline-числа, что отмечено про recall.
