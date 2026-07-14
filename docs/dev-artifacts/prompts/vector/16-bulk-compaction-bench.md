בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V4.3 — бенчи bulk-load и компакции + отчёт

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 4.3 (#409, фаза P4, последний). Бенчи + отчёт для двух
> возможностей P4: батчевый bulk-load (`upsert_batch`, #395) и фоновая
> компакция (#408). НЕ трогай прод-код — только новый бенч + отчёт + Cargo.toml.

## Зачем / DoD
- **bulk-load**: доказать, что `upsert_batch` (один rayon `parallel_insert`)
  даёт **≥5×** vs N одиночных `upsert` на том же датасете. Это hard-DoD листа.
- **компакция**: измерить стоимость `run_background_compaction`-эквивалента
  (rebuild-aside через `collect_live_vectors` + `backfill_if_absent`/upsert_batch)
  на индексе с разной долей tombstone'ов — сколько стоит компакция и как растёт
  с числом живых.

## Контекст (проверенные факты)
- Образец бенча: `crates/shamir-engine/benches/vector_search.rs` и
  `filtered_vector_search.rs` — стиль `bu::tune_tiered(&mut group, ...)`,
  `clustered_vectors(n,dim,K,σ,seed)` из `shamir_bench_utils::vector_data`,
  `rt()`, `rid_from(i)`, `HnswAdapter::new/upsert/upsert_batch`. Повторяй стиль.
- `HnswAdapter` (`crates/shamir-index/src/vector/hnsw_adapter.rs`):
  `upsert(rid,&vec)`, `upsert_batch(&[(rid,vec)])`, `delete(rid)`,
  `collect_live_vectors() -> Vec<(RecordId,Vec<f32>)>`, `deleted_ratio()`,
  `live_count()`, `new_compaction_target(dim,metric,config)`,
  `backfill_if_absent(&[(rid,vec)])`. Компакция-бенч может собрать live-set
  из «грязного» адаптера и построить новый через `new_compaction_target` +
  `backfill_if_absent` (или upsert_batch) — это и есть измеряемая стоимость
  rebuild-aside. Полный `run_background_compaction` завязан на VectorBackend/
  Store — на уровне бенча достаточно смоделировать rebuild-aside стоимость.

## Задача
1. **Новый бенч** `crates/shamir-engine/benches/vector_bulk_compaction.rs`
   (зарегистрируй `[[bench]] harness=false` в `crates/shamir-engine/Cargo.toml`).
   Две группы:
   - `vector_bulk_load`: для n ∈ {1_000, 10_000} (dim=128, Cosine) —
     `bench serial`: построить индекс N одиночными `upsert`; `bench batch`:
     тот же датасет одним `upsert_batch`. Один и тот же датасет/сид. Цель —
     показать ≥5× (batch быстрее).
   - `vector_compaction`: построить индекс n=10_000, натумбстонить долю
     d ∈ {0.3, 0.5} (delete случайных rid), затем бенчить rebuild-aside:
     `collect_live_vectors` + построение нового адаптера через
     `new_compaction_target` + `backfill_if_absent`(live) (или upsert_batch).
     Мера — время компакции.
   - `tune_tiered` + wall-guard ОБЯЗАТЕЛЬНО (QUICK-default). Детерминизм сидов.
2. **Отчёт** `docs/dev-artifacts/benchmarks/vector/2026-07-05-bulk-compaction.md`: таблицы
   (bulk serial vs batch с явным кратным ускорением; компакция по долям
   tombstone'ов), вывод — подтверждён ли DoD ≥5× для bulk; стоимость компакции
   и её масштабирование. Ссылка на бенч + команда воспроизведения. Строка в
   `docs/dev-artifacts/benchmarks/vector/README.md`.

## Прогон (изоляция кэша — ОБЯЗАТЕЛЬНО)
```
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench -p shamir-engine --bench vector_bulk_compaction
```
QUICK-default, НЕ BENCH_FULL. Убедись что отрабатывает за секунды; приложи вывод.
ВНИМАНИЕ: путь CARGO_TARGET_DIR пиши через forward-slash (`D:/dev/...`) — в
bash backslashes съедаются и ломают сборку.

## Дисциплина + гейт
- Бенч ДОЛЖЕН компилироваться и отрабатывать за секунды (проверь прогоном).
- Гейт: `cargo fmt -p shamir-engine -- --check`; `cargo clippy -p shamir-engine
  --benches -- -D warnings`; `./scripts/test.sh @vector @engine` (лёгкий —
  убедиться что ничего не сломано; бенч прод-код не трогает).
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test).
- Импорты в шапке. Один основной экспорт на файл. НЕ трогать прод-код (адаптер/
  бэкенд) — только новый бенч + отчёт + Cargo.toml [[bench]].
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- Бенч `vector_bulk_compaction` (bulk serial vs batch × {1k,10k}; компакция ×
  {0.3,0.5} tombstone), зарегистрирован, tune_tiered+wall-guard, детерминизм.
- Прогон за секунды (вывод приложен), bulk batch ≥5× показан.
- Отчёт `2026-07-05-bulk-compaction.md` + строка в README.
- fmt/clippy(benches)/`./scripts/test.sh @vector @engine` зелёные.
- Финал: тронутые файлы, измеренное bulk-ускорение (кратное), стоимость
  компакции по долям, вывод прогона, вывод гейта.
