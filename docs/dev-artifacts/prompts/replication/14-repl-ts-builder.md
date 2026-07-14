בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TS client builder для репл-DDL + vitest юнит-тесты

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION-CLIENT-SURFACE.md` §3 п.3. Rust-сторона
> готова (op-типы `crates/shamir-query-types/src/admin/types/repl_ops.rs`
> коммит 143dc060, Rust-билдер `ddl/replication.rs` коммит 904755a8).

## Задача

TS-типы + билдеры для 10 репл-DDL ops, БАЙТ-В-БАЙТ совпадающие по wire-форме
(msgpack map, snake_case ключи) с Rust serde. Паттерн — как
`crates/shamir-client-ts/src/core/{types,builders}/ddl.ts`: type-интерфейс
в `types/`, функция-конструктор в `builders/`, возвращающая wire-объект.

## Файлы

- `crates/shamir-client-ts/src/core/types/replication.ts` (новый) — интерфейсы.
- `crates/shamir-client-ts/src/core/builders/replication.ts` (новый) — билдеры.
- `crates/shamir-client-ts/src/core/builders/index.ts` — добавить
  `export * from './replication.js';`.
- Если types барыжатся через общий barrel — добавить и туда по образцу ddl.
- `crates/shamir-client-ts/src/core/builders/__tests__/replication.test.ts` (vitest).

## ТОЧНЫЕ wire-формы (соответствие Rust serde — соблюсти буквально)

Вспомогательные:
- `ReplScope` → `{ db: string, repo?: string, table?: string }` — repo/table
  ОПУСКАЮТСЯ если не заданы (`skip_serializing_if = Option::is_none`). Не
  сериализуй `null`/`undefined` — просто НЕ клади ключ.
- `ReplDirection` → строка `"pull"` | `"push"` | `"both"`.
- `ReplMode` → строка `"read_only"` | `"read_write"`.
- `ReplStream` → `{ scope: ReplScope, direction: ReplDirection, mode: ReplMode }`.
- `SubAction` (externally-tagged enum!) →
  - Pause → строка `"pause"`
  - Resume → строка `"resume"`
  - SetProfile(name) → объект `{ "set_profile": name }`

Ops (discriminator-ключ = имя op'а):
- `{ create_replication_profile: name, streams: ReplStream[] }`
- `{ drop_replication_profile: name }`
- `{ create_publication: name, scopes: ReplScope[] }`
- `{ drop_publication: name }`
- `{ create_subscription: name, upstream: string, publication: string, profile: string }`
- `{ drop_subscription: name }`
- `{ alter_subscription: name, action: SubAction }`
- `{ list_publications: true }`  (bool presence-flag — сверь имя поля в
  repl_ops.rs `ListPublicationsOp`; presence-flag сериализуется как `true`)
- `{ list_subscriptions: true }`
- `{ replication_status: true }`

⚠️ Сверь ТОЧНЫЕ имена полей и presence-flag'ов по
`crates/shamir-query-types/src/admin/types/repl_ops.rs` — НЕ угадывай
(особенно presence-поля read-only структур: имя = discriminator, тип bool).

## Билдеры (эргономика — как ddl.ts, opts-объект или fluent)

`replicationProfile(name, streams)`, `dropReplicationProfile(name)`,
`publication(name, scopes)`, `dropPublication(name)`,
`subscription(name, { upstream, publication, profile })`,
`dropSubscription(name)`, `alterSubscription(name, action)` (action:
`"pause"|"resume"|{ set_profile: string }`), `listPublications()`,
`listSubscriptions()`, `replicationStatus()`. Плюс хелпер
`replScope(db, opts?)` / `replStream(scope, direction, mode)`.

## Тесты (vitest)

Каждый билдер → assert глубокого равенства (`toEqual`) с ожидаемым wire-
объектом (проверяет snake_case ключи + опущенные optional'ы + формы
энумов). Обязательно покрыть:
- ReplScope с/без repo/table (опущение ключей);
- каждое значение ReplDirection/ReplMode;
- SubAction все три формы (pause/resume строки + set_profile объект);
- профиль с несколькими streams; publication с несколькими scopes;
- subscription со всеми полями; три read-only presence-flag = true.

(Wire-паритет с Rust проверяется отдельным cross-language фикстур-тестом в
задаче интеграции #375/#376 — здесь достаточно точных toEqual на форму.)

## Гейт

- vitest зелёный: из `crates/shamir-client-ts` запусти тестовый раннер
  (см. `package.json` scripts — обычно `npm test` / `npx vitest run`;
  используй то, что реально настроено, `vitest.config.ts` уже есть).
- TS компилируется: `npx tsc --noEmit` (или build-скрипт из package.json)
  чистый.

## Definition of done

- types/replication.ts + builders/replication.ts + экспорт + vitest-тесты.
- Все wire-формы соответствуют Rust serde (особенно SubAction externally-
  tagged и опущение optional-ключей).
- vitest зелёный, tsc чистый.
- Финальное сообщение: тронутые файлы, как запускались тесты, их вывод.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
