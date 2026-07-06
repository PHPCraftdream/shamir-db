בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-2 — транзиентно пустой search в окне fit (Б-4) (#424)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #424 —
> MEDIUM находка ревью кампании. Файл:
> `crates/shamir-index/src/vector/hnsw_adapter.rs`.

## Дефект (Б-4)

Два места (`search` ~:2081 и `search_cofilter` ~:1530) делают:
```rust
if self.quantized_active() { /* u8-ветка */ return Ok(...); }
// ... f32-ветка:
let hnsw = match self.hnsw.load_full() {
    Some(h) => h,
    None => { return Ok(vec![]); }  // <-- Б-4
};
```
Комментарий над `None`-веткой называет её «invariant violation, unreachable» —
но это НЕ так: между проверкой `quantized_active()` (или внешним
`if/else if` гейтом в `search`) и `hnsw.load_full()` возможен реальный флип
`is_fitted` фиттером, который дропает f32-граф (`hnsw.store(None)`,
Release) РОВНО в этом окне. Запрос, попавший в это окно, тихо получает
пустой результат вместо корректного ответа через u8-граф.

## Задача

В ОБОИХ местах: при `None` от `hnsw.load_full()` — перечитать
`quantized_active()`; если теперь `true` — выполнить u8-ветку (тот же код,
что и в основной quantized-ветке функции, для этого же query/k/ef/allow_set)
вместо `Ok(vec![])`. Если `quantized_active()` всё ещё `false` (значит
действительно инвариант нарушен — unquantized адаптер без f32-графа,
что и правда unreachable) — тогда пусть остаётся `Ok(vec![])` с уточнённым
комментарием (это уже настоящий "should never happen").

Не дублируй код бездумно — вынеси u8-ветку каждой функции в приватный
helper-метод, если это не усложнит чрезмерно (сигнатуры совпадают:
query, k, ef/allow_set, quantizer, hnsw_u8), либо просто повтори вызов
существующей логики через retry-обёртку (`match ... { None => retry }`).
Хирургично — НЕ рефактори остальную функцию.

## Регресс-тест

Тест на детерминированное окно [гейт → load_full] vs [flip → store(None)]:
принудительно форсировать флип МЕЖДУ проверкой `quantized_active()`и
`load_full()` — например через управляемую задержку/barrier в тестовом
хелпере, либо статистически (много конкурентных search во время
fit-перехода с достаточным числом итераций, assert что НИ ОДИН search не
вернул пустой результат при живых данных). Смотри существующие
конкурентные тесты в `quantized_graph_tests.rs` как образец инструментария
(Lcg, clustered, rid, spawn+timeout паттерн). Отдельно от VR-1 тестов —
не трогай их.

## Гейт

- `./scripts/test.sh @vector @engine --full` 1×;
- `cargo clippy -p shamir-index --all-targets -- -D warnings`;
- `cargo fmt -p shamir-index -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: только `search`/
`search_cofilter` в hnsw_adapter.rs + их тесты. Пиллары: lock-free, guard
не через await. stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Оба места retry'ят в u8-ветку при транзиентном None, регресс-тест
воспроизводит и закрывает окно, гейт зелёный. Финал: механика фикса,
как воспроизведён/закрыт race, вывод гейта.
