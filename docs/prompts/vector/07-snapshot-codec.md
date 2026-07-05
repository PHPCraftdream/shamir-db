בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V2.1 — snapshot-кодек HNSW (dump/load round-trip, chunks+crc32 в info_store)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 2.1 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P2 — persisted HNSW; см. раздел «Персистентность HNSW»). Это ПЕРВЫЙ
> лист P2: только КОДЕК (dump→байты→info_store и обратно), БЕЗ startup-
> интеграции (#401) и delta-log (#402).

## Зачем

Сейчас HNSW-граф in-memory: рестарт = полный rebuild сканом (минуты на больших
N). Нужен снапшот графа в info_store, чтобы рестарт был O(load). Этот лист даёт
изолированный, юнит-тестируемый кодек снапшота.

## Проверенные факты API (из спайка V0.0, коммит 5ec84564; контракт-тесты
## crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs)

- `use hnsw_rs::api::AnnT;` → `hnsw.file_dump(dir: &Path, basename: &str) ->
  anyhow::Result<String>` (TRAIT-метод; пишет `<basename>.hnsw.graph` +
  `.hnsw.data` в СУЩЕСТВУЮЩУЮ директорию, возвращает basename).
- `HnswIo::new(dir, basename)` + `.load_hnsw_with_dist::<f32, ShamirDist>(dist)
  -> Result<Hnsw<'b, f32, ShamirDist>>` (нужен `_with_dist` — ShamirDist без
  Default). Lifetime `'a: 'b` → для `Hnsw<'static>` нужен `Box::leak(Box::new(
  HnswIo))` (boot-only, осознанный leak; паттерн в контракт-тесте
  `leaked_loader_yields_static_hnsw`).
- tempfile в dev-deps (3.10.1).

## Образцы для переиспользования (ИЗУЧИ перед кодом)

- **Chunk-persist паттерн**: `crates/shamir-engine/src/table/interner_manager.rs`
  — HWM (last_persisted_len), next_chunk_idx, zero-padded chunk-ключи в
  info_store, инкрементальная запись дельты. Тот же скелет для снапшота.
- **MetaEnvelope**: `crates/shamir-index/src/meta_envelope.rs` — magic `SDB2` +
  version + payload (bincode). Оборачивай sidecar в него.
- **Store API**: `crates/shamir-storage/src/types.rs` — `Store::{set, get,
  remove, transact(Vec<KvOp>)}`. `transact` атомарен (мульти-set/remove).
- **crc32fast** уже в deps shamir-index (заведён в V0.0).
- HnswAdapter поля (`crates/shamir-index/src/vector/hnsw_adapter.rs`): dim,
  metric, ef_search, hnsw (Arc<Hnsw<'static>>), rid_map (internal→RecordId),
  rid_to_internal, vectors (internal→Vec<f32>), deleted (tombstones), next_id.

## Задача — новый `crates/shamir-index/src/vector/snapshot.rs`

Кодек снапшота (модуль подключить в `crates/shamir-index/src/vector/mod.rs`;
раскладка tests/ — новый `tests/snapshot_tests.rs` + манифест).

### Формат снапшота (в info_store, keyspace передаётся аргументом)
- **file_dump в temp-dir** (`tempfile::TempDir` или std temp) → прочитать оба
  файла `.hnsw.graph`/`.hnsw.data` (spawn_blocking для CPU/IO).
- **Чанки**: порезать каждый файл на ~1 MiB чанки, записать в info_store под
  zero-padded ключами (напр. `<keyspace>.gN.graph.KKKK`, `.gN.data.KKKK`), с
  **crc32 каждого чанка** (пиллар «checksums everywhere»).
- **Sidecar** (MetaEnvelope, bincode): `{ dim, metric, hnsw params (m,
  ef_construction, max_layer), next_id, rid_map (internal→RecordId),
  rid_to_internal, tombstones (deleted set), hnsw_rs_version: String,
  quantization: Option<...> (RESERVED поле под P5 — заведи как Option, пока
  None), crc32 всех секций }`.
- **Манифест** (MetaEnvelope): активное поколение `gen N`, число чанков graph/
  data, basename. Запись/флип манифеста — потом (в #402 generation flip); здесь
  достаточно писать одно поколение и манифест на него.

### API кодека (примерно, уточни под стиль)
- `pub async fn dump_snapshot(adapter: &HnswAdapter, store: &Arc<dyn Store>,
  keyspace: &str) -> Result<(), SnapshotError>` — dump графа + sidecar +
  манифест в info_store.
- `pub async fn load_snapshot(store: &Arc<dyn Store>, keyspace: &str) ->
  Result<HnswAdapter, SnapshotError>` (или возвращает части для сборки адаптера
  — на твоё усмотрение; главное восстановить рабочий граф + rid_map +
  tombstones). Load: манифест → verify crc чанков → temp-файлы → HnswIo →
  Box::leak → Hnsw<'static> → собрать HnswAdapter.
- `SnapshotError` (thiserror): `Corrupt` (crc mismatch), `VersionMismatch`
  (чужая версия формата/hnsw_rs), `Io`, `Serde`, `Backend`.

Возможно понадобится в HnswAdapter добавить конструктор `from_parts(...)` или
геттеры к полям для кодека — сделай минимально (pub(crate) где надо).

## Тесты (TDD red-first) — `tests/snapshot_tests.rs`, in-memory Store

- **round-trip preserves top-k**: построить граф (N>256, HNSW-путь),
  dump_snapshot → load_snapshot на in-memory Store → top-k соседи для
  нескольких запросов идентичны до/после (сравни МНОЖЕСТВА id; как в
  контракт-тесте file_dump_load_roundtrip).
- **битый crc чанка → Err(Corrupt)**: испортить байт в одном чанке в store →
  load → Err(Corrupt), не паника.
- **чужая версия формата → Err(VersionMismatch)**: подменить version в sidecar/
  манифесте → Err(VersionMismatch).
- **tombstones выживают**: удалить пару rid → dump → load → удалённые не
  всплывают в поиске; live — на месте.
- **rid_map/next_id восстановлены**: после load rid'ы резолвятся, next_id не
  меньше исходного (новые upsert'ы не конфликтуют).

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh -p
  shamir-index --full` зелёный + workspace clippy (новый pub API).
- fmt/clippy тронутых крейтов `-- -D warnings` чисты.
- Пиллары: spawn_blocking для dump/load (CPU/IO), Store::transact для
  атомарной записи чанков+манифеста где уместно, checksums везде.
- Box::leak — с inline-комментарием (boot-only, осознанно).
- НЕ грепать/пайпать тесты на лету. Импорты в шапке. Раскладка tests/.
- НЕ трогать код вне задачи; НЕ запускать startup-интеграцию (это #401).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи; если
оставил stray-логи в корне — отметь, НЕ удаляй сам.

## Definition of done

- `snapshot.rs` — dump/load кодек: file_dump→чанки+crc32+sidecar(MetaEnvelope)+
  манифест в info_store; load с verify+Box::leak. SnapshotError.
- 5 тестов зелёные (round-trip, битый crc, чужая версия, tombstones,
  rid_map/next_id). `./scripts/test.sh -p shamir-index --full` + workspace
  clippy зелёные.
- Финал: тронутые файлы, форма sidecar/манифеста, размер чанка, как решён
  Box::leak lifetime, вывод гейта, что оставлено на #401/#402.
