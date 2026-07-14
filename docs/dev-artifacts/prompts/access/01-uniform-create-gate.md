בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.4b (A2) — единообразный гейт на create-путях (аддитивно)

## Цель
Закрыть дыры: `handle_create_db` / `handle_create_repo` / `handle_create_table`
не зовут `authorize_access` на create-пути. Добавить `Action::Create` на родителе.
**Аддитивно**: пока дефолтный mode = `0o777` (OPEN), гейт пропускает всех — тесты
остаются зелёными. Это подготовка к G.4c (смене дефолта).

## Прецедент (ТОЧНО зеркалить)
`handle_create_function_folder` (admin_function.rs:191) уже гейтит:
```rust
let err_access =
    |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());
...
self.shamir
    .authorize_access(&self.actor, &parent_path, Action::Create)
    .await
    .map_err(err_access)?;
```
Этот паттерн уже зелёный в e2e (admin-актор проходит Create под OPEN) — значит
те же правки для db/repo/table тоже безопасны.

## Срез — 3 правки (гейт ПЕРЕД мутацией, ПОСЛЕ duplicate-check)

### 1. `handle_create_db` (admin_db_repo.rs:16)
- parent = `ResourcePath::Root`.
- Добавить closure `err_access` (его нет; есть только `err_code`).
- Вставить гейт ПОСЛЕ has_db/if_not_exists-блока (стр.39) и ПЕРЕД
  `self.shamir.create_db_as(...)` (стр.40):
```rust
self.shamir
    .authorize_access(&self.actor, &ResourcePath::Root, Action::Create)
    .await
    .map_err(err_access)?;
```
- Импорты `ResourcePath`, `Action` — проверь шапку admin_db_repo.rs; если нет —
  добавь в шапку (`use shamir_types::access::{Action, ResourcePath};` или как уже
  принято в соседних admin_*.rs — свериться с admin_function.rs шапкой).

### 2. `handle_create_repo` (admin_db_repo.rs:117)
- parent = `ResourcePath::Database { db: self.db_name.clone() }`.
- `err_access` closure (если в этом методе нет — добавить).
- Гейт ПОСЛЕ has_repo/if_not_exists-проверки, ПЕРЕД фактическим созданием репо.

### 3. `handle_create_table` (admin_table_index.rs:14)
- parent = `ResourcePath::Store { db: self.db_name.clone(), store: op.repo.clone() }`.
- **Удалить TODO-комментарий** (admin_table_index.rs:29-36 «authz gap … deferred»)
  и заменить его реальным гейтом.
- `err_access` closure (метод имеет `err`+`err_code`; добавь `err_access`).
- Гейт ПОСЛЕ has_table/if_not_exists-блока (стр.57), ПЕРЕД `add_table_as` (стр.60).

## Проверка корректности parent-путей
- `ResourcePath::Store { db, store }` — репо-уровень (таблица создаётся ВНУТРИ
  репо). Свериться с тем, как Table-путь строится в соседних handler'ах
  (`ResourcePath::Table { db, store, table }` → его parent = Store). Если в
  кодовой базе репо-узел зовётся иначе (Store vs Repo) — использовать тот вариант,
  что компилится и совпадает с `path.parent()` в access.rs.
- `db_name` — поле `self.db_name` (см. как другие методы его берут).

## Тест (по 1 на каждый путь — гейт ВЫЗВАН, под OPEN ПРОПУСКАЕТ)
Добавить Rust integration-тесты (где уже живут access-тесты:
`crates/shamir-db/src/shamir_db/tests/` — посмотри `access_meta_tests.rs`,
`access_tree_tests.rs` как образец вызова create через executor с актором).
- Для каждого create (db/repo/table): создать как `Actor::System` ИЛИ как актор,
  у кого права под OPEN → УСПЕХ (гейт прозрачен). Это доказывает, что гейт
  добавлен и не ломает зелёный путь.
- НЕ добавляй negative-тест (deny) здесь — он для G.4d (после смены дефолта).
  Под OPEN deny невозможен.
- Если чистый integration-тест дорог — достаточно убедиться, что СУЩЕСТВУЮЩИЕ
  тесты (которые создают db/repo/table) остаются зелёными: это и есть
  доказательство аддитивности. Тогда новые тесты можно не плодить — но тогда
  ОБЯЗАТЕЛЬНО прогнать полный gate ниже.

## Гейт (ОБЯЗАТЕЛЬНО — широкий, т.к. трогаем общий путь)
- `cargo fmt -p shamir-db -- --check`
- `cargo clippy -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-db --full`  (lib + integration — create-пути под нагрузкой)
- e2e (создаёт db/repo/table как admin — критично, что гейт пропускает):
  ```
  cd crates/shamir-client-ts && \
  SHAMIR_SERVER_BIN=D:/dev/rust/.cargo-target/debug/shamir-server.exe \
  npx vitest run e2e-ddl e2e-permissions 2>&1 | tail -40
  ```
  ⚠️ Если правка трогает серверный код — пересобери debug-сервер ПЕРЕД e2e:
  `cargo build -p shamir-server` (бинарь в D:/dev/rust/.cargo-target/debug/).
  Все тесты должны остаться зелёными (admin проходит Create под OPEN).

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER git reset/checkout/clean/stash/restore/rm или любую мутирующую git-команду.
  Только редактируй файлы. НЕ коммить — коммитит оркестратор.
- НЕ меняй дефолтный mode (это G.4c, отдельная фаза!). Только ДОБАВЬ гейт-вызовы.
  use в шапке. Surgical changes — не трогай несвязанный код.
- Заверши финальным текстом: изменённые файлы + вывод ВСЕГО гейта (включая e2e).

## Коммит (оркестратор, после zero-trust verify)
`feat(access): G.4b — uniform Action::Create gate on create_db/repo/table`
