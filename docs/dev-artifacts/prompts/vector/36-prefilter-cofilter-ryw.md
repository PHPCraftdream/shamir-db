בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-5 — read-your-own-writes на pre/co-filter путях filtered ANN (Б-5) (#427)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #427 —
> MEDIUM находка ревью кампании. Область: `crates/shamir-engine/src/table/
> read_exec.rs` (~:1290-1410, filtered vector query planning) +
> `crates/shamir-index/src/vector/hnsw_adapter.rs` (`search_prefilter`,
> `search_cofilter`, ~:1568, :1643).

## Дефект (Б-5)

`read_exec.rs` резолвит filtered-ANN запрос `And(vector_similarity,
residual)` тремя путями по селективности:
- **post-filter** (запасной, редкая селективность): `backend.lookup_tx(...,
  tx, staged)` — ЯВНО передаёт `staged` (tx-staged векторы) в bare vector
  search, staged-кандидаты попадают в ANN-выдачу и residual-фильтруется
  ПОСЛЕ (проекцией полной записи через `build_filtered_vector_result`
  → residual применяется к резолвленным записям, включая staged).
- **pre-filter** и **co-filter** (частые, малая/средняя селективность):
  кандидаты резолвятся ИЗ ВТОРИЧНОГО ИНДЕКСА (`try_plan_index2`/
  `try_plan_index_scan`, строки ~1319-1354) — committed-only, затем
  `hnsw.search_prefilter(&fvq.query, k, &candidates)` /
  `hnsw.search_cofilter(...)` НЕ получают `staged` вообще.

Итог: in-tx вставка/апдейт векторной строки видна через bare
`vector_similarity` (post-filter путь), но НЕВИДИМА через
`And(vector_similarity, residual)`, если residual резолвится через
вторичный индекс (частый случай) — несогласованная семантика видимости
в одной сессии.

## Осложнение (важно понять перед фиксом)

Staged-строка не может появиться в `candidates` естественным путём —
вторичный индекс (btree/functional/fts), из которого резолвятся
`candidates`, НЕ индексирует незакоммиченные tx-мутации. Значит фикс
должен: (1) получить полный набор staged-мутаций таблицы для этой tx
(rid → запись, не только вектор — нужны ПОЛНЫЕ значения полей residual-
предиката, не только эмбеддинг); (2) для каждой staged-строки применить
residual-предикат (уже есть `FilterNode::matches(record, ctx) -> bool`,
`crates/shamir-engine/src/query/filter/filter_node.rs:228`) — если
матчит, включить её rid в `candidates` ПЕРЕД вызовом
`search_prefilter`/`search_cofilter`, и передать вектор этой строки
в adapter (либо расширить сигнатуру adapter, либо — проще — score'ить
staged-кандидата на стороне `read_exec.rs` brute-force'ом (`dist.eval`)
и смерджить с ranked-результатом adapter'а, по образцу того, как это уже
сделано в bare `search()` (см. `hnsw_adapter.rs::search`, секция
«Merge the caller's own un-committed staged vectors»).

## Задача

1. Найди, как получить ПОЛНУЮ staged-запись (не только вектор) по rid для
   данной таблицы/tx — вероятно `TxContext` хранит staged full-record
   bytes где-то для insert/update (грепни `staged_records`/
   `write_set`/аналоги в `shamir-tx::TxContext`). Если полного набора
   staged-записей с полями нет в удобной форме — сначала исследуй, не
   выдумывай API.
2. В `read_exec.rs`, ПЕРЕД вызовом `search_prefilter`/`search_cofilter`:
   собери staged-кандидатов этой таблицы (rid + вектор + запись),
   отфильтруй по residual через `FilterNode::matches`, объедини с
   `candidates` (или отдельно score'и brute-force и смерджи после).
3. Реализация должна давать одинаковый видимый результат независимо от
   выбранного пути (pre/co/post-filter) — тот же принцип read-your-own-
   writes, что уже есть в post-filter.
4. Сохрани cost-based выбор пути (PRE_FILTER_MAX_CANDIDATES/
   CO_FILTER_MAX_SELECTIVITY) без изменений — добавление staged-кандидатов
   не должно ломать селективность-эвристику произвольно (staged обычно
   мало строк на tx — добавь их ПОСЛЕ вычисления selectivity по
   committed-кандидатам, не до).

## Тесты

In-tx тест на КАЖДЫЙ из трёх путей (форсируй селективность через размер
таблицы/candidate count, как в существующих filtered-ANN тестах —
смотри `crates/shamir-index/src/vector/tests/` и
`crates/shamir-engine` filtered-vector тесты): вставка векторной строки
внутри tx (без коммита) + запрос `And(vector_similarity, residual)`,
матчащий residual этой строки → должна найтись. Негативный: residual НЕ
матчит staged-строку → не находится (не путать «всегда включать
staged» с корректной residual-фильтрацией).

## Гейт

- `./scripts/test.sh @vector @engine --full` 1×;
- `cargo clippy -p shamir-engine -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt` тронутых `-- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично, но задача реально
затрагивает несколько файлов — это ожидаемо. НЕ трогай post-filter путь
(уже корректен). stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Pre-filter и co-filter пути видят staged-строки, прошедшие residual-
предикат, идентично post-filter пути; residual корректно фильтрует (не
"всегда включать"); regression тесты на все три пути зелёные; гейт
зелёный. Финал: как получена staged-запись для residual-эвалюации,
где смёржены результаты, вывод тестов, вывод гейта. Если найдёшь, что
полного staged-record API не существует и его создание — отдельная
большая задача вне разумного скоупа — честно доложи и предложи
минимальный безопасный фикс (например: pre/co-filter деградируют
до post-filter пути, КОГДА tx имеет staged-мутации на этой таблице,
жертвуя производительностью ради корректности видимости — это
приемлемый компромисс, если полный merge слишком инвазивен).
