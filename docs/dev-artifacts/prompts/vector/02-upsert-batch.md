בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V0.2 — upsert_batch в VectorAdapter (parallel_insert, батчевый rebuild)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), рабочая директория
> D:\dev\rust\shamir-db. Реализуешь лист 0.2 плана
> `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`.

## Контекст (проверенные факты из спайка V0.0, коммит 5ec84564)

Контракт-тесты в `crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs`
установили реальный API hnsw_rs 0.3.4:
- `Hnsw::parallel_insert(&self, datas: &[(&Vec<T>, usize)])` — Rayon-параллельная
  вставка. ВАЖНО: тип аргумента — `&[(&Vec<T>, usize)]` (ссылка на `Vec`, НЕ
  слайс; для слайсов есть отдельный `parallel_insert_slice`).
- Контракт batch-вставки: `get_nb_point() == n` после батча (все точки
  физически в графе). Self-search НЕ гарантируется (HNSW approximate +
  unseedable RNG) — не используй его как проверку.

Текущий код:
- `crates/shamir-index/src/vector/adapter.rs` — trait `VectorAdapter { upsert,
  delete, search, dim, len, apply_committed_vectors }`.
- `crates/shamir-index/src/vector/hnsw_adapter.rs` — `HnswAdapter::upsert`
  (строки ~138-205) использует `entry_async` для сериализации per-rid +
  `spawn_blocking` для CPU-bound graph insert. ВАЖНО: там есть D12-инвариант
  (защита от гонки конкурентных upsert одного rid → без дублей в графе) —
  СОХРАНИ его логику при батче.
- `crates/shamir-index/src/vector/vector_backend.rs::rebuild` (~246-271) —
  полный скан store, upsert chunks по 64 ПОСЛЕДОВАТЕЛЬНО. Перевести на батч.

## Задача

1. **`upsert_batch` в trait `VectorAdapter`** (`adapter.rs`): default-метод
   `async fn upsert_batch(&self, items: &[(RecordId, Vec<f32>)]) -> Result<(),
   VectorError>` с наивной реализацией (loop по `self.upsert`) — чтобы другие
   адаптеры (BruteForce) не ломались.
2. **Override в `HnswAdapter`** (`hnsw_adapter.rs`):
   - Валидация: ВСЕ dim проверить ЗАРАНЕЕ (до вставки); при несовпадении —
     вернуть `Err(DimMismatch)`, НИ ОДНОГО не вставлять (атомарность валидации).
   - Клейм internal-слотов/rid-map через существующий `entry_async`-протокол,
     СОХРАНЯЯ D12-инвариант (старый internal тумбстонится, конкурентный
     upsert того же rid не создаёт дубль). Продумай: как сериализовать per-rid
     клейм для батча, потом один `spawn_blocking` с `parallel_insert` по всем
     новым (rid,vec). Обнови `vectors`-map/`rid_map`/`rid_to_internal`/`next_id`
     согласованно.
   - Один `spawn_blocking` на весь батч (не N).
3. **`rebuild` на батч** (`vector_backend.rs`): вместо chunks-по-64-
   последовательно — собирать батч и звать `adapter.upsert_batch`. Разумный
   размер батча (напр. тот же 1000 из store-скана, или крупнее) — обоснуй.

## Тесты (TDD, red-first; раскладка tests/ уже есть — hnsw_adapter_tests.rs)

- **батч из N → search видит все**: вставить батч, `len()==N`, поиск возвращает
  ожидаемых соседей (на N>256 — HNSW-путь; recall vs brute-force порог, как в
  существующих тестах).
- **dim-mismatch в середине батча → ни один не применён**: батч, где один
  вектор неверной длины → `Err(DimMismatch)`, `len()` не изменился
  (атомарность валидации).
- **D12-регрессия**: конкурентный `upsert_batch` + `upsert` того же rid →
  в выдаче нет дубля этого rid (переиспользуй/расширь существующий D12-тест
  в hnsw_adapter_tests.rs, строки ~459-542).
- **rebuild через батч**: rebuild из store с M записями → граф содержит все M.

## Дисциплина репозитория (ОБЯЗАТЕЛЬНО)

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт листа:
  `./scripts/test.sh -p shamir-index --full` зелёный.
- fmt `cargo fmt -p shamir-index -- --check` чист; clippy
  `cargo clippy -p shamir-index --all-targets -- -D warnings` чист.
- Пиллары: lock-free (scc/atomics/ArcSwap как в текущем адаптере),
  `spawn_blocking` для CPU-bound, batched+amortized, `THasher`.
- НЕ грепать/пайпать вывод тестов на лету — писать в файл, потом grep.
- Импорты в шапке; раскладка tests/; НЕ трогать код вне задачи.
- Публичные сигнатуры trait меняешь аддитивно (default-метод) — downstream
  крейты не должны сломаться; проверь `cargo clippy --workspace --all-targets`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи.

## Definition of done

- `upsert_batch` (default + HnswAdapter override с parallel_insert, атомарная
  валидация dim, D12 сохранён); `rebuild` на батч.
- 4 теста зелёные; `./scripts/test.sh -p shamir-index --full` + workspace
  clippy зелёные.
- Финал: тронутые файлы, как сохранён D12-инвариант при батче, размер
  rebuild-батча и обоснование, вывод гейта.
