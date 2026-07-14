בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TS cross-language msgpack паритет против Rust-fixture

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION-CLIENT-SURFACE.md` §4. Rust-fixture
> `crates/shamir-query-builder/tests/fixtures/repl_ddl_msgpack.json` (коммит
> b0d427f8) — эталон hex msgpack для 10 репл-DDL ops. TS-билдеры в
> `crates/shamir-client-ts/src/core/builders/replication.ts` (757b3ded).
> TS msgpack-энкодер: `@msgpack/msgpack` v3 (`encode`).

## Задача

Доказать, что TS-клиент и Rust-клиент дают БАЙТ-ИДЕНТИЧНЫЙ msgpack для
одного логического репл-DDL op'а — эталонный тест wire-контракта между
двумя клиентами.

## Тест (vitest)

Новый `crates/shamir-client-ts/src/core/builders/__tests__/repl_parity.test.ts`:

1. Прочитать Rust-fixture JSON (относительный путь до
   `../../../../../shamir-query-builder/tests/fixtures/repl_ddl_msgpack.json`
   — сверь реальную глубину; читать через `fs.readFileSync` в Node-окружении
   vitest, игнорируя `_comment`/`_key_order_note`/`_key_order`-ключи).
2. Для каждого op-label в fixture: построить ТОТ ЖЕ op через TS-билдер
   (те же входные строки, что зашил Rust-fixture — сверь их по
   `crates/shamir-query-builder/tests/repl_ddl_msgpack.rs`, чтобы входы
   совпадали 1:1), `encode(op)` из `@msgpack/msgpack`, привести к hex.
3. `expect(tsHex).toEqual(fixtureHex)` для каждого op'а.

## Порядок ключей — критично

`@msgpack/msgpack.encode` сохраняет порядок вставки ключей объекта; rmp_serde
пишет поля в порядке ОБЪЯВЛЕНИЯ struct (см. `_key_order_note` в fixture).
Значит объектные литералы в TS-билдере ДОЛЖНЫ вставлять ключи в том же
порядке, что поля Rust-struct в `repl_ops.rs`. Если тест выявит расхождение
из-за порядка — ПОПРАВЬ порядок вставки ключей в
`builders/replication.ts`/`types/replication.ts` (это правка билдера, не
теста), пока байты не совпадут. Сверь порядок полей по `repl_ops.rs`
для каждого op'а и вложенных struct (`ReplScope { db, repo, table }`,
`ReplStream { scope, direction, mode }`, `CreateSubscriptionOp
{ create_subscription, upstream, publication, profile }` и т.д.).

## Fallback (только если raw-байт-паритет НЕДОСТИЖИМ)

Если после выравнивания порядка ключей остаётся принципиальное расхождение
энкодеров (напр. `@msgpack/msgpack` иначе кодирует какой-то тип, чем
rmp_serde) — НЕ подгоняй тест фальшиво. Вместо raw-hex сравнивай
СЕМАНТИЧЕСКИ: `decode(tsBytes)` и `decode(hexToBytes(fixtureHex))` →
`toEqual` на декодированных структурах. Задокументируй в комментарии теста
ТОЧНУЮ причину, почему raw-байты расходятся, и что семантический паритет
сохранён. (Но сначала честно добейся raw-паритета — для map/str/uint/bool
оба энкодера следуют спеку и должны совпадать.)

## Гейт

- vitest зелёный (scoped на новый тест): из `crates/shamir-client-ts`
  запусти реально настроенный раннер (`npx vitest run <path>`).
- `npx tsc --noEmit` чистый (если правил builder/types).

## Definition of done

- `repl_parity.test.ts` сравнивает TS-энкод с Rust-fixture для 10 ops.
- Byte-паритет достигнут (или обоснованный семантический fallback с
  документированной причиной).
- Если правил порядок ключей билдера — юнит-тесты #374 всё ещё зелёные.
- Финальное сообщение: byte или semantic паритет, какие правки порядка
  ключей понадобились, вывод vitest.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
