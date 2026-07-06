בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-8 — delete-гонка с флипом (Б-6) + fit в spawn_blocking (О-2) (#430)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #430.
> Область: `crates/shamir-index/src/vector/hnsw_adapter.rs`.

## Находка 1 — Б-6 (LOW): delete-гонка с флипом

`delete()` (~:2247-2263):
```rust
async fn delete(&self, rid: RecordId) -> Result<(), VectorError> {
    if let Some(internal) = self.rid_to_internal.read_async(&rid, |_, v| *v).await {
        if self.deleted.insert_async(internal, ()).await.is_ok() {
            self.deleted_count.fetch_add(1, Ordering::Relaxed);
            self.bump_migrated_on_tombstone(internal);
        }
        let _ = self.rid_to_internal.remove_async(&rid).await;
        let _ = self.vectors.remove_async(&internal).await;
        // V5.2 (#411) — also drop codes from the u8 buffer post-fit.
        if self.quantized_active() {
            let _ = self.vectors_u8.remove_async(&internal).await;
        }
    }
    Ok(())
}
```

Проблема: `self.quantized_active()` читается ОДИН РАЗ, а между этим чтением и
`vectors_u8.remove_async` нет барьера. Если конкурентный `try_fit_and_rebuild`
делает флип (`quantized_active()` false→true) ИМЕННО в этом окне — `delete()`
видит `false`, пропускает `vectors_u8.remove_async`, а параллельный
`claim_and_publish_u8` для этого же internal (снапшот/дельта-пасс) МОГ УЖЕ
вставить коды в `vectors_u8` до того, как delete прочитал tombstone. Итог:
код остаётся в `vectors_u8` навсегда — `deleted`-множество его фильтрует на
search (invisible), но память не освобождается до следующей компакции.

**Фикс**: после `deleted.insert_async` (уже произошёл tombstone — это
happens-before точка), нужно ГАРАНТИРОВАННО убрать код из `vectors_u8`
НЕЗАВИСИМО от текущего `quantized_active()` — то есть просто ВСЕГДА вызывать
`vectors_u8.remove_async(&internal).await` (не под условием), а не только
когда `quantized_active()==true`. `remove_async` на отсутствующем ключе —
no-op (проверь семантику scc::HashMap::remove_async — должна быть такой).
Убери условие `if self.quantized_active()` полностью, оставь безусловный вызов.
Если это меняет какой-то инвариант (например где-то предполагается, что
`vectors_u8` пуст до первого fit) — проверь и опиши в докладе, но по всей
видимости remove на несуществующем ключе безопасен и корректен в обоих
режимах (до и после fit).

Регресс-тест: смоделируй гонку — tombstone internal ИМЕННО в окне между
claim (snapshot/delta pass кладёт код в vectors_u8) и удалением; после этого
assert что `vectors_u8` НЕ содержит internal (или что после компакции размер
соответствует ожидаемому). Если гонку трудно детерминировать напрямую —
минимум: unit-тест что `delete()` теперь безусловно чистит `vectors_u8`
(до и после флипа), плюс существующие concurrency-стресс-тесты
(`concurrent_upsert_with_tombstone_across_fit_does_not_hang` и т.п.) остаются
зелёными.

## Находка 2 — О-2: `Sq8Quantizer::fit` вне spawn_blocking

`try_fit_and_rebuild` (~:1127, внутри асинхронной функции) вызывает:
```rust
let quantizer_arc = Arc::new(Sq8Quantizer::fit(&training, dim));
```
CPU-bound (O(N×dim) вычисление min/max/scale по training set, потенциально
тысячи векторов) выполняется ПРЯМО на async executor thread — блокирует
tokio worker, нарушает пиллар №2 (CPU-bound → `spawn_blocking`).

**Фикс**: вынеси `Sq8Quantizer::fit(&training, dim)` в
`tokio::task::spawn_blocking`. `training: Vec<Vec<f32>>` уже клонирован
(владеющий), можно `move` в blocking-closure без заимствований извне.
Верни `Arc::new(...)` из блокирующей задачи, `.await` результат снаружи —
паттерн уже есть в этом же файле (см. `hnsw_u8.search` внутри
`spawn_blocking` несколькими функциями выше, например в
`search_quantized_graph`). НЕ переноси весь цикл квантования+claim в
spawn_blocking — там есть async-вызовы (`claim_and_publish_u8`), оставь их
снаружи как есть; только сам `fit()`-вызов (чистый CPU, никаких await
внутри) должен уйти в blocking pool.

Также проверь, есть ли квантование СНАПШОТА при компакции (`backfill_if_absent`
/ `new_compaction_target_quantized` путь, VR-6) — если там тоже прямой вызов
`Sq8Quantizer::fit` (переобучение на пост-компакционных данных, как реализовано
в VR-6), он ДОЛЖЕН получить тот же фикс (это, вероятно, тот же самый
`try_fit_and_rebuild`, переиспользуемый — если так, фикс автоматически
покрывает оба места; если fit вызывается where-то ещё отдельно — заверни и там).

## Тесты

Существующие concurrency/fit тесты (`quantized_graph_tests.rs`,
`compaction_tests.rs`) не должны сломаться — семантика fit не меняется, только
поток выполнения. Новый regression-тест на Б-6 (см. выше).

## Гейт

- `./scripts/test.sh @vector --full` 1×;
- `cargo clippy -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt -p shamir-index -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: только hnsw_adapter.rs +
его тесты. Не трогай другие файлы. stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Б-6: `delete()` безусловно чистит `vectors_u8` независимо от
`quantized_active()`, регресс-тест зелёный. О-2: `Sq8Quantizer::fit`
(везде, где вызывается на CPU-bound пути) обёрнут в `spawn_blocking`,
существующие тесты зелёные. Гейт зелёный. Финал: точные diff-места, вывод
тестов, вывод гейта.
