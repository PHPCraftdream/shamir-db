בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-6 — quantization-aware компакция (П-1) (#428)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #428 —
> MEDIUM находка ревью кампании. Область: `crates/shamir-index/src/vector/
> hnsw_adapter.rs` (`new_compaction_target`, `backfill_if_absent`,
> `collect_live_vectors`, ~:656-768) + `crates/shamir-index/src/vector/
> vector_backend.rs` (`run_background_compaction`, ~:1024-1117).

## Дефект (П-1)

`run_background_compaction` (`vector_backend.rs:1041-1044`) всегда создаёт
target компакции через `HnswAdapter::new_compaction_target(dim, metric,
config)` — обычный (`Self::new`), НИКОГДА не квантованный. `collect_live_vectors`
дегквантует все живые векторы в f32 (комментарий прямо признаёт: «compaction
rebuild target is always an unquantized adapter... a quantization-aware
compaction is #412» — но #412 сделал снапшот-формат v2, а НЕ саму компакцию
quantization-aware, дефект остался). `backfill_if_absent` жёстко пишет в
f32-путь (`self.hnsw.load_full()` + `self.vectors.insert_async`), с
комментарием «backfill runs on a compaction target (always a non-quant
adapter)».

Итог: после ЛЮБОЙ фоновой компакции SQ8-индекс молча теряет квантизацию —
память возвращается к 4× (SQ8 экономия #418 обнуляется), и следующий снапшот
дампится как v1 (без QuantMeta), пока индекс не пересечёт FIT_THRESHOLD
заново на новом (пустом после компакции с точки зрения `next_id`? — нет,
target наследует live-данные, но НЕ квантование) adapter'е и не сработает
deferred-fit ОДНАКО с полностью новым квантайзером, обученным на
пост-компакционном распределении — плюс окно между компакцией и повторным
fit, где память НЕ экономится.

## Задача

1. **`run_background_compaction`**: узнать у `old_hnsw`, квантован ли он
   (нужен аксессор — грепни `is_quantized`/`quantization` поле, добавь
   `pub(crate) fn quantization_kind(&self) -> Option<VectorQuantization>`
   если такого геттера ещё нет). Если `Some(q)` — создать target через
   `HnswAdapter::new_compaction_target_quantized(dim, metric, config, q)`
   (новый конструктор, зеркалящий `new_with_quantization`, но с
   `compaction_deleted_rids` как в текущем `new_compaction_target`).
2. **Решение о квантайзере target'а** (реши сам, задокументируй выбор):
   - **Вариант A (проще, рекомендуется)**: target ВСЕГДА стартует
     неквантованным (f32), backfill кладёт живые (дегквантованные)
     векторы как обычно, и ЕСТЕСТВЕННЫЙ deferred-fit (порог
     FIT_THRESHOLD=256) переобучает квантайзер на пост-компакционном
     живом наборе — если `live_pairs.len() >= FIT_THRESHOLD` уже на
     этапе backfill, это сработает автоматически через существующий
     механизм `upsert`/`upsert_batch`, ЕСЛИ backfill сам вызывает
     `try_fit_and_rebuild`-путь (сейчас `backfill_if_absent` НЕ вызывает
     deferred-fit check — добавь `if self.quantization.is_some() &&
     !self.is_quantized() && self.len() >= FIT_THRESHOLD { let _ =
     self.try_fit_and_rebuild().await; }` в конце backfill, по образцу
     upsert). Это даёт **quantization-aware компакцию БЕЗ переноса
     старого QuantMeta** — переобучение с нуля на живых данных, что
     честнее (пост-компакционное распределение могло дрейфнуть от
     оригинального fit — см. П-4 fit-порог note в guide).
   - Обоснуй в финале, почему выбрал этот вариант (или другой), если
     видишь более простой путь — опиши альтернативу и её trade-off.
3. **`backfill_if_absent`**: должен работать корректно и для
   квантованного, и для неквантованного пути. До первого fit (или если
   `quantization.is_none()`) — текущая f32-логика без изменений. После
   `is_quantized()==true` (если deferred-fit из п.2 уже сработал в
   процессе backfill) — новые вставки должны идти через тот же
   claim/insert-в-u8-граф механизм, что и обычный `upsert` (СМ. VR-1
   `claim_and_publish_u8[_async]` — переиспользуй, не дублируй).
4. **Снапшот после компакции**: `run_background_compaction` Step 7 уже
   форсирует снапшот с `new_adapter_arc` — если target квантован, снапшот
   должен получиться v2 с QuantMeta (проверь, что `run_background_snapshot`
   уже умеет это — она общая для всех адаптеров, вероятно да, но
   подтверди тестом).

## Регресс-тесты

1. Компакция SQ8-индекса (>FIT_THRESHOLD live-векторов, с deletes/
   tombstones до компакции) → assert `new_adapter.is_quantized()` (или
   эквивалент после fit) — target реально квантован, НЕ вечный f32.
2. Assert память: сравни RSS/оценку размера до/после (или хотя бы assert
   `f32_graph_present()==false` после компакции+fit — детерминированный
   memory-инвариант, как в существующих VR-1/#418 тестах).
3. Снапшот после компакции квантованного индекса — v2 формат
   (`SNAPSHOT_FORMAT_VERSION==2`, QuantMeta присутствует), не v1.
4. Search корректен после компакции+рефита (recall smoke, как в
   существующих quantized_graph_tests.rs).
5. Back-compat: компакция НЕквантованного индекса (`quantization: None`)
   не меняется — target остаётся f32 (существующие тесты компакции не
   должны сломаться).

## Гейт

- `./scripts/test.sh @vector @engine --full` 1×;
- `cargo clippy -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt -p shamir-index -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: hnsw_adapter.rs +
vector_backend.rs + их тесты. Пиллары: lock-free, guard не через await,
переиспользуй существующий claim-механизм VR-1, не дублируй. stray-логи
отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Компакция SQ8-индекса производит квантованный target (переобучение с нуля
на живых данных — задокументированное решение), снапшот после — v2,
back-compat неквантованной компакции сохранён, регресс-тесты зелёные, гейт
зелёный. Финал: выбранный вариант (A или иной) с обоснованием, точный
механизм (где добавлен deferred-fit check, как backfill маршрутизирует
между f32/u8), вывод тестов, вывод гейта.
