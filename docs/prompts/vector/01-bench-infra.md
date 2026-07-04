בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V0.1 — bench-инфраструктура: @vector scope + кластеризованный датасет-генератор

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), рабочая директория
> D:\dev\rust\shamir-db. Реализуешь лист 0.1 плана
> `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md` (прочитай раздел «Бенч-фундамент
> (P0)» и «Решения» — там зафиксировано, что бенчи только QUICK-режим).

## Контекст (важные РЕШЕНИЯ, не отклоняйся)

- **Только QUICK-режим бенчей.** FULL-заглушку в `shamir-bench-utils`
  (`is_full()` всегда false) НЕ снимать, не трогать. Никакого re-enable FULL.
- Генератор данных нужен ОБЩИЙ для двух будущих инструментов (criterion-бенч
  V0.3 и vector_report V0.4), поэтому живёт в `shamir-bench-utils`.

## Задача

### 1. `@vector` scope в `scripts/test.sh`
Добавить именованный scope в функцию `scope_args` (найди её; там уже есть
`@tx`, `@engine`, `@oracle`, `@storage`, `@server`, `@e2e`, `@all`). Добавить:
```
@vector) echo "-p shamir-index -p shamir-engine" ;;
```
Расположи среди других `@*` веток по стилю файла. НЕ ломай существующие scope.
Проверь, что `./scripts/test.sh @vector` резолвится в оба крейта.

### 2. Кластеризованный датасет-генератор в `shamir-bench-utils`
Новый файл `crates/shamir-bench-utils/src/vector_data.rs` (подключить в
`crates/shamir-bench-utils/src/lib.rs` — сверь, как там ре-экспортятся модули;
следуй существующему стилю). Публичная функция генерации векторов:

- **Кластеризованное распределение** (НЕ uniform — uniform нереалистично льстит
  recall): K центроидов, каждый вектор = случайный центроид + гауссов шум σ.
  Сигнатура примерно:
  `pub fn clustered_vectors(n: usize, dim: usize, k_clusters: usize, sigma: f32, seed: u64) -> Vec<Vec<f32>>`
  (уточни имена под стиль крейта; можно вернуть и центроиды, если удобно для
  ground-truth — на твоё усмотрение, но задокументируй).
- **Детерминизм по seed** — БЕЗ глобального RNG и БЕЗ запрещённых `Math.random`/
  `Date::now`. Используй LCG (образец есть в
  `crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs::lcg_vec` —
  тот же множитель 6364136223846793005), Box-Muller для гаусса из двух LCG-
  uniform. Один и тот же seed → идентичный датасет.
- Параметры K/σ/seed — входные (попадут в отчёт для воспроизводимости).
- **Юнит-тест на детерминизм** (`crates/shamir-bench-utils/src/tests/` или по
  раскладке крейта): один seed → два вызова дают идентичные векторы; разные
  seed → различаются; размерности корректны; точки кластеризованы (напр.
  средняя внутрикластерная дистанция заметно меньше межкластерной — не строгий,
  а sanity-порог).

## Дисциплина репозитория (ОБЯЗАТЕЛЬНО)

- Тесты ТОЛЬКО через `./scripts/test.sh` (сырой `cargo test` заблокирован).
  Гейт: `./scripts/test.sh -p shamir-bench-utils` зелёный + ручной smoke
  `./scripts/test.sh @vector` (резолвится, крейты компилируются).
- fmt: `cargo fmt -p shamir-bench-utils -- --check` чист; clippy:
  `cargo clippy -p shamir-bench-utils --all-targets -- -D warnings` чист.
- НЕ грепать/пайпать вывод тестов на лету — писать в файл, потом grep.
- Импорты в шапке файла; раскладка tests/; один файл = один смысл.
- Fx-хэш/пиллары где уместно (генератор — обычный код, без concurrent-структур).
- НЕ трогать код вне задачи; хирургические правки.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log` и файлы вне задачи.

## Definition of done

- `@vector` scope работает в `scripts/test.sh`.
- `vector_data.rs` с кластеризованным seeded-генератором + юнит-тест
  детерминизма; `shamir-bench-utils` гейт зелёный.
- Финал: тронутые файлы, сигнатура генератора, как проверен детерминизм,
  вывод `./scripts/test.sh -p shamir-bench-utils`.
