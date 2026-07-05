בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V6.2 — расширение TS e2e: crates/shamir-client-ts/src/__tests__/e2e-vector.test.ts

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #414:
> расширить существующий `e2e-vector.test.ts` (vitest, харнесс
> `e2e-harness.ts` — поднимает реальный сервер) на ВСЕ новые векторные
> возможности кампании через ТИПИЗИРОВАННЫЙ TS-билдер (НЕ raw JSON — билдер
> обязателен по CLAUDE.md; raw допустим только там, где wire сам под тестом).

## Контекст

- Свежий образец полного покрытия на Node-стороне:
  `tests/e2e/tests/18-vectors.test.js` (коммит 6c207a17) — там же
  подтверждённые wire-имена: `vector_quantization: "sq8"` в CreateIndexOp;
  `ef_search?: u32`, `oversample?: f32` в `vector_similarity`;
  filtered ANN = `and([vectorSimilarity, residual])`;
  `stats.index_used`: `index2_ranked` / `filtered_vector_scan`.
- TS-билдеры уже поддерживают vector_quantization (V5.2 Фаза B, коммит
  83c96abe) и ef_search/oversample (#399) — используй их API (см.
  `src/core/builders/ddl.ts`, `filter.ts` + их __tests__).
- ВАЖНО: серверный бинарь для харнесса — свежесобранный
  `target/release/shamir-server.exe` (Jul 5). Если харнесс собирает/ищет
  иначе — проверь, что тестируется свежий сервер.

## Что покрыть (расширение e2e-vector.test.ts)

1. Реалистичные dim (например 64/128) и все 3 метрики (cosine/l2/dot) —
   создание индекса + ANN top-k с проверкой порядка/кластера.
2. DDL с quantization "sq8" через билдер + back-compat без quantization.
3. per-query efSearch/oversample через билдер: recall-superset assert
   (большой ef не теряет id из выдачи малого), clamp огромного ef.
4. Filtered ANN через билдер (and(vectorSimilarity, eq)): только
   прошедшие фильтр, index_used = filtered_vector_scan; пустой предикат
   терминирует с 0 записей.
5. Insert→delete→ANN: удалённый id исчезает из выдачи.
6. sq8 через порог fit (вставить 280+ векторов батчами, dim 16): свой
   вектор в top-3 (soft recall), filtered ANN после fit работает.

## Запуск

- Смотри package.json/vitest.config.ts как гоняются e2e-тесты TS-клиента.
  Прогони ВЕСЬ vitest-suite клиента и добейся зелёного.
- Rust-код НЕ менять; Rust-тесты (если вдруг) — только ./scripts/test.sh.
  Баг стека → красный тест + описание, НЕ молчаливый фикс.

## Дисциплина

- Менять только `e2e-vector.test.ts` (+ мелкие правки харнесса, если
  строго необходимы для новых сценариев). Стиль соседних тестов.
- stray-логи отметь, НЕ удаляй. НИКОГДА не запускай git-команды, мутирующие
  дерево/индекс.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- e2e-vector.test.ts покрывает пункты 1–6 через типизированный билдер;
  весь vitest-suite TS-клиента зелёный.
- Финал: список сценариев, использованные билдер-API (имена методов),
  вывод прогона, найденные баги (если есть).
