בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V4.2 — фоновая компакция tombstone (double-write + backfill + ArcSwap swap)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 4.2 (#408, фаза P4). ДИЗАЙН УЖЕ НАПИСАН И УТВЕРЖДЁН:
> `docs/design/vector-compaction.md` — прочитай его ЦЕЛИКОМ и следуй ему
> буквально. Ниже — только акценты и требования к гейту. При расхождении
> дизайн-док — источник истины по механизму.

## Что делаешь (кратко; детали — в дизайн-доке)

Фоновая компакция HNSW: при высокой доле tombstone'ов построить свежий
`HnswAdapter` без мёртвых узлов (rebuild-aside), атомарно подменить через
`ArcSwap<AdapterSlot>` (RCU), форс-снапшот. Корректность под конкурентными
мутациями (и tx, и non-tx CRUD/репликация) — через **double-write +
backfill-if-absent + reconcile-deletes**, БЕЗ потери и БЕЗ ghost.

## Абсолютно критичные точки (где рождаются баги — не срежь)

1. **Механизм — double-write, НЕ delta-replay.** Delta-log содержит ТОЛЬКО
   tx-мутации (`append_vector_delta` зовётся лишь из `commit_phases.rs:458`).
   Non-tx путь (`plan_insert/update/delete` из `table_manager_crud.rs` и
   `table_manager_replication.rs`) в delta-log НЕ пишет. Поэтому во время
   компакции КАЖДЫЙ из 4 мутационных сайтов дублирует op в
   `compaction_target: Arc<ArcSwapOption<AdapterSlot>>` (дизайн §3, §8):
   - `plan_insert`, `plan_update`, `plan_delete`, `apply_staged_vectors`.
   - Паттерн: после primary op на `load_full().adapter` — `if let Some(t) =
     self.compaction_target.load_full() { t.adapter.upsert/delete(..).await }`.
     Guard `ArcSwapOption::load()` НЕ держать через await — извлечь Arc и
     дропнуть guard (используй `load_full()` → Option<Arc<AdapterSlot>>).
   - Idle-overhead (компакция не идёт): один atomic load → None → ветка не
     берётся. Обязано быть ~0.
2. **Порядок протокола (дизайн §3) НЕПРИКОСНОВЕНЕН:** new(пустой) → взвести
   double-write (Step2) → collect S0 из old (Step3) → **Step4a backfill_if_absent(S0)**
   → **Step4b reconcile-deletes** (переиграть ВСЕ `compaction_deleted_rids` как
   delete на new — ОБЯЗАТЕЛЬНО, закрывает backfill↔delete resurrect-гонку) →
   **Step5 swap (arc_swap.store(new))** → **Step6 clear (compaction_target.store(None))**
   → Step7 форс-снапшот. Порядок Step5→Step6 (swap ПЕРЕД clear) закрывает
   хвостовую гонку идемпотентностью; менять НЕЛЬЗЯ (дизайн §4, §10.9).
3. **`compaction_deleted_rids` (дизайн §10.4):** `delete` в `HnswAdapter`
   убирает rid из `rid_to_internal`, поэтому backfill не отличит «не было» от
   «удалён во время double-write». Нужен доп. tombstone-set по RID в
   target-адаптере (напр. `Option<Arc<scc::HashMap<RecordId,(),THasher>>>` поле
   в HnswAdapter, Some только у compaction-target), пополняемый на каждом
   double-write delete; `backfill_if_absent` и Step4b его читают. Продумай
   атомарность так, чтобы Step4b гарантированно затирал любой resurrect.
4. **Single-flight + координация со снапшотом (дизайн §5):**
   `compaction_in_flight: Arc<AtomicBool>` + `CompactionFlightGuard` drop-guard
   (зеркало `SnapshotFlightGuard` — сбрасывает флаг на Ok/Err/panic). Компакция
   и фоновый снапшот НЕ должны идти одновременно: `trigger_compaction_check`
   пропускает если `snapshot_in_flight`; `trigger_snapshot_check` пропускает
   если `compaction_in_flight` (добавь эту проверку).
5. **Триггер (дизайн §5):** `deleted_ratio() >= VECTOR_COMPACTION_RATIO_THRESHOLD`
   (tunable, 0.3) И `live_count() >= VECTOR_COMPACTION_MIN_LIVE` (tunable, 1000).
   Вызывается на ack-пути (там же где `trigger_snapshot_check`) + после non-tx
   `plan_delete`. Новые tunables — в `shamir-tunables` (по образцу
   `VECTOR_SNAPSHOT_DELTA_THRESHOLD`). Тест-override порога как
   `set_snapshot_threshold_for_test`.
6. **Форс-снапшот после swap (дизайн §6):** от НОВОГО адаптера, `delta_count=0`.
   Crash-safety не ухудшается (§6, §10.7).

## Тесты (TDD; дизайн §9) — ОБЯЗАТЕЛЬНЫ, особенно стресс

Раскладка `crates/shamir-index/src/vector/tests/` (новый файл
`compaction_tests.rs`, зарегистрировать в `tests/mod.rs`). Строй запросы/данные
через существующие хелперы.
- unit: `collect_live_vectors` (только не-tombstone), `backfill_if_absent`
  (skip existing / skip deleted-rid / insert absent), `should_compact` пороги.
- integration: rebuild-aside (`deleted_count(new)==0`, live совпал);
  double-write видимость; single-flight no-op; snapshot↔compaction координация;
  backfill ordering (double-write свежее значение сохраняется); delete в
  double-write не воскрешается (Step4b).
- **СТРЕСС (критично, дизайн §9.11-13):** N async-задач гоняют случайные
  upsert/delete (микс tx-промоут И non-tx `plan_insert/plan_delete`) ПАРАЛЛЕЛЬНО
  с запущенной компакцией. После swap+clear: live-set(new) ТОЧНО равен
  ожидаемому (0 потерь, 0 ghost); все rid в `rid_to_internal(new)` живые; поиск
  во время компакции не паникует и возвращает только валидные rid. Прогони под
  нагрузкой (эти гонки всплывают под nextest-параллелизмом — если тест
  флапает/висит, это НАСТОЯЩИЙ баг гонки, чини корень, НЕ поднимай таймаут).

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test). Гейт:
  `./scripts/test.sh @vector --full` (+ `@engine --full` — трогаешь
  vector_backend, который engine дёргает на commit-пути). Прогони стресс
  НЕСКОЛЬКО раз (луп), чтобы поймать флап.
- `cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings`;
  `cargo fmt -p shamir-index -p shamir-engine -- --check`.
- Пиллары: lock-free (ArcSwapOption/ArcSwap/atomics, без std::sync Mutex на
  hot-path), `spawn_blocking` для CPU-bound graph build, без O(N²), guard не
  через await. Импорты в шапке. Один основной экспорт на файл (compaction —
  методы на VectorBackend/HnswAdapter в их файлах; drop-guard рядом с
  SnapshotFlightGuard). НЕ трогай код вне задачи.
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- double-write на 4 сайтах (idle ~0), `compaction_target` ArcSwapOption,
  `compaction_deleted_rids` tombstone-set, протокол Step1-7 включая Step4b
  reconcile, single-flight+drop-guard+координация, триггер+tunables,
  форс-снапшот.
- Все тесты (unit/integration/СТРЕСС) зелёные, стресс стабилен под лупом.
- `./scripts/test.sh @vector @engine --full` + workspace-clippy тронутых +
  fmt зелёные.
- Финал: тронутые файлы, как реализован double-write и где дропается guard,
  как Step4b закрывает resurrect-гонку, доказательство что idle-overhead ~0,
  вывод стресс-теста (сколько прогонов, стабилен ли), вывод гейта, что
  оставлено на #409 (бенчи компакции).
