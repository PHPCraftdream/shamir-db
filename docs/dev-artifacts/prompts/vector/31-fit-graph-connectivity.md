בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-1 — fit-переход: потеря graph-связности (Б-1) + ранний convergence-exit (Б-3) (#423)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #423 —
> HIGH CONFIRMED находка ревью кампании. Файл:
> `crates/shamir-index/src/vector/hnsw_adapter.rs` (`try_fit_and_rebuild` и
> смежные пути). Прочитай функцию целиком прежде чем менять.

## Б-1 (HIGH): векторы окна fit НИКОГДА не вставляются в u8-граф

Факты (проверены ревьюером):
- Коммит `c7a6efbe` удалил `hnsw_u8_catchup.parallel_insert` из catch-up
  loop (~строки 937-972) и `quantize_and_insert_u8` из self-migration
  (~:1354-1360, :1513-1517), но ОСТАВИЛ комментарии «the fitter's final
  parallel_insert after the catch-up loop handles graph connectivity» —
  такого кода в файле НЕТ. Все 4 `parallel_insert`: строки ~864, ~903
  (delta-пасс ДО флипа), ~1462, ~1492.
- Сценарий: upsert стартует до флипа `is_fitted`, его `vectors.insert`
  завершается после delta-скана фиттера → catch-up/self-migration кладут
  коды ТОЛЬКО в `vectors_u8`, узла в `hnsw_u8` нет. Пока
  `len() <= QUANT_BRUTE_FORCE_MAX=512` брутфорс маскирует; при росте
  индекса вектор навсегда невидим для graph-search и co-filter, дырой
  уезжает в снапшот v2 (dump сериализует hnsw_u8).

Фикс: после catch-up loop выполнить `parallel_insert` в `hnsw_u8` для ВСЕХ
internals из `vectors_u8`, отсутствующих в графе. Dedup обязан опираться на
существующий atomic-claim (`vectors_u8.entry_async` Vacant→claim) либо
эквивалентный точный признак «узел уже в графе» — двойная вставка d_id
это отдельный класс бага (уже ловили). Обнови лживые комментарии.

## Б-3 (MEDIUM): convergence-check инфлируется post-flip вставками

~:964-971: `vectors_u8.len() + deleted_count >= next_id_at_flip` —
`vectors_u8` содержит и internals >= `next_id_at_flip` (post-flip upserts
кладут туда напрямую), цикл может выйти рано, пока pre-flip upsert ещё не
долетел до `vectors.insert`. Следствия: f32-граф дропается (`hnsw.store(None)`,
~:1002) до `hnsw.load_full()` в pre-flip upsert → `Internal("f32 graph
absent...")` (~:1323) — ошибка легитимного коммита; плюс усиление Б-1.

Фикс: считать сходимость только по internals < `next_id_at_flip`
(например, O(1)-счётчик мигрированных pre-flip internals, НЕ полный скан —
`scc::len()` тут и так O(N), но у нас уже есть зеркала; выбери точный и
дешёвый механизм и обоснуй в комментарии).

## Регресс-тесты (обязательно, оба класса)

1. **Главный**: конкурентные upserts через порог fit с датасетом >512
   (600+ векторов, чтобы брутфорс НЕ маскировал), после сходимости fit:
   поиск каждого вектора своим же запросом с ef=достаточным — missing==0
   И через graph-путь (проверь что тест реально не попадает в brute-force:
   len>QUANT_BRUTE_FORCE_MAX). Существующий
   `concurrent_upsert_across_threshold_no_loss` (400 векторов) бьёт по
   брутфорсу — либо подними его датасет, либо добавь отдельный тест.
2. Регресс на Б-3: pre-flip upsert не получает Internal-ошибку
   (детерминированно или статистически под лупом — как получится честно).
3. Проверка снапшота: после fit с гоночными вставками dump→load v2 не
   теряет узлы (round-trip count).

## Гейт

- `./scripts/test.sh @vector @engine --full` 1×;
- `cargo clippy -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt -p shamir-index -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh (вывод в файл → grep файла).
Хирургично: только fit-transition пути и их комментарии. Пиллары:
lock-free, guard не через await, никаких новых Mutex. Импорты в шапке.
stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Оба дефекта закрыты, комментарии правдивы, регрессы (>512 graph-путь,
Б-3, снапшот round-trip) зелёные, гейт зелёный. Финал: механика фикса,
как гарантирован dedup вставки, чем считается сходимость, вывод гейта.
