בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V6.1 — Node e2e полного стека: tests/e2e/tests/18-vectors.test.js

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #413:
> написать e2e-тест ПОЛНОГО стека (Node-клиент → WS/TCP → сервер → engine →
> HNSW) для всех новых векторных возможностей кампании. Существующий каркас:
> `tests/e2e/` (runner `e2e.test.js`, helpers, tests/01..17). Образец
> векторного минимума — `tests/e2e/tests/14-index2-types.test.js` (секция
> vector: create_index index_type:"vector", vector_dim, vector_metric; фильтр
> `{ op: "vector_similarity", field, query, k }`).

## Что покрыть в 18-vectors.test.js (новый файл, стиль как 01–17)

1. **DDL со всеми новыми опциями**: `create_index` c `index_type: "vector"`,
   `vector_dim`, `vector_metric` (cosine / l2 / dot), и `vector_quantization:
   "sq8"` (появилось в #411 — проверь точное имя wire-поля в
   `crates/shamir-server`/`shamir-query-types` по grep, НЕ угадывай).
2. **Insert + ANN top-k**: вставить ~20–50 векторов (dim 8–32, кластеры),
   `vector_similarity` top-k возвращает ближайшие, порядок по расстоянию.
3. **per-query `ef_search` + `oversample`** (#399): те же поля в
   `vector_similarity`-опе — проверь wire-имена по серверному коду/спекам
   (`docs/guide-docs/client-server-protocol-spec/` допускает raw JSON — это e2e wire-тест,
   raw JSON здесь легитимен). Смысловой assert: запрос с большим ef_search
   не хуже по recall; невалидные значения дают понятную ошибку.
4. **Filtered ANN** (#404–405): `vector_similarity` в сочетании с обычным
   where-предикатом (как это выражается на wire — посмотри серверные тесты /
   спеки; если комбинированный фильтр — and(vector_similarity, eq)).
   Assert: возвращаются только записи, прошедшие фильтр, top-k корректен.
5. **Sequential tx-инварианты на полном стеке** (лёгкая версия #416/#420):
   два последовательных insert'а с векторами → оба ищутся своим же вектором
   как top-hit; delete строки → её rid исчезает из ANN-выдачи.
6. **Квантованная таблица e2e**: индекс с quantization=sq8, вставить >
   порога fit (256+) векторов батчами, ANN всё ещё возвращает корректные
   top-k (recall-подобный assert мягкий: свой вектор в top-3).

## Как запускать

- Смотри `tests/e2e/README.md` + `package.json` — как поднимается сервер и
  каким скриптом гоняется suite. Запусти ВЕСЬ e2e-suite и добейся зелёного
  (не только нового файла).
- Rust-тесты, если понадобятся: ТОЛЬКО через `./scripts/test.sh` (raw
  cargo test забанен). Но задача — JS e2e; Rust-код НЕ менять. Если найдёшь
  баг на wire/сервере — НЕ чини молча: зафиксируй красный тест + опиши
  корень в финальном сообщении, оркестратор решит.

## Дисциплина

- Новый файл — только `tests/e2e/tests/18-vectors.test.js` (+ регистрация,
  если runner требует явной). Ничего вне задачи не трогать.
- Стиль/структура — как соседние тесты (module.exports async, test/assert
  helpers, fixtures.setupDb).
- stray-логи в корне — отметь, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- 18-vectors.test.js покрывает пункты 1–6; весь e2e-suite зелёный локально.
- Финал: перечень покрытых сценариев, точные wire-имена новых полей
  (ef_search/oversample/quantization) с указанием где в Rust-коде они
  определены, вывод прогона suite, найденные (если есть) баги стека.
