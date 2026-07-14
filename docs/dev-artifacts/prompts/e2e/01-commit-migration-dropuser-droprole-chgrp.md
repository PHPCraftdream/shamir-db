בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.3 (C3) — тонкое e2e: commit-migration / dropUser-dropRole / chgrp

## Цель
Закрыть e2e-пробелы headline lifecycle-операций. Сейчас:
- e2e-миграция гоняет ТОЛЬКО start→status→**rollback** (нет commit-пути).
- `dropUser`/`dropRole` — создаются в e2e, но НЕ дропаются.
- `chgrp` — unit-only, нет e2e на эффект.

**Только тесты — кода фич нет.** Расширяем существующие файлы (НЕ новые).

## Инфраструктура (e2e через debug-сервер)
- Debug-бинарь сервера уже собран оркестратором. **Кастомный target-dir** (см.
  `.cargo/config.toml`): бинарь лежит в
  `D:\dev\rust\.cargo-target\debug\shamir-server.exe` (НЕ в repo `target/`).
- Запуск e2e: из `crates/shamir-client-ts`, с env
  `SHAMIR_SERVER_BIN=D:/dev/rust/.cargo-target/debug/shamir-server.exe`
  (harness `serverBinPath()` берёт его первым приоритетом; без него
  `SERVER_AVAILABLE=false` и весь suite `describe.skipIf` пропустится — это
  значит «не проверено», НЕ «зелено»). node_modules уже установлены.
- Команда (bash):
  ```
  cd crates/shamir-client-ts && \
  SHAMIR_SERVER_BIN=D:/dev/rust/.cargo-target/debug/shamir-server.exe \
  npx vitest run e2e-ddl e2e-permissions 2>&1 | tail -60
  ```
  Убедись, что тесты РЕАЛЬНО исполнились (PASS, не SKIP). Если видишь "skipped"
  по всему suite — SHAMIR_SERVER_BIN не подхватился, чини env.

## Заземление (file:line, всё уже существует)

### Билдеры
- `commitMigration(signer, dbInUse, migrationId)` — ddl.ts:602 (HMAC).
- `startMigration(signer, dbInUse, srcRepo, table, dstRepo, dstEngine, opts?)` —
  ddl.ts:574. `migrationStatus(migId)` — ddl.ts:232.
- `dropUser(signer, username, opts?)` — admin.ts:205 (HMAC; canonical внутри).
- `dropRole(signer, role, opts?)` — admin.ts:227 (HMAC).
- `createUser(name, password, opts?)` — admin.ts:189; `createRole(name, perms)` —
  admin.ts:219; `chgrp(resource, group: number|null)` — admin.ts:121;
  `createGroup(name)` — admin.ts:125; `accessTree({db})` — admin.ts:164;
  `admin.refTable/refDatabase` — для ResourceRef.
- **HMAC-signer = сам клиент**: `dropUser(adminClient, name)` (см. e2e.test.ts:559
  "client IS the HmacSigner", `ddl.dropTable(client!, db, 'main', t)`).

### Серверная семантика (подтверждена чтением)
- `handle_start_migration` синхронно гонит run_snapshot→drain→`mark_cutover_ready`
  и возвращает `phase: "cutover_ready"` (admin_migration.rs:175). Значит
  `commitMigration` СРАЗУ после `startMigration` проходит (final_drain требует
  CutoverReady — оно уже выставлено).
- `handle_commit_migration` (admin_migration.rs:185): финал-дрейн, bulk-populate
  index2, **удаляет миграцию из active map** → последующий `migrationStatus(migId)`
  вернёт `not_found` (код "not_found"). Результат commit:
  `{ migration_id, phase: "committed", tail_drained }`.
- `accessTree` shape (e2e-permissions.test.ts:321): `records[0].access_tree =
  { resources: {name, kind, owner, mode, children, [group]}, functions, principals }`.

### Образцы (читай перед написанием)
- Миграция e2e (rollback): `e2e-ddl.test.ts:424-464` — точный паттерн setupDb→
  seed→createRepo(dst,in_memory)→startMigration→migrationStatus.
- Permissions suite: `e2e-permissions.test.ts` — `describe.skipIf(!SERVER_AVAILABLE)`,
  beforeAll startServer/connectAdmin, helper `createUserAndConnect`, A8 createRole
  (:243), A9 accessTree (:314). Хелперы harness: `setupDb`, `seed`, `br`,
  `connectAs`, `uniqueDbName`.

## Срез — 3 теста

### 1. Migration COMMIT — расширить `e2e-ddl.test.ts`
Добавить новый `it()` СРАЗУ после rollback-теста (после строки 464), внутри того
же describe. Паттерн (по образцу rollback-теста):
- `setupDb(client!, 'ddl_migc', ['migdata'])`; `seed` 2-3 строк с известными id.
- `createRepo('dst_repo', { engine: 'in_memory' })`.
- `startMigration(client!, db, 'main', 'migdata', 'dst_repo', 'in_memory')` →
  assert result `phase === 'cutover_ready'`, достать `migration_id`.
- `commitMigration(client!, db, migId)` → assert result `phase === 'committed'`.
- **Проверка применённости**: прочитать таблицу `migdata` в `dst_repo` (Query на
  репо 'dst_repo') → вернуть все seed-строки (по id/значениям). Свериться с тем,
  как читается репо в других тестах (Query.from с repo). Если чтение конкретного
  репо в TS-билдере требует особой формы — посмотри e2e-rename-repo.test.ts.
- **Status после commit**: `migrationStatus(migId)` теперь должен дать ошибку/пусто
  ("not_found"). Ассертить мягко (поймать throw ИЛИ records пуст) — точную форму
  ответа подтверди эмпирически на запущенном сервере и зафиксируй ассерт.

### 2 & 3. dropUser / dropRole / chgrp — расширить `e2e-permissions.test.ts`
Добавить 3 `it()` внутри главного `describe.skipIf(!SERVER_AVAILABLE)`:

- **dropUser**: создать пользователя (`adminClient!.createScramUser(name, pw, [])`
  ИЛИ `admin.createUser`), затем `admin.dropUser(adminClient!, name)` в batch →
  assert без ошибки. Усиление (если просто): после дропа `connectAs(...)` этим
  юзером падает (catch) ИЛИ повторный dropUser без `if_exists` даёт ошибку, а с
  `{ if_exists: true }` — нет. Имя юзера — уникальное (`drop_u_${process.pid}`).

- **dropRole**: `admin.createRole('g3_role', [admin.permission(...)])` (см. A8 для
  формы permission) → `admin.dropRole(adminClient!, 'g3_role')` → без ошибки.
  Усиление: повторный dropRole с `{ if_exists: true }` не падает.

- **chgrp**: `admin.createGroup('g3_grp')` (достать gid из результата — посмотри
  форму ответа create_group эмпирически; если возвращает числовой id — используй
  его; иначе сверься как gid резолвится). Затем `admin.chgrp(admin.refTable(...)
  ИЛИ admin.refDatabase(db), gid)` → assert chgrp-результат эхо `group === gid`.
  **Верификация персистентности**: `admin.accessTree({ db })` → найти узел ресурса
  (для refDatabase — это корневой `resources`; для refTable — в `children`) →
  assert его `group === gid`. Если access_tree не отдаёт `group` на узле — оставь
  ассерт на chgrp-эхо + комментарий, что readback группы в access_tree отсутствует
  (limitation), НЕ выдумывай поле.

## Гейт
- Сборка debug-сервера — уже сделана оркестратором (не пересобирай; C3 не меняет
  wire/сервер).
- `cd crates/shamir-client-ts && SHAMIR_SERVER_BIN=<abs debug exe> npx vitest run
  e2e-ddl e2e-permissions` → все новые `it()` **PASS** (не skipped). Покажи вывод.
- Rust-гейт не нужен (только TS-тесты добавляются). Но если тронешь .ts вне тестов —
  не трогай. lint TS: если есть `npm run lint` — прогони; иначе пропусти.

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER git reset/checkout/clean/stash/restore/rm или любую мутирующую git-команду.
  Только редактируй файлы. НЕ коммить — коммитит оркестратор.
- Тесты ТОЛЬКО как указано (vitest для TS). Не трогай несвязанные тесты.
  Surgical changes. Queries строй ТОЛЬКО через билдеры (никакого raw JSON).
- Заверши финальным текстом: какие it() добавил (файл:имя) + вывод vitest (PASS-строки).

## Коммит (оркестратор, после zero-trust verify)
`test(e2e): G.3 C3 — commit-migration / dropUser-dropRole / chgrp`
