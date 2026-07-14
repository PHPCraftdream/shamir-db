# Brief: net_grants restrictive default + wire surface (taskId #609, P2)

## Контекст и директива пользователя

`crates/shamir-db/src/shamir_db/shamir_db/core.rs:704-742`, метод
`build_net_gateway` — пустой `net_grants` СЕЙЧАС означает "нет
function-level ограничения" (функция получает ПОЛНЫЙ DB-wide
`net_allowlist`), в отличие от `secret_grants` (пусто = НЕТ секретов —
restrictive default). Существующий развёрнутый doc-комментарий объясняет
это как СОЗНАТЕЛЬНЫЙ выбор ради обратной совместимости с уже
задеплоенными функциями.

**Пользователь (2026-07-14) явно отменил это обоснование**: релизов ещё
не было, обратную совместимость сохранять не нужно — "если что-то нужно
поменять, меняем без оглядки на обратную совместимость". Значит:
привести `net_grants` к ТОЙ ЖЕ restrictive-семантике, что уже есть у
`secret_grants` (пусто = НЕТ доступа), И одновременно закрыть wire-пробел
(`crates/shamir-db/src/shamir_db/execute/admin_function.rs:63` —
`net_grants: Vec::new()` — жёстко захардкожено, wire `CreateFunction`
вообще не может задать `net_grants`).

## Задача

### 1. Флип дефолта в `build_net_gateway`

`crates/shamir-db/src/shamir_db/shamir_db/core.rs`, метод
`build_net_gateway` (строки ~704-745):

- Убрать fallback "empty/None → full net_allowlist". Новая семантика:
  `None` (нет catalogue-записи, builtin) — оставить как full allowlist
  (это не то же самое что "explicit empty grants", builtin-функции не
  проходят через `CreateFunctionOptions` вообще, не путать с
  пользовательскими функциями). `Some(grants) if grants.is_empty()` —
  теперь означает **НЕТ egress вообще** (пустой intersection = пустой
  вектор, не `net_allowlist.to_vec()`).
- Перепиши doc-комментарий метода: убери весь блок про "backward
  compatibility, no migration path" (директива пользователя отменяет это
  обоснование) — замени на короткое: "Task #609: empty `net_grants` now
  means NO egress, matching `secret_grants`'s restrictive-by-default
  precedent. No backward-compatibility constraint applies (pre-release)."
- Секция "Intersection is literal-string, not pattern-aware" — оставь как
  есть, это отдельное, всё ещё актуальное known-limitation, не в scope
  этой задачи.

### 2. Wire-surface: `net_grants` в `CreateFunctionOp`

`crates/shamir-query-types/src/admin/types/function_ops.rs`,
`struct CreateFunctionOp` (строки 24-54) — добавь поле рядом с
`secret_grants`:

```rust
/// Egress allowlist for this function, INTERSECTED with the DB-wide
/// `net_allowlist` (can only narrow, never exceed the DB ceiling).
/// Absent/empty means NO egress for this function (task #609 — matches
/// `secret_grants`'s restrictive-by-default precedent; no backward-
/// compatibility default, this repo has not released yet).
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub net_grants: Vec<String>,
```

Обнови doc-комментарий блока над структурой (строки 12-16), упомянув
`net_grants` рядом с `secret_grants`.

### 3. Прокинуть через `handle_create_function`

`crates/shamir-db/src/shamir_db/execute/admin_function.rs:58-64`:

```rust
let opts = CreateFunctionOptions {
    replace: op.replace,
    visibility,
    security,
    secret_grants: op.secret_grants.clone(),
    net_grants: op.net_grants.clone(), // task #609: wire surface added
};
```

(Убери комментарий "unchanged — net_grants has its own separate wiring
(task #544)" — он больше не актуален.)

### 4. HMAC — НЕ трогать

`net_grants` — ТОЛЬКО сужающая capability (никогда не может дать больше,
чем DB allowlist), в отличие от `secret_grants` (эскалирующая, доступ к
хостовым секретам). Существующий `canonical_create_function(name, security, secret_grants)`
не включает `net_grants` и НЕ должен — не добавляй HMAC-требование для
`net_grants` (это не эскалация привилегий, сужение egress не нуждается в
"did-you-mean-it" подтверждении). Если это решение спорно — оставь как
есть и не расширяй scope без необходимости.

### 5. Тесты — почини существующие, добавь новый

`crates/shamir-db/tests/functions_lifecycle.rs`:

- Тест `net_grants_empty_falls_back_to_db_allowlist` (строки ~876-917) —
  **ПЕРЕПИШИ семантику**: с новым restrictive-дефолтом пустой
  `net_grants` теперь означает "функция НЕ может выйти в сеть вообще",
  не "падает на полный DB allowlist". Переименуй тест в
  `net_grants_empty_denies_all_egress` (или похоже) и поправь assertion
  на противоположный — запрос ДОЛЖЕН быть denied, а не succeed.
  Обнови doc-комментарий над тестом, убрав описание старой семантики.
- Остальные `net_grants: Vec::new()` литералы в этом файле (строки
  ~1099, 1154, 1212) и в `crates/shamir-db/tests/create_function_gating.rs`
  (строки ~62, 101) — проверь по одному: если тест **не про сетевой
  доступ** (например, тестирует visibility/security/create-gating), пустой
  `net_grants` там нейтрален и ничего не сломает (функция просто не
  выходит в сеть, что тест и так не проверяет) — оставь как есть. Если
  тест **делает реальный сетевой вызов** через такую функцию и ожидает
  успеха — добавь `net_grants: vec!["<конкретный host>".to_string()]`
  соответствующий проверяемому хосту, чтобы тест не сломался нарочно (не
  потому что фикс неправильный, а потому что тест раньше неявно полагался
  на старый permissive-дефолт).
- Добавь новый тест (рядом с `net_grants_narrower_than_db_allowlist_denies_outside_host`,
  ~строка 932) — `create_function_with_empty_net_grants_via_wire_gets_no_egress`
  или похожий: через `handle_create_function`/`BatchOp::CreateFunction`
  (wire-путь, не embedded API) с `net_grants: vec![]` (дефолт) — функция
  не может выйти НИ на один хост из DB allowlist, даже разрешённый.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-db -- --check`
- `cargo clippy -p shamir-query-types -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-db --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `secret_grants`'s логику/HMAC-гейт — уже правильно.
- НЕ трогай "literal-string intersection" известное ограничение — отдельная,
  недокументированная как часть ЭТОЙ задачи проблема.
- НЕ добавляй HMAC-требование для `net_grants` — обоснование в п.4 выше.
- НЕ трогай builtin-функций путь (`None` case в `build_net_gateway`) —
  остаётся full allowlist, это другой код-путь, не тот же самый
  "explicit empty grants" пользовательской функции.

## Проверка (сделает оркестратор)

- Диф ограничен `core.rs`, `function_ops.rs`, `admin_function.rs`, плюс
  тестовые правки в `functions_lifecycle.rs`/`create_function_gating.rs`.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-query-types -p shamir-db --full` зелёный.
- Переписанный тест `net_grants_empty_denies_all_egress` (или как назовёшь)
  реально проверяет НОВОЕ поведение (deny), не тавтологичен.
