בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# 389 — WASM functions_lifecycle таймауты под полной параллельностью nextest

> Контекст: под `./scripts/test.sh --full` (полная nextest-параллельность,
> все крейты разом) WASM-тесты в
> `crates/shamir-db/tests/functions_lifecycle.rs` временами ловят
> `SLOW`/`TIMEOUT`. Причина — НЕ дедлок: каждый такой тест компилирует
> WASM-модуль (cranelift, CPU-bound) в момент, когда десятки других
> тестовых бинарей грузят все ядра → legit-медленные WASM-тесты голодают
> по CPU и вылезают за slow-timeout. Это отдельный класс (#380 подтвердил:
> НЕ ACL-drift). ⛔ НЕ поднимать slow-timeout глобально как маскировку —
> глобальный таймаут прячет реальные дедлоки, ради ловли которых nextest
> и введён.

## Задача

Ограничить ПАРАЛЛЕЛИЗМ compile-heavy WASM-тестов, а не поднимать таймаут.
Nextest `[test-groups]` + override `test-group` — ровно этот инструмент:
дать WASM-классу выделенную группу с `max-threads = N` (небольшое N,
напр. 2–4), чтобы cranelift-компиляции не конкурировали за все ядра
одновременно, но и не сериализовались полностью.

## Точка врезки

`.config/nextest.toml`. Сейчас там `[profile.default]` +
per-test overrides (`wasm_function_inserts_and_queries` 120s,
scram 10s). Расширить:

1. Объявить группу:
   ```toml
   [test-groups]
   wasm-heavy = { max-threads = 2 }   # cranelift-компиляции CPU-bound;
                                        # ограничиваем параллелизм, не таймаут
   ```
   (Подбери max-threads под здравый смысл: 2 — консервативно; если ядер
   много и хочется быстрее — можно 4. Обоснуй выбор в комментарии.)

2. Override, приписывающий ВЕСЬ WASM-класс к группе. Класс — тесты в
   `functions_lifecycle.rs`, начинающиеся с `wasm_` ПЛЮС любые, что реально
   компилируют WASM (`facade_*`, `source_function_full_lifecycle`,
   `create_from_source_compiles` — свериться, какие из них вызывают
   компиляцию через `compile_or_skip`/`create_from_source`). Самый
   надёжный фильтр — по бинарю:
   ```toml
   [[profile.default.overrides]]
   filter = "package(shamir-db) and binary(functions_lifecycle)"
   test-group = "wasm-heavy"
   ```
   Свериться с nextest-синтаксисом фильтров (binary/test/package). Если
   правильнее сузить до реально-медленных — сузь по `test(/wasm_.*/)`, но
   не потеряй facade/source-компилирующие. Объясни выбор.

3. Существующий per-test override `wasm_function_inserts_and_queries`
   (120s/kill 240s) СОХРАНИТЬ — он про legit-долгий прогон, ортогонален
   группе. Убедиться, что override'ы не конфликтуют (тест может быть и в
   группе, и иметь свой slow-timeout — это ок).

4. Продублировать группу в `[profile.ci]`, если у CI-профиля свои
   override'ы параллелизма (сейчас профиль ci не имеет overrides — добавь
   тот же test-group override под ci, чтобы CI тоже не голодал).

## Проверка (БЕЗ сырого cargo test — только через wrapper)

- `./scripts/test.sh -p shamir-db --full -- wasm` — WASM-класс зелёный,
  без SLOW/TIMEOUT.
- **Ключевая проверка нагрузки:** прогнать полный `--full` под нагрузкой
  (эти таймауты вылезают именно под общей параллельностью, не в изоляции):
  ```
  ./scripts/test.sh --full > run.log 2>&1; rc=$?
  grep -aE "Summary|FAIL|TIMEOUT|SLOW|ABORT|panic|leak" run.log; echo "exit=$rc"
  ```
  Зациклить 2–3 раза — SLOW-маркеры на functions_lifecycle должны исчезнуть
  (или стать редкими и оставаться ПОД kill-порогом). Если всё ещё SLOW —
  уменьшить max-threads группы ещё, ЛИБО расширить класс (какой-то
  компилирующий тест не попал в фильтр).
- Синтаксис .config/nextest.toml валиден: `cargo nextest list` не падает
  на разборе конфига.

## Definition of done

- `[test-groups]` + override, ограничивающие параллелизм WASM-класса.
- НЕ поднят глобальный slow-timeout (проверка: `[profile.default]`
  slow-timeout остался 30s/6).
- Полный `--full` прогон под нагрузкой: functions_lifecycle без TIMEOUT,
  SLOW редкий/отсутствует и под порогом.
- Финал: что за группа, max-threads и почему, какой фильтр покрыл класс,
  вывод прогонов (эксит-код + греп по маркерам), остаточные наблюдения.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
