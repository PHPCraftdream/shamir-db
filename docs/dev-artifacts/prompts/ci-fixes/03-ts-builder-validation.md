# Brief: cheap client-side builder validation + Batch.transactional() bug (task #623)

## Контекст

Аудит §2.3 (`docs/dev-artifacts/audits/2026-07-06-client-surface-parity.md`)
отметил, что TS-билдеры (`crates/shamir-client-ts/src/core/builders/`) не
проверяют дешёвые инварианты локально — round-trip до сервера тратится
впустую, а ошибка приходит неструктурной серверной строкой вместо понятной
клиентской. Расследование (Explore-агент) сузило список до РЕАЛЬНЫХ
расхождений — не всё из исходного аудита оказалось валидным:

1. **`unique` + `sorted` — запрещённая комбинация на сервере.**
   `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:386-387`:
   `if op.sorted && op.unique { return Err(...."Index cannot be both sorted
   and unique"...) }`. РЕАЛЬНЫЙ round-trip waste — стоит проверять
   клиентски в `createIndex` (`crates/shamir-client-ts/src/core/builders/ddl.ts:156-200`).

2. **`ef_search` — сервер МОЛЧА клэмпит**, не отклоняет.
   `crates/shamir-index/src/vector/hnsw_adapter.rs:44` определяет
   `MAX_EF_SEARCH = 10_000`; строки ~290/~350 клэмпят
   `v.min(MAX_EF_SEARCH).max(k)` без ошибки. Клиент должен явно
   отклонять `ef_search > 10_000` вместо того чтобы полагаться на тихий
   сервер-сайд clamp — так пользователь узнаёт о проблеме сразу, а не
   получает молча урезанный (и потому менее точный) поиск.

3. **`vector_dim` НЕ обязателен** — сервер уже дефолтит `384`
   (`table_manager_index_mgmt.rs:244`, `op.vector_dim.unwrap_or(384)`).
   ЭТО НЕ БАГ — не добавляй проверку "vector_dim required for vector
   index", исходная формулировка аудита была неточной. Пропусти этот
   пункт.

4. **`k == 0` / пустой query vector** — сервер НЕ валидирует, просто
   молча возвращает 0 результатов (harmless fallthrough), не ошибка.
   Низкий приоритет, но дёшево — добавь клиентскую проверку `k <= 0` →
   throw (явная ошибка лучше молчаливого "0 результатов", которое легко
   спутать с "просто нет совпадений").

5. **FTS `mode` default — TS `'and'` СОВПАДАЕТ с сервером**
   (`read_planner.rs:50-54`, дефолт `FtsMode::AndAll` при отсутствии
   `mode`). Это НЕ расхождение — не трогай, никакой унификации не нужно.

6. **Реальный баг**: `Batch.transactional(isolation?: IsolationLevel)`
   в `crates/shamir-client-ts/src/core/builders/batch.ts:163-166`:
   ```ts
   transactional(isolation?: IsolationLevel): this {
     this.transactionalValue = true;
     this.isolationValue = isolation;  // <-- unconditional overwrite
     return this;
   }
   ```
   Вызов `.transactional('serializable').transactional()` (например,
   второй вызов без аргумента где-то в цепочке fluent-API) СБРАСЫВАЕТ
   `isolationValue` обратно в `undefined`, теряя ранее установленный
   уровень изоляции. Исправь: присваивай `isolationValue` ТОЛЬКО когда
   `isolation !== undefined`:
   ```ts
   transactional(isolation?: IsolationLevel): this {
     this.transactionalValue = true;
     if (isolation !== undefined) this.isolationValue = isolation;
     return this;
   }
   ```

## Задача

### A. `Batch.transactional()` fix (batch.ts)

Применить исправление из пункта 6 выше. Добавить регрессионный тест в
`crates/shamir-client-ts/src/core/builders/__tests__/` (grep существующий
файл тестов на `Batch`/`batch.ts`, добавь туда) — сценарий: вызов
`.transactional('serializable')`, затем `.transactional()` (без
аргумента), проверь, что `build().isolation === 'serializable'`
(НЕ `undefined`).

### B. `createIndex` — unique+sorted guard (ddl.ts)

В `crates/shamir-client-ts/src/core/builders/ddl.ts`, функция
`createIndex` (строки ~156-200) — если `opts?.unique && opts?.sorted`,
брось `Error` с понятным сообщением ДО сборки `CreateIndexOp` (зеркалит
серверную ошибку "Index cannot be both sorted and unique", но клиентски,
без round-trip). Добавь тест в `ddl`-тестах (найди существующий файл
тестов для ddl.ts, тот же паттерн что и Batch.tryBuild — throws с
понятным сообщением).

### C. `ef_search` — explicit reject over MAX (filter.ts или где строится vectorSimilarity)

Найди, где TS-билдер собирает `ef_search` для vector search
(`crates/shamir-client-ts/src/core/builders/filter.ts` или query.ts —
grep `ef_search`). Добавь константу `MAX_EF_SEARCH = 10_000` (зеркалит
`crates/shamir-index/src/vector/hnsw_adapter.rs:44` — оставь комментарий
с этой ссылкой, чтобы константы не разошлись незаметно) и явную проверку:
если `ef_search` задан и `> MAX_EF_SEARCH`, брось `Error` с сообщением,
объясняющим, что сервер бы тихо клэмпнул значение, и что явная ошибка
предпочтительнее. Добавь тест.

### D. `k <= 0` — explicit reject in vectorSimilarity (низкий приоритет, но дёшево)

Там же, где строится vector similarity filter/query с параметром `k` —
если `k` явно передан и `<= 0`, брось `Error` (сервер бы молча вернул 0
результатов — это легко спутать с "просто нет совпадений", явная ошибка
понятнее). Добавь тест.

## НЕ трогай

- `vector_dim` required-check — НЕ добавляй, см. пункт 3 выше (сервер
  уже дефолтит, это не баг).
- FTS `mode` default — НЕ трогай, см. пункт 5 выше (уже совпадает).
- Ничего в Rust-крейтах — вся задача только в `shamir-client-ts`.
- `build()` (unchecked-путь) в batch.ts — если добавляешь новые проверки
  по образцу `tryBuild()`, СМОТРИ на существующий паттерн `tryBuild()`
  (строка 242) как на прецедент, но новая логика A-D должна быть в самих
  методах-билдерах (`createIndex`, `transactional`, ef_search/k-точках),
  а не в отдельном "tryBuild"-варианте — эти проверки достаточно дёшевы
  и универсальны, чтобы быть частью основного пути, а не opt-in.

## Прогон проверок

- `npx tsc --noEmit` (из `crates/shamir-client-ts`)
- `npm test` (vitest) — из `crates/shamir-client-ts`. E2E-тесты,
  требующие release-бинарник сервера, могут падать с ошибкой "stale
  shamir-server binary" — это ПРЕДСУЩЕСТВУЮЩАЯ инфраструктурная
  проблема (release-бинарник старее исходников), НЕ связанная с этой
  задачей; не пытайся её чинить. Убедись, что ЮНИТ-тесты (не e2e)
  проходят полностью, включая все новые/изменённые тесты из пунктов
  A-D.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-client-ts/src/core/builders/batch.ts`,
  `ddl.ts`, `filter.ts` (или query.ts, где реально лежит ef_search/k),
  плюс соответствующие `__tests__` файлы.
- `tsc --noEmit` чист.
- `npm test` — юнит-тесты зелёные (e2e stale-binary failures допустимы
  как предсуществующие и не в scope).
