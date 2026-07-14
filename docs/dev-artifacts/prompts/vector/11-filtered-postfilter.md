בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V3.1 — filtered ANN: план-распознавание + post-filter с oversample

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 3.1 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P3 — filtered ANN, ПЕРВЫЙ лист). Только post-filter+oversample;
> pre-filter/co-filter — #405, бенч — #406.

## Зачем

Сейчас `vector_similarity(..)` — отдельный планировщик-путь; комбинация с
предикатами (`AND city='NYC'`) не работает как ANN-с-фильтром. Нужен базовый
рабочий путь: распознать `And([VectorSimilarity, ...predicates])`, сделать
ANN с запасом (oversample), применить предикаты, добрать если не хватило k.

## Контекст кода (проверенные факты)

- `crates/shamir-query-types/src/filter/filter_enum.rs` — `Filter::And(Vec<Filter>)`,
  `Filter::VectorSimilarity { field, query, k, ef_search: Option<u32>,
  oversample: Option<f32> }` (поле oversample ЗАВЕДЕНО в V1.1, но семантики нет
  — включаешь ЗДЕСЬ).
- `crates/shamir-engine/src/table/read_planner.rs:52-69` — маппит
  VectorSimilarity → IndexQuery::Vector{vec,k,opts:SearchOpts{ef_search,
  oversample}}. Сейчас обрабатывает ТОЛЬКО голый VectorSimilarity; And с
  предикатами вокруг vector — как планируется? Изучи, как read_planner
  разбирает And и где мог бы распознать «vector + предикаты».
- `crates/shamir-index/src/vector/{adapter,hnsw_adapter}.rs` — `search(query, k,
  opts: SearchOpts, staged)`; `SearchOpts.oversample: Option<f32>`.
- `MAX_TOPK` в hnsw_adapter.rs (=10_000) — cap.
- Как предикаты применяются к записям после индекс-lookup: изучи read-путь
  (read_exec.rs / где rids из индекса материализуются в записи и фильтруются
  остаточными предикатами). Post-filter = применить остаточные предикаты к
  ANN-кандидатам.

## Задача

1. **План-распознавание `And([VectorSimilarity, preds…])`:** в read_planner
   (или где план строится) распознать паттерн: один VectorSimilarity + прочие
   предикаты в And. Скомпилировать во внутренний `FilteredVectorQuery`
   (vector-часть + остаточные предикаты). Enum `Filter` НЕ ломать; это
   внутренняя планировочная форма. Если vector НЕ в And (голый) — прежний путь.
2. **Post-filter с адаптивным oversample:** запросить у vector-индекса
   k′ = k × oversample (default 2×, из `SearchOpts.oversample` или дефолт) →
   применить остаточные предикаты к кандидатам → если прошло < k, retry с
   k′ ×= 2 до cap (MAX_TOPK) → вернуть что есть (даже если < k, когда
   исчерпали). Изучи overscan-заметку из V0.0 (search_filter возвращает <knbn
   при жёстком фильтре — здесь мы сами оверсэмплим на уровне k, отдельно).
3. **Семантика oversample-поля:** теперь ВКЛЮЧЕНА — `oversample` из запроса
   управляет начальным k′-множителем. clamp разумно.

## Тесты (TDD red-first)

- **корректность:** filtered top-k == brute-force filtered ground truth на 10K
  (несколько селективностей: 50%, 10%, 1% — предикат по вспомогательному полю).
  Строй запрос через query-builder (правило CLAUDE.md).
- **фильтр отсекает всё → пустой результат, БЕЗ бесконечного retry** (дошли до
  cap → вернули пусто).
- **oversample управляет:** больший oversample → не меньше валидных кандидатов
  (монотонность; статистич.).
- **tx-staged + фильтр по staged-полям:** in-tx запрос с фильтром видит
  свои staged-вектора и корректно фильтрует.
- back-compat: голый VectorSimilarity (без And) работает как раньше.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector
  --full` + `@engine --full` (read_planner тронут) + workspace clippy.
- fmt/clippy тронутых крейтов `-- -D warnings`.
- Запросы строить ТОЛЬКО через билдеры (исключения — serde-round-trip тесты).
- Пиллары. Импорты в шапке. Раскладка tests/. НЕ трогать код вне задачи.
- stray-логи в корне — отметь, НЕ удаляй сам.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits.

## Definition of done

- Распознавание And([VectorSimilarity, preds]) → FilteredVectorQuery;
  post-filter с адаптивным oversample-retry до MAX_TOPK; oversample-поле
  включено.
- Тесты (корректность vs brute-force на селективностях, пустой-без-retry,
  oversample-монотонность, tx-staged+фильтр, back-compat голый vector) зелёные.
- `./scripts/test.sh @vector @engine --full` + workspace clippy зелёные.
- Финал: тронутые файлы, форма FilteredVectorQuery, дефолт+cap oversample,
  где применяются остаточные предикаты, вывод гейта, что оставлено на #405
  (pre/co-filter).
