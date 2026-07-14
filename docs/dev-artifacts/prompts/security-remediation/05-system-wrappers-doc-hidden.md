# Brief: System wrappers → `#[doc(hidden)]` (taskId #606, audit residual, P2)

## Контекст

Три convenience-функции жёстко используют `Actor::System` и уже несут
предупреждающий `SAFETY (wire-reachability, task #546)` doc-комментарий,
но остаются `pub`:

- `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:25` — `pub async fn create_db`
- `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:332` — `pub async fn add_repo`
- `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:152` — `pub async fn rename_table`

Изначальная рекомендация ревью — `pub(crate)` — **на практике сломала бы
50+ интеграционных тестовых файлов** (`crates/shamir-db/tests/*.rs` —
отдельный компилируемый крейт, не видит `pub(crate)`) и легитимный
boot-путь `crates/shamir-server/src/server/server_launcher.rs:292,300`
(вызывает `create_db("default")`/`add_repo("default", ...)` при старте
сервера, до появления любого реального актёра). Пользователь явно решил:
делать `pub(crate)` НЕ надо, вместо этого — `#[doc(hidden)]` (дёшево, без
блэст-радиуса, убирает функции из публичной rustdoc-документации/API
discovery — не техническая защита, а «не свети в витрине»).

## Задача

Для каждой из трёх функций (`create_db`, `add_repo`, `rename_table`):

1. Добавь атрибут `#[doc(hidden)]` НЕПОСРЕДСТВЕННО перед сигнатурой
   функции (после doc-комментария, `#[doc(hidden)]` — обычный атрибут,
   не часть doc-comment блока):
   ```rust
   /// ...существующий doc-комментарий с SAFETY-блоком...
   #[doc(hidden)]
   pub async fn create_db(&self, name: &str) -> DbInstance {
       ...
   }
   ```
2. НЕ меняй саму сигнатуру (остаётся `pub`, просто скрыта из rustdoc) —
   не трогай `pub` → `pub(crate)`, не добавляй параметров.
3. В конце существующего SAFETY-doc-комментария каждой функции добавь
   одну строку, поясняющую ПОЧЕМУ `#[doc(hidden)]`, а не `pub(crate)`:
   ```
   /// // `#[doc(hidden)]` (not `pub(crate)`): narrowing visibility would
   /// // break 50+ integration test files (a separate compiled crate) and
   /// // shamir-server's legitimate boot-time `create_db`/`add_repo` calls
   /// // in `server_launcher.rs` — hiding from public rustdoc/API
   /// // discovery is the achievable P2 mitigation here (task #606).
   ```
   (Адаптируй последнее предложение под конкретную функцию — например,
   `rename_table` не вызывается из `server_launcher.rs`, так что для неё
   упомяни только тестовый blast radius, если это единственная причина —
   проверь сам через `grep -rn "\.rename_table(" crates/ --include="*.rs" | grep -v "shamir-db/src\|shamir-db/tests"` чтобы убедиться, есть ли у неё
   продакшен-вызывающие вне тестов.)

## Прогон проверок

- `cargo doc -p shamir-db --no-deps 2>&1 | grep -i warn` — убедись, что
  `#[doc(hidden)]` не порождает предупреждений (обычно не порождает).
- `cargo fmt -p shamir-db -- --check`
- `cargo clippy -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-db --full` — обязан остаться зелёным
  (ничего не должно сломаться, т.к. `#[doc(hidden)]` не меняет
  видимость для компиляции, только для документации).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ меняй `pub` на `pub(crate)` ни у одной из трёх функций — явное
  решение пользователя отклонить этот вариант из-за blast radius.
- НЕ трогай другие функции в этих файлах, даже если у них похожий
  паттерн (`create_db_as`/`add_repo_as`/`rename_table_as` — принимающие
  явный `actor` параметр — НЕ System-only wrappers, вне scope).

## Проверка (сделает оркестратор)

- Диф ограничен ровно тремя функциями (атрибут + одна строка комментария
  каждая).
- `cargo doc -p shamir-db --no-deps`, fmt, clippy чисты.
- `./scripts/test.sh -p shamir-db --full` зелёный (ничего не должно было
  сломаться).
