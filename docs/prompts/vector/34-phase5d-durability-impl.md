בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-4 — реализация durability Phase 5d, вариант A (#426)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #426:
> реализовать УТВЕРЖДЁННЫЙ вариант A из
> `docs/design/vector-phase5d-durability.md` (коммит 8d67a710). Прочитай
> дизайн ЦЕЛИКОМ — эскиз реализации (§5) и тестов (§6) обязательны к
> следованию; отклонения только с обоснованием в финале.

## Суть (из дизайна)

- Разделить `apply_vector_batch` (commit_phases.rs:453-517) на
  `apply_vector_graph_batch` (deletes+staged promote, остаётся post-lock в
  promote_vectors) и `apply_vector_delta_batch` (append_vector_delta +
  trigger_snapshot_check, переносится PRE-publish).
- Новый `apply_vector_delta_phase(tx, repo, commit_version)` — вызывается
  ДО `version_guard.commit()`: в `materialize.rs` (:59, до :215) и в
  `commit.rs::commit_tx_inner_legacy_async` (:492, между apply_data_phase
  :539 и commit :553).
- Ошибка delta-append → Deferred + warn-лог (детектируемо ДО ack),
  см. §5.3 дизайна.
- vector_backend.rs НЕ меняется. WAL schema НЕ меняется.
  recover_inflight_v2 НЕ меняется. Устаревший комментарий
  commit_phases.rs:250-260 «rebuild-on-open reconciles» обнови — после
  фикса контракт становится «delta durable pre-publish; restore_on_open
  применит».

## Тесты (по §6 дизайна)

1. Crash-seam `phase5d_delta` в crash_recovery.rs (child-process +
   существующий механизм SHAMIR_TEST_CRASH_AFTER/abort): kill сразу ПОСЛЕ
   записи дельта-чанка, но ДО publish → после рестарта restore_on_open
   применяет чанк, вектор ищется. И симметричный: kill ДО delta-append →
   как сегодня (мутация не видна, tx не acked — допустимо).
2. Негативный: инъекция провала delta-append (fail-injection по образцу
   существующих) → tx возвращает Deferred, ack не даёт ложного успеха.
3. Регресс happy-path: существующие tx_vector_delete_tests /
   commit_phase5_tests / persist-тесты зелёные без изменений семантики.

## Гейт

- `./scripts/test.sh @vector @engine --full` 1×;
- `./scripts/test.sh -p shamir-engine --full -- crash_` (crash-сьют);
- `cargo clippy -p shamir-engine -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt` тронутых `-- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh (вывод в файл → grep файла).
Хирургично: файлы из §5.1 дизайна + crash_recovery.rs. ⚠️ НЕ трогать
`crates/shamir-index/src/vector/hnsw_adapter.rs` и
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs` — там работают
другие агенты. Пиллары: guard не через await, импорты в шапке.
stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Delta-append выполняется pre-publish на обоих путях (materialize +
legacy_async), crash-seam тест доказывает выживание мутации, негативный
Deferred-тест зелёный, устаревшие комментарии обновлены, гейт зелёный.
Финал: механика, crash-тест вывод (до/после), вывод гейта.
