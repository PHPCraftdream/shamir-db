בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V3.2 — pre-filter (exact SIMD) + co-filter (search_filter) + cost-based выбор

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 3.2 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P3, второй лист). V3.1 (коммит dbc46b05) дал post-filter+oversample
> путь через `FilteredVectorQuery` (crates/shamir-engine/src/table/filtered_vector.rs
> + read_exec.rs `read_filtered_vector_scan`). Здесь добавляем ДВА более
> быстрых пути + cost-based выбор между тремя.

## Зачем

Post-filter (V3.1) хорош при слабой селективности, но при ЖЁСТКОМ фильтре
(1% проходит) он оверсэмплит впустую. Два лучших пути:
- **pre-filter**: если предикаты покрыты вторичным индексом И candidate set
  МАЛ (≤ порога) → точный SIMD-скоринг кандидатов БЕЗ HNSW (точный результат).
- **co-filter**: средняя селективность → `hnsw.search_filter` с FilterT по
  множеству разрешённых internal id (обходит граф с маской).
- **cost-based выбор** между pre/co/post по оценке кардинальности.

## Проверенные факты (спайк V0.0, коммит 5ec84564, hnsw_rs_contract_tests.rs)

- `hnsw.search_filter(data, knbn, ef, Option<&dyn FilterT>) -> Vec<Neighbour>`.
- `FilterT { fn hnsw_filter(&self, id: &DataId) -> bool }`, `DataId = usize`
  (internal id). Blanket impl'ы: `Vec<usize>` (**требует SORTED** — binary_search)
  и `F: Fn(&DataId)->bool` (для bitmap/set — без сортировки).
- **ВАЖНО (overscan)**: `search_filter` применяет фильтр только на layer-0 +
  post-hoc drop → при жёстком фильтре может вернуть **< knbn** даже когда
  подходящих точек достаточно глубже. Co-filter путь ДОЛЖЕН щедро оверсэмплить
  `ef` (не только `knbn`). Это ГЛАВНЫЙ подводный камень — см. ниже тест.
- `HnswAdapter.rid_to_internal` (scc::HashMap RecordId→usize) — есть маппинг
  rid→internal (для построения allow-set внутренних id). `vectors`-map
  (internal→Vec<f32>) — для точного SIMD-скоринга кандидатов (pre-filter).

## Задача

1. **Pre-filter путь**: если остаточные предикаты покрыты вторичным индексом
   (btree/functional) и оценка candidate set ≤ порога (tunable, напр. 4096) →
   получить rid-кандидаты из вторичного индекса → отфильтровать те, что имеют
   вектор → точный top-k SIMD-скорингом (dot/l2 из simd.rs) по их векторам.
   HNSW не трогаем. Результат ТОЧНЫЙ.
2. **Co-filter путь**: собрать allow-set внутренних id (rid_to_internal по
   разрешённым rid) → `hnsw.search_filter` с FilterT. Для bitmap-подобного
   набора — closure `Fn(&usize)->bool` (без сортировки). ЩЕДРЫЙ ef-overscan
   (учесть V0.0-факт: search_filter может вернуть <knbn). Новый метод в
   VectorAdapter/HnswAdapter (напр. `search_filtered(query, k, opts,
   allow: &AllowSet)`).
3. **Cost-based выбор** в read_filtered_vector_scan / filtered_vector.rs:
   оценить кардинальность candidate set из вторичного индекса → мал →
   pre-filter; средняя (есть индекс, но велик) → co-filter; велика/нет
   покрывающего индекса → post-filter (V3.1). Пороги — tunable, обосновать.
   Границы применимости co-filter — из спайка V0.0.

## Тесты (TDD red-first)

- **ОБЯЗАТЕЛЬНО (из ревью V0.0)**: тест, доказывающий overscan-факт
  search_filter — allow-list из ~20 подходящих id, при `ef=knbn` возвращается
  < knbn, при большом ef — knbn. Превратить задекларированный в спайке факт в
  контракт (это было отложено на #405 — закрой здесь). Файл — по месту
  co-filter (hnsw_adapter tests или новый).
- **эквивалентность трёх путей**: property/юнит — pre/co/post на одном датасете
  + фильтре дают эквивалентный top-k (в пределах HNSW-recall для co/post;
  pre-filter — точный, сверить с brute-force filtered ground truth).
- **cost-based выбор**: мал candidate set → pre-filter выбран (проверить через
  stats.index_used или счётчик); средний → co-filter; большой/нет индекса →
  post-filter.
- **FilterT sorted-инвариант**: Vec<usize>-allow отсортирован (иначе
  binary_search сломан) — тест или assert.
- back-compat: V3.1 post-filter путь всё ещё работает когда pre/co неприменимы.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector
  @engine --full` + workspace clippy.
- fmt/clippy тронутых крейтов `-- -D warnings`. Запросы через билдеры.
- Пиллары: SIMD-ядра из simd.rs (не дублируй), scc/atomics, без O(N²).
- Импорты в шапке. Раскладка tests/. НЕ трогать код вне задачи.
- stray-логи в корне — отметь, НЕ удаляй сам.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits.

## Definition of done

- pre-filter (точный SIMD по candidate set из вторичного индекса) + co-filter
  (search_filter/FilterT со щедрым ef) + cost-based выбор трёх путей.
- overscan-контракт-тест (закрывает долг #405 из V0.0), эквивалентность путей,
  cost-based выбор, sorted-инвариант, back-compat.
- `./scripts/test.sh @vector @engine --full` + workspace clippy зелёные.
- Финал: тронутые файлы, пороги cost-based + обоснование, форма AllowSet/
  FilterT, как решён ef-overscan, вывод гейта, что оставлено на #406 (бенч).
