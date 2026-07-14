בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V5.3 — снапшот v2 (quantization) + миграция + компакция-aware + бенч

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Реализуешь
> лист 5.3 (#412, фаза P5, последний). Персист квантованного индекса
> (#411 отложил персист на этот лист: `VectorConfig.quantization` сейчас
> `#[serde(skip)]`, при рестарте квант-индекс ребилдится как f32). Здесь —
> снапшот v2, который сохраняет квант-параметры + u8-коды/граф, миграция
> v1→v2, компакция-aware, и бенч f32 vs sq8 (RSS/QPS/recall).

## Контекст (проверенные факты)
- `crates/shamir-index/src/vector/snapshot.rs` — кодек v1: dump graph через
  hnsw_rs `file_dump`, чанки+crc32 в info_store, sidecar (MetaEnvelope,
  bincode), manifest, всё в одном `Store::transact`. `SNAPSHOT_FORMAT_VERSION:
  u16 = 1` (стр.83). **Sidecar УЖЕ несёт зарезервированное поле**
  `quantization: Option<Vec<u8>>` (стр.192, «RESERVED P5, всегда None в V2.1»)
  — используй его. `HNSW_RS_VERSION`-гейт (VersionMismatch на чужой версии).
  load: `HnswIo::load_hnsw_with_dist` + `Box::leak` → `Hnsw<'static,...>`,
  `from_parts` восстанавливает адаптер.
- `crates/shamir-index/src/vector/hnsw_adapter.rs` (#411): квант-адаптер имеет
  `hnsw_u8: ArcSwapOption<Hnsw<u8,ShamirDistU8>>`, `vectors_u8: scc::HashMap
  <usize,Vec<u8>>`, `quantizer: OnceLock<Arc<Sq8Quantizer>>`, `is_fitted`.
  `Sq8Quantizer` (sq8.rs) несёт `mins: Vec<f32>`, `scales: Vec<f32>` + dim
  (сериализуемо). `ShamirDistU8::new(Arc<Sq8Quantizer>, metric)`.
- `crates/shamir-index/src/vector/tests/crash_recovery_tests.rs`,
  `vector_restore_tests.rs`, `delta_log_tests.rs` — образцы тестов персиста.

## ⚠️ ПЕРВЫЙ ШАГ — де-риск (сделай спайк-тест ДО основной работы)
Проверь, что hnsw_rs 0.3.4 `file_dump` + `HnswIo::load_hnsw_with_dist`
работают для `Hnsw<'static, u8, ShamirDistU8>` (в спайке #393 доказано, что
`Hnsw<i8,_>` строится/ищет — но dump/load u8-графа НЕ проверялся). Напиши
маленький тест (в snapshot tests): построй `Hnsw<u8,ShamirDistU8>`, `file_dump`,
`load_hnsw_with_dist` с восстановленным ShamirDistU8, убедись что поиск даёт
те же результаты. ЕСЛИ file_dump НЕ работает для u8 (например требует
Serialize на T или падает) — ЗАФИКСИРУЙ это и переключись на план B: сериализуй
u8-КОДЫ (vectors_u8) в чанки напрямую (как data-чанки) + при load ПЕРЕСТРОЙ
u8-граф через parallel_insert из кодов (граф-структура не сохраняется, но
восстанавливается детерминированно из кодов + quantizer — это дороже, но
корректно и не зависит от file_dump<u8>). Реши по факту спайка, обоснуй.

## Задача
1. **Формат v2**: bump `SNAPSHOT_FORMAT_VERSION` → 2. Sidecar пишет
   `quantization: Some(bincode(QuantMeta))` для квант-адаптера, где `QuantMeta`
   = { mins, scales, dim, method:"sq8" }. Для неквантованного — `None` (v2 с
   quantization=None эквивалентен v1-семантике по данным).
2. **dump квант-адаптера**: сохрани (а) квант-параметры в sidecar.quantization;
   (б) u8-граф ИЛИ коды (по итогу спайка: file_dump<u8> либо коды-чанки+rebuild).
   `dump_snapshot`/`dump_snapshot_with_gen` — ветка по `is_fitted`/quantization.
3. **load/from_parts**: восстанови квант-адаптер — quantizer из QuantMeta,
   u8-граф (load или rebuild из кодов), `is_fitted=true`, vectors_u8. Search
   после рестарта идёт квант-путём (u8-graph + rescore) с тем же recall.
4. **Миграция**: (а) v1-снапшот (f32) грузится как раньше (back-compat, БЕЗ
   VersionMismatch — прими и v1, и v2). (б) конфиг С quantization, но снапшот
   v1 (f32, без квант-мета) → rebuild+warn (log::warn, полный ребилд из
   data-store; НЕ падать). (в) v2-снапшот на билде, который понимает только
   v1 — уже покрыто VersionMismatch-гейтом (не регресс).
5. **Компакция-aware (#408)**: после rebuild-aside компакции u8-адаптер несёт
   свежие коды; форс-снапшот после компакции должен писать v2 с актуальной
   квант-метой. Убедись, что компакция+снапшот+рестарт сохраняют квант-состояние
   (тест).
6. **Restore in VectorBackend**: `VectorConfig.quantization` сейчас
   `#[serde(skip)]` (не персистится в descriptor). Реши: либо снять skip и
   персистить в descriptor (если bincode-проблему из #411 можно обойти —
   проверь), либо восстанавливать quantization ИЗ снапшот-sidecar при
   restore_on_open (предпочтительно — sidecar durable и уже несёт квант-мету).
   Опиши выбор. Квант-индекс ДОЛЖЕН пережить рестарт как квантованный.
7. **Бенч** `docs/dev-artifacts/benchmarks/vector/2026-07-05-quantization.md` + бенч-функция
   (в существующем vector-бенче или новом): f32 vs sq8 — RSS (память, через
   `memory-stats` dev-dep как в #397), QPS (search latency), recall@10.
   Показать 4× memory-редукцию (или измеренную) + recall-drop. QUICK-tune.

## Тесты (TDD)
- спайк file_dump<u8> (см. выше).
- round-trip квант-снапшота: build квант-адаптер → dump v2 → load → search
  даёт эквивалентный top-k (recall сохранён); quantizer-params совпадают.
- миграция: v1-снапшот грузится (back-compat); quantization-конфиг + v1-снапшот
  → rebuild+warn (не падает, индекс рабочий).
- crash/cold-start для квант-индекса (по образцу crash_recovery_tests): рестарт
  сохраняет квант-состояние, is_fitted=true, recall≥порог.
- компакция+снапшот+рестарт: квант-состояние переживает.
- back-compat: неквантованный снапшот v2 round-trip как v1.

## Дисциплина + гейт
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector --full` (несколько раз — персист+квант concurrency);
  `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo fmt
  -p shamir-index -- --check`. Бенч — изоляция кэша
  `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench` (forward-slash!), QUICK.
- Пиллары: lock-free, spawn_blocking для file_dump/build, crc32 на чанках,
  atomic transact. Импорты в шапке. Один основной экспорт на файл. Не ломай
  v1 персист (существующие crash_recovery/restore тесты зелёные).
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- Снапшот v2 (квант-мета в sidecar + u8-граф/коды), load/from_parts
  восстанавливает квант-адаптер, миграция v1→v2 + rebuild-warn на v1@quant,
  компакция-aware, restore квант-индекса переживает рестарт квантованным.
- Тесты (спайк, round-trip, миграция, crash/cold-start квант, компакция,
  back-compat) зелёные; существующие v1-персист-тесты не сломаны.
- Бенч f32 vs sq8 + отчёт (RSS/QPS/recall, memory-редукция).
- `./scripts/test.sh @vector --full` + clippy + fmt зелёные.
- Финал: итог спайка file_dump<u8> (сработал / план B), как персистится
  квант-мета и восстанавливается при рестарте, как решён restore
  quantization (sidecar vs descriptor), измеренные RSS/QPS/recall f32 vs sq8,
  вывод гейта.
