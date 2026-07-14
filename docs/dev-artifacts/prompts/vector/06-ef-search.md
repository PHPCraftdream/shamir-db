בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V1.1 — per-query ef_search (+ oversample поле) — wire + адаптер + билдеры Rust/TS

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 1.1 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P1). Это ЕДИНСТВЕННЫЙ лист P1 ($score удалён — см. отдельный трек
> docs/dev-artifacts/design/where-select-binds.md).

## Зачем

Сейчас `ef_search` захардкожен в HnswAdapter (=50 из HnswConfig) → recall
статичен (baseline recall@10 ~0.5–0.7). Клиент должен уметь на КАЖДЫЙ запрос
крутить trade-off recall/latency, не пересоздавая индекс. Плюс заводим ПОЛЕ
`oversample` уже сейчас (семантику включит P3/#404) — чтобы sig `search`
расширить один раз.

## Контекст кода (проверенные факты)

- `crates/shamir-query-types/src/filter/filter_enum.rs:149-155` —
  `Filter::VectorSimilarity { field: FieldPath, query: Vec<f32>, k: u32 }`.
- `crates/shamir-index/src/backend.rs` — `IndexQuery::Vector { vec: Vec<f32>, k: u32 }`.
- `crates/shamir-index/src/vector/adapter.rs` — trait `VectorAdapter::search(
  &self, query: &[f32], k: u32, staged: Option<&[(RecordId, Vec<f32>)]>)`.
- `crates/shamir-index/src/vector/hnsw_adapter.rs` — `search` использует
  `self.ef_search` (поле, =50). hnsw_rs `hnsw.search(query, k, ef)` принимает
  ef ПО-запросно (контракт V0.0 подтвердил).
- `crates/shamir-engine/src/table/read_planner.rs:52-59` — маппинг
  `Filter::VectorSimilarity → IndexQuery::Vector`.
- Билдеры: Rust `crates/shamir-query-builder/` (найди vector_similarity/filter
  билдер), TS `crates/shamir-client-ts/src/core/builders/` (filter.ts —
  vectorSimilarity). Паттерн wire-parity фикстуры — как в replication DDL
  (crates/shamir-query-builder/tests/fixtures/ + TS-тест сверяет msgpack байты).

## Задача (аддитивно по wire — старый msgpack без поля обязан читаться)

1. **Wire-типы:**
   - `Filter::VectorSimilarity { …, ef_search: Option<u32>, oversample: Option<f32> }`
     — оба `#[serde(default, skip_serializing_if = "Option::is_none")]`.
   - Прокинуть через `IndexQuery::Vector { …, ef_search: Option<u32>,
     oversample: Option<f32> }` (или ввести `SearchOpts { ef_search, oversample }`
     — на твоё усмотрение, но аддитивно и чисто).
2. **Адаптер:** расширить `VectorAdapter::search` до приёма ef (напр. новый
   параметр `SearchOpts` или `ef_search: Option<u32>`). HnswAdapter: если задан
   ef → использовать его в `hnsw.search(query, k, ef)` вместо self.ef_search;
   иначе дефолт. BruteForce игнорирует ef (точный поиск) — no-op, документируй.
   `oversample` в этом листе НЕ включает логику (P3), но поле принимается и
   пробрасывается/игнорируется без ошибки — оставь TODO-коммент со ссылкой на
   #404.
   ВНИМАНИЕ: `search` вызывается из нескольких мест (lookup_tx, read_planner) —
   обнови ВСЕ call-site'ы.
3. **Clamp (DoS-защита):** ef_search зажать сверху разумным cap (tunable в
   shamir-tunables ИЛИ константа; сверь, есть ли уже MAX_TOPK-подобное). Слишком
   большой ef → clamp, не паника.
4. **Билдеры + parity:**
   - Rust: метод `.ef_search(n)` / `.oversample(f)` на vector-similarity
     билдере.
   - TS: `.efSearch(n)` / `.oversample(f)` (camelCase) в filter-билдере.
   - Wire-parity фикстура (Rust↔TS byte-identical msgpack), паттерн replication
     DDL — расширь существующую vector-фикстуру или добавь новую.

## Тесты (TDD red-first)

- **serde back-compat:** старый msgpack VectorSimilarity БЕЗ ef_search/oversample
  десериализуется (поля = None). Round-trip с полями.
- **ef влияет на recall:** на 10k графе ef=400 даёт recall ≥ ef=16 (статистич.,
  как существующие recall-тесты; не строгий, а монотонность).
- **clamp:** ef=u32::MAX → не паника, зажат.
- **billing call-sites:** engine-путь (read_planner→lookup) прокидывает ef.
- **TS:** vitest — билдер продуцирует правильный wire; parity-фикстура
  byte-identical с Rust.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector --full`
  зелёный + `./scripts/test.sh -p shamir-query-types -p shamir-query-builder`
  + TS vitest (`crates/shamir-client-ts`, npm test — если окружение позволяет;
  если нет — отметь, что требует node-окружения, но код напиши + фикстуру).
- fmt/clippy на ВСЕ тронутые крейты `-- -D warnings`; workspace clippy (сигнатура
  search меняется → проверь всех call-site'ов).
- Wire — только аддитивные optional поля; ничего не ломать в существующих
  вариантах enum.
- Запросы строить ТОЛЬКО через билдеры (правило CLAUDE.md), исключения — только
  serde-round-trip тесты и napi-граница.
- Импорты в шапке; раскладка tests/.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи.

## Definition of done

- `ef_search` + `oversample` (поле) аддитивно в wire, прокинуты до hnsw.search;
  clamp; все call-site'ы обновлены; oversample-логика отложена в #404 (TODO).
- Билдеры Rust `.ef_search()` + TS `.efSearch()` + wire-parity фикстура.
- Тесты (serde back-compat, ef→recall монотонность, clamp, engine call-site,
  TS parity) зелёные.
- Финал: тронутые файлы, форма SearchOpts/сигнатуры, cap ef, как решён TS-прогон,
  вывод гейта.
