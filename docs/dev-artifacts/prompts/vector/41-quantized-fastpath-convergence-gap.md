בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Post-VR-8 — quantized-fast-path convergence undercounting (#433)

> Ты — агент `@oh` в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Область:
> `crates/shamir-index/src/vector/hnsw_adapter.rs` (методы `upsert`,
> `upsert_batch`, `backfill_if_absent`) + их тесты.

## Контекст находки

Во время финального 10×-стресс-прогона VR-конвейера (`@vector @engine
--full`) нашли и исправили два бага в `hnsw_adapter.rs`:

1. **Catch-up loop priority inversion** (исправлено, коммит ещё не сделан
   в момент написания брифа) — неограниченный hot-spin `yield_now()` в
   catch-up loop `try_fit_and_rebuild` (~L1420-1495) под насыщенной машиной
   голодал blocking-pool потоки, растягивая конвергенцию с микросекунд до
   180s test-kill. Исправлено bounded spin (32 итерации) + `sleep(100µs)`
   backoff (`CATCHUP_SPIN_BUDGET`/`CATCHUP_BACKOFF`, ~L328).

2. **`backfill_if_absent` f32-path self-migration re-check без guard**
   (исправлено) — self-migration re-check (~L812-838) не проверял
   `!self.deleted.contains(&internal)` перед claim, в отличие от
   идентичного паттерна в `upsert` (~L2142) — открывало двойной счёт
   `migrated_pre_flip` при гонке delete-vs-backfill на одном rid. Guard
   добавлен, зеркалируя `upsert`.

При написании regression-теста на находку #2
(`backfill_delete_same_rid_race_no_double_count`,
`crates/shamir-index/src/vector/tests/compaction_tests.rs`) обнаружен
**ТРЕТИЙ, более глубокий и ПРЕДСУЩЕСТВУЮЩИЙ баг** (не введён ни VR-7, ни
VR-8) — тест стабильно (5/5 прогонов) уходит в TIMEOUT/hang.

## Находка #3 — quantized_active()-fast-path пропускает claim, undercounting migrated_pre_flip

### Механизм (мой анализ, ТРЕБУЕТ твоей проверки/углубления)

И `upsert()` (~L1927+), и `upsert_batch()` (аналогичный паттерн), и
скопированный мной в `backfill_if_absent` (~L790-795) код содержат
структуру:

```rust
let internal = self.next_id.fetch_add(1, Ordering::Relaxed);
// ... entry_async / rid_to_internal логика (содержит .await) ...

if self.quantized_active() {
    let codes = self.quantize_and_insert_u8(internal, vec).await?;
    let _ = self.vectors_u8.insert_async(internal, codes).await; // RAW insert
    let _ = self.rid_map.insert_async(internal, rid).await;
    return Ok(()); // upsert() / continue (backfill)
}

// ... f32 path + self-migration re-check (ГАРДИРОВАННЫЙ claim) ...
```

**Проблема**: `next_id.fetch_add` происходит ДО проверки
`quantized_active()`. Между этими двумя точками есть `.await` (минимум —
`entry_async`/`rid_to_internal.entry_async`). Если за время этого await
конкурентный `try_fit_and_rebuild` успевает ПОЛНОСТЬЮ отработать (захватить
`next_id_at_flip` — ЗНАЧЕНИЕ БОЛЬШЕ НАШЕГО `internal`, — и выставить
`is_fitted = true`), то когда наш вызов резюмируется и проверяет
`quantized_active()`, видит `true` — и уходит в "уже квантован" ветку.

Эта ветка вызывает `quantize_and_insert_u8` (вставляет граф-ноду напрямую)
и делает **сырой** `vectors_u8.insert_async` — НЕ через
`claim_and_publish_u8[_async]` (CAS-based claim, который единственный
бампает `migrated_pre_flip`). Значит, для НАШЕГО `internal` (который
`< next_id_at_flip`, т.е. обязан быть учтён в конвергенции per
`try_fit_and_rebuild`'s catch-up loop) `migrated_pre_flip` НИКОГДА не
бампается через этот путь.

**Результат**: catch-up loop (`migrated_pre_flip >= target`) никогда не
сходится для ЭТОГО internal → loop крутится вечно (bounded spin + sleep
backoff не помогает — это логическая недостача, не тайминг) → TIMEOUT.

### Почему это НЕ проявлялось раньше

Окно гонки требует, чтобы ЦЕЛЫЙ fit-цикл (fit + graph build + начало
catch-up loop, все внутри `try_fit_and_rebuild`) уложился ВНУТРИ одного
await-suspend другого вызова (между его `fetch_add` и его
`quantized_active()` проверкой). Это редко, но реально — существующие
concurrency-тесты (`concurrent_upsert_across_fit_no_f32_graph_absent_error`
и др.) недостаточно нагружали планировщик, чтобы гарантированно поймать это
окно. Мой новый regression-тест на находку #2 (delete-воркеры, конкурирующие
за один OS-поток на `#[tokio::test]` current_thread runtime) расширил окно
и стабильно (5/5) его ловит.

### Твоя задача

1. **Подтверди или опровергни** мой анализ — прочитай реальный код
   `upsert`, `upsert_batch`, и мой фикс в `backfill_if_absent` (все три
   имеют одинаковую структуру "quantized_active() fast-path перед f32
   path + guarded self-migration re-check"). Если я ошибся в механизме —
   найди РЕАЛЬНУЮ причину зависания
   `backfill_delete_same_rid_race_no_double_count` (см. тест в
   `compaction_tests.rs`, конец файла — 30s `tokio::time::timeout` guard,
   стабильно ловит FAIL/timeout).

2. **Спроектируй и реализуй правильный, красивый, эффективный фикс.**
   Варианты для рассмотрения (выбери лучший, обоснуй):
   - **A.** В "уже квантован" fast-path (во всех трёх местах) заменить
     сырой `vectors_u8.insert_async` на `claim_and_publish_u8[_async]`,
     плюс guard `!self.deleted.contains(&internal)` (зеркалируя
     self-migration re-check паттерн) — так этот путь тоже участвует в
     конвергенции корректно (claim + bump), не теряя ни одного internal.
     Нужно решить: что делать, если claim ПРОИГРАН (значит фиттер уже
     сам вставил этот internal через свой snapshot/delta/catch-up scan) —
     тогда наш `quantize_and_insert_u8` уже вставил ДУБЛИРУЮЩУЮ графовую
     ноду до проверки claim! Нужно переставить порядок: сначала claim,
     ПОТОМ (если выиграли) — insert графовой ноды, аналогично тому, как
     это уже сделано в guarded self-migration re-check ветке.
   - **B.** Иначе — если видишь более простую архитектурную правку
     (например, унификация: убрать отдельную "quantized_active()
     fast-path" ветку вообще и ВСЕГДА идти через единый guarded путь
     claim-then-insert, что устраняет дублирование кода между тремя
     местами) — реализуй её, но обоснуй trade-off (лишний
     `self.deleted.contains` чек на горячем пути upsert, если он уже
     давно квантован — обычно быстрый lock-free lookup, скорее всего
     приемлемо).
   - Учти: это ГОРЯЧИЙ, многократно рецензированный путь (VR-1..VR-8,
     несколько раундов adversarial review этой сессией). Изменения
     должны быть ХИРУРГИЧНЫМИ, не менять семантику для НЕ-гоночных
     случаев (обычный upsert после того как всё давно сошлось — самый
     частый путь в продакшне — не должен получить лишние аллокации/локи
     заметно дороже текущего).

3. **Тесты**: обнови/адаптируй мой новый
   `backfill_delete_same_rid_race_no_double_count` (сейчас has 30s
   timeout guard, стабильно фейлит) — после фикса должен ПРОХОДИТЬ.
   Добавь АНАЛОГИЧНЫЙ regression-тест для `upsert()` (и/или
   `upsert_batch()`, если тот же паттерн там тоже есть) — статистическая
   гонка, аналогичная существующим `concurrent_upsert_*`-тестам в
   `quantized_graph_tests.rs`, с `tokio::time::timeout` guard (соблюдай
   конвенцию — 30-60s, см. `concurrent_upsert_with_tombstone_across_
   fit_does_not_hang` как образец).

4. **Существующие тесты не должны сломаться** — особенно
   `concurrent_upsert_across_fit_no_f32_graph_absent_error`,
   `concurrent_upsert_with_tombstone_across_fit_does_not_hang`,
   `concurrent_same_rid_upsert_race_across_fit_no_double_count`,
   `stress_concurrent_mutations_during_quantized_compaction` — прогони их
   МНОГОКРАТНО (5-10 раз) после фикса, не полагайся на 1 зелёный прогон.

## Гейт

- `./scripts/test.sh @vector --full` 1× (полный), плюс целевые тесты
  5-10× повторно (см. п.4 выше);
- `cargo clippy -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt -p shamir-index -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: только
`hnsw_adapter.rs` (upsert/upsert_batch/backfill_if_absent) + их тесты.
Пиллары: lock-free, guard не через await, переиспользуй существующий
claim-механизм (`claim_and_publish_u8`/`claim_and_publish_u8_async`), не
дублируй логику там где можно вынести в общий helper. stray-логи отметь,
не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Root cause подтверждён (или скорректирован, если анализ ошибочен).
Правильный, эффективный, хирургичный фикс во ВСЕХ местах с этим
паттерном (upsert/upsert_batch/backfill_if_absent). Regression-тесты
для КАЖДОГО затронутого пути, все с hang-guard (`tokio::time::timeout`).
Существующие тесты зелёные при многократных прогонах. Гейт зелёный.
Финал: точное объяснение root cause, выбранная архитектура фикса и
обоснование, список изменённых мест, вывод тестов (включая счётчики
повторных прогонов), вывод гейта.
