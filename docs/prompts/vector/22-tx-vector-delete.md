בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V-fix — провести tx-committed vector deletes в HNSW-граф + delta (gap#1)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Реализуешь
> задачу #416 — pre-existing HIGH-6 gap#1. Сейчас tx-удаление строки с
> vector-индексом НЕ доходит до HNSW-графа: вектор остаётся ghost'ом в
> ANN-поиске (И до рестарта — живой граф не тумбстонит, И после — в delta нет
> `DeltaOp::Delete`). Vector search возвращает `RecordId` удалённой записи.
> Задача: провести tx-delete до графа + durable delta. Механизм уже доказан.

## Корень (проверенные факты)
- `crates/shamir-index/src/vector/vector_backend.rs`: `plan_delete_tx` при
  `tx_id.is_some()` — **no-op на живом графе** (по образцу insert-стейджинга,
  но для delete НЕТ стейджа). `plan_insert_tx`/`plan_update_tx` стейджат
  вектор через `staged_vector` → `TxContext::stage_vector` →
  `staged_vectors: TFxMap<u64, Vec<(RecordId,Vec<f32>)>>`.
- `crates/shamir-tx/src/tx_context.rs`: `staged_vectors` (стр.117) — per-table
  insert'ы, ожидающие commit. **НЕТ поля staged_vector_deletes.**
- `crates/shamir-engine/src/tx/commit_phases.rs` `apply_vector_batch`
  (Phase 5d): цикл по backend'ам → `apply_staged_vectors(vecs)` (промоут
  insert'ов) → `append_vector_delta(&info_store, vecs, &[])` — **`deleted=&[]`**
  (вот дыра). Комментарий у вызова прямо описывает вариант-A фикс.
- **Механизм доказан**: `delta_log_tests::append_vector_delta_with_deleted_
  slice_persists_and_replays_delete` фиксирует, что
  `append_vector_delta(.., deleted=[rid])` пишет `DeltaOp::Delete`, применяемый
  при restart. `HnswAdapter::delete(rid)` тумбстонит на живом графе.

## Задача (вариант A — зеркалит insert-путь)
1. **TxContext**: добавь `staged_vector_deletes: TFxMap<u64, Vec<RecordId>>`
   (per-table rid'ы удаляемых vector-строк), инициализация в `new`/Default,
   учти в `is_empty()`/агрегатах где перечисляются staged_vectors (стр.407,
   422, etc.), RAII-очистка при abort (Drop tx → discard, как staged_vectors —
   ghost на живом графе не появляется до commit). Аксессор
   `stage_vector_delete(token, rid)` + `staged_vector_deletes_for(token)`.
2. **VectorBackend::plan_delete_tx**: при `tx_id.is_some()` — вместо no-op
   застейджить rid в `TxContext::staged_vector_deletes` (по образцу
   `staged_vector`/`plan_insert_tx`). При `tx_id.is_none()` (non-tx) — прежний
   `plan_delete` (немедленный graph delete) не трогать.
3. **Phase 5d commit** (`apply_vector_batch`): собери staged deletes для
   таблицы (из TxContext), для каждого backend'а:
   - применить graph-side delete: для каждого удаляемого rid вызвать живой
     `adapter.delete(rid)` (через backend — по образцу apply_staged_vectors
     промоута; под commit-lock, после swap). Тумбстонит вектор в графе.
   - `append_vector_delta(&info_store, vecs, &deleted_rids)` — заменить `&[]`
     на собранный slice → durable `DeltaOp::Delete`, реплеится при restart.
   Порядок: промоут insert'ов + delete'ов согласованно (replace = delete old +
   insert new — не потерять). Убери устаревший gap#1-комментарий у вызова,
   замени на описание реализованной проводки.
   Сигнатуру `apply_vector_batch` при необходимости расширь (доступ к
   staged_vector_deletes через repo/tx — как получается `vecs`).
4. **Double-write при компакции (#408)**: delete-промоут в Phase 5d идёт через
   backend'ов `plan_*`/adapter — если компакция взведена, delete должен
   продублироваться в compaction_target (проверь: idёт ли delete-промоут через
   путь, покрытый double-write из #408; если apply-delete в обход — добавь).

## Тесты (регресс — обязательно; принцип «каждый баг → тест»)
- **ghost-фикс (главный)**: tx вставляет vector-строки, commit; затем tx
  УДАЛЯЕТ одну, commit; ANN-search НЕ возвращает удалённый rid (жив-граф
  тумбстонит) — И ДО рестарта, И ПОСЛЕ (cold-start: рестарт из снапшота+delta,
  search не возвращает удалённый). Это прямой регресс на gap#1.
- abort: tx удаляет vector-строку, ROLLBACK (drop tx без commit) → вектор
  ОСТАЁТСЯ в графе (staged delete отброшен RAII, ghost не появился).
- replace через tx (delete+insert того же rid) → search возвращает НОВЫЙ
  вектор, не старый и не оба.
- back-compat: non-tx `plan_delete` (немедленный) работает как раньше;
  таблицы без vector-индекса не задеты.
- delta round-trip: staged delete → `DeltaOp::Delete` в delta-log → replay при
  restart тумбстонит (переиспользуй/расширь существующий delta-тест).

## Дисциплина + гейт
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector @engine --full` (несколько раз — tx-commit
  concurrency); `cargo clippy -p shamir-index -p shamir-engine -p shamir-tx
  --all-targets -- -D warnings`; `cargo fmt` тронутых `-- --check`.
- Пиллары: lock-free, guard не через await, staged-мутации RAII-безопасны на
  abort. Импорты в шапке. Один основной экспорт на файл. НЕ трогать код вне
  задачи (insert-промоут не рефактори сверх согласования с delete).
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- TxContext.staged_vector_deletes + plan_delete_tx стейджит + Phase 5d
  промоутит (graph delete + durable DeltaOp::Delete); abort-RAII; компакция
  double-write покрыта.
- Регресс-тесты (ghost до/после рестарта, abort, replace, back-compat, delta
  round-trip) зелёные; существующие tx/vector/persist тесты не сломаны.
- `./scripts/test.sh @vector @engine --full` + clippy + fmt зелёные.
- Финал: тронутые файлы, как проведён tx-delete (стейдж→Phase5d→graph+delta),
  как abort-RAII не оставляет ghost, покрытие компакции, вывод регресс-теста
  ghost до/после рестарта, вывод гейта.
