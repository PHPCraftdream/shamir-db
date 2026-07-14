בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V4.1 — deleted_count-зеркало + deleted_ratio + починка O(N) len() (K8)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 4.1 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P4, первый лист). Это подготовка к фоновой компакции (#408): дать
> O(1) счётчики live/deleted и deleted_ratio, а заодно ЗАКРЫТЬ долг K8 —
> O(N) `len()` на ГОРЯЧЕМ пути.

## Проверенные факты (я прочитал код)

Файл `crates/shamir-index/src/vector/hnsw_adapter.rs`:

- Поле `deleted: scc::HashMap<usize,(),THasher>` (строка 125) — тумбстоны
  внутренних id. `next_id: AtomicUsize` (126) — монотонный аллокатор
  внутренних id (растёт на КАЖДЫЙ insert, включая replace).
- **`len()` (строки 673-676)** сейчас:
  ```rust
  #[allow(clippy::disallowed_methods)] // O(N) ack: ... off hot path
  fn len(&self) -> usize { self.next_id.load(Relaxed) - self.deleted.len() }
  ```
  `self.deleted.len()` — это `scc::HashMap::len()` = **O(N) обход** (запрещён
  `clippy.toml`, задавлен allow'ом). Комментарий «off hot path» — **НЕВЕРЕН**:
  `search()` вызывает `self.len()` на строке 611 (`if self.len() <=
  BRUTE_FORCE_MAX`) на КАЖДЫЙ поисковый запрос. Это горячий O(N). Это и есть K8.
- **Три и только три сайта, где растёт `deleted`** (новый тумбстон):
  1. `upsert` строка 430: `let _ = self.deleted.insert(old_internal, ());`
     (replace — старый internal тумбстонится).
  2. `upsert_batch` строка 515: то же в цикле по батчу.
  3. `delete` строка 563: `let _ = self.deleted.insert_async(internal, ()).await;`
  В сайтах 1-2 `old_internal` — это ЖИВОЙ internal, снятый из
  `rid_to_internal` под `entry_async`-guard'ом (D12-протокол), поэтому он НЕ
  может уже быть в `deleted`. В сайте 3 — аналогично (internal из
  `rid_to_internal`). Т.е. каждый insert реально добавляет НОВЫЙ ключ.
- **`from_parts` (строки 218-240)** восстанавливает `deleted`/`next_id` из
  sidecar при загрузке снапшота — сюда нужно ЗАСИДИТЬ зеркало один раз.
- `apply_committed_vectors` (в `adapter.rs`, default-impl) УЖЕ делегирует в
  `upsert_batch` (батчевый promote сделан в #395) — трогать НЕ надо, только
  подтвердить тестом, что промоут-путь корректно ведёт счётчик.

## Задача

1. **Зеркало `deleted_count: AtomicUsize`** в `HnswAdapter`. Инициализация 0 в
   `new` (строка ~149). В `from_parts` — засидить `AtomicUsize::new(deleted.len())`
   ОДИН раз (это единственный легитимный O(N) на пути загрузки, вне hot-path;
   пометь `#[allow(clippy::disallowed_methods)] // O(N) ack: one-time seed at
   snapshot load`).
2. **Инкремент зеркала на КАЖДОМ из трёх сайтов** — СТРОГО консистентно с
   `deleted`-мапой. `scc::HashMap::insert` возвращает `Result<(),(K,V)>` (Err
   если ключ уже есть). Инкременть `deleted_count` ТОЛЬКО когда insert вернул
   `Ok` (защита от двойного счёта, если под гонкой D12 ключ уже был тумбстонен —
   инвариант говорит что нет, но код обязан быть корректен и в этом случае, а не
   полагаться на инвариант). Пример:
   ```rust
   if self.deleted.insert(old_internal, ()).is_ok() {
       self.deleted_count.fetch_add(1, Ordering::Relaxed);
   }
   ```
   (и `insert_async(..).await.is_ok()` в сайте 3). Ordering::Relaxed достаточно
   — это счётчик-зеркало, не публикующий барьер.
3. **Переписать `len()` в O(1)**: `self.next_id.load(Relaxed) -
   self.deleted_count.load(Relaxed)`. УДАЛИТЬ `#[allow(clippy::disallowed_methods)]`
   и старый комментарий (теперь честный O(1), запрещённый `scc::len()` ушёл).
4. **Accessor'ы для компакции (#408)**: `pub(crate) fn deleted_count(&self) ->
   usize`, `pub(crate) fn live_count(&self) -> usize` (== `len()`), `pub(crate)
   fn deleted_ratio(&self) -> f64` (= `deleted_count as f64 / next_id as f64`,
   при `next_id==0` → 0.0). Off-hot-path, но всё O(1). Это API, который #408
   будет опрашивать порогом.

## Тесты (TDD red-first) — `crates/shamir-index/src/vector/tests/`

Добавь в существующий `hnsw_adapter_tests.rs` (НЕ создавай новый файл без
нужды) группу тестов:
- `len_is_o1_correct_after_churn`: N insert → len==N; M replace тех же rid →
  len всё ещё N (replace не меняет live-кардинальность), deleted_count==M; K
  delete → len==N-K, deleted_count==M+K. Проверь на батч-пути (upsert_batch) И
  одиночном (upsert).
- `deleted_ratio_tracks_tombstones`: после churn ratio == deleted/next_id (с
  допуском на f64), при пустом адаптере == 0.0.
- `deleted_count_mirror_matches_map_len`: инвариант — `deleted_count() ==
  deleted.len()` после произвольной последовательности insert/replace/delete
  (ЕДИНСТВЕННОЕ место, где легитимно позвать `deleted.len()` — в тесте, под
  allow; это проверка, что зеркало не разошлось с истиной).
- `from_parts_seeds_deleted_count`: собери адаптер через from_parts с непустым
  `deleted` → `deleted_count()` совпадает с размером переданной мапы (если
  from_parts трудно вызвать напрямую — через snapshot round-trip: dump→load,
  предварительно натумбстонив несколько записей, и проверь len()/deleted_count
  после загрузки).
- back-compat: существующие тесты `apply_committed_vectors_*`,
  `hnsw_adapter_tests` остаются зелёными; промоут-путь ведёт счётчик.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector --full` (+ `@engine` если тронешь что-то, что
  видит engine — не должен). Плюс `cargo clippy -p shamir-index --all-targets
  -- -D warnings` (важно: после починки len() запрещённый `scc::len()` на
  проде должен ИСЧЕЗНУТЬ — clippy это подтвердит без allow'а).
- `cargo fmt -p shamir-index -- --check`.
- Пиллары: lock-free (Atomic-зеркало по образцу `Drainer::window_depth` /
  `VersionedOverlay::count` — так прямо просит CLAUDE.md про scc len()), Relaxed
  ordering, без новых блокировок. Импорты в шапке. НЕ трогать код вне задачи
  (не рефактори upsert/search сверх инкремента зеркала).
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- `deleted_count: AtomicUsize` зеркало, инкремент на 3 сайтах (только на Ok
  insert), засидка в `from_parts`; `len()` теперь O(1) БЕЗ allow-задавливания
  `scc::len()`; accessor'ы deleted_count/live_count/deleted_ratio для #408.
- Тесты (churn-корректность на обоих путях, ratio, зеркало==мапа, from_parts
  seed, back-compat) зелёные.
- `./scripts/test.sh @vector --full` + `cargo clippy -p shamir-index
  --all-targets -D warnings` (без нового allow на проде) зелёные.
- Финал: тронутые файлы, где инкрементится зеркало и почему Ok-guard, как
  засижена загрузка, подтверждение что горячий `len()` теперь O(1), вывод
  гейта, что оставлено на #408 (сама компакция).
