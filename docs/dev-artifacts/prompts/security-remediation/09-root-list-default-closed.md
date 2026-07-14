# Brief: Root/List default mode 0o755 → 0o750 (taskId #620, часть #615)

## Контекст

`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`, `resource_meta`'s
`ResourcePath::Root` branch (строка ~137-148) — дефолтный (absent
`"root_meta"` setting) mode = `0o755` (owner=System rwx, group r-x,
**other r-x**). `Action::List` мапится на Read-класс прав; other имеет r
(Read) бит — значит ЛЮБОЙ аутентифицированный non-owner/non-System актёр
может листать топ-level базы данных (имена БД перечислимы) по умолчанию.

Пользователь (2026-07-14) решил: сделать дефолт закрытым. Флип
`0o755` → `0o750` (owner rwx, group r-x, **other ---**) — убирает ВСЕ
other-биты; оператор, которому нужен открытый листинг, может явно
`chmod`'нуть обратно на `0o755`/`0o777` через `set_resource_meta`.

## Задача

### 1. Флип дефолта

`access_control.rs`, `resource_meta`'s `ResourcePath::Root` (строка
~139-143):

```rust
Ok(None) => Ok(ResourceMeta {
    owner: Actor::System,
    group: None,
    mode: 0o750, // was 0o755 — task #615/#620: close default enumeration
}),
```

Обнови doc-комментарий над этой веткой (строки ~132-136) и над
`build_net_gateway`-подобными местами, где `0o755` упоминается как
текущий дефолт Root, если такие есть в комментариях рядом (grep
`0o755` в этом файле, поправь только те, что говорят про Root).

### 2. Тесты — обнови буквальные assertions на новый дефолт

Каждый файл ниже содержит буквальные `0o755`, ПРИМЕНИМЫЕ К ROOT (не
путай с `WasmCompiler` — тоже дефолт `0o755`, НЕ трогать её — и не с
`Mode::with_setuid(0o755, true)` в `enforcement_tests.rs`, это про
Function definer setuid-бит, другая тема, НЕ трогать):

- `crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs`:
  строки ~160,173 (тест `root_meta_survives_reopen`/похожий — если это
  про `set_resource_meta`+явный mode, а не про дефолт, возможно не нужно
  трогать — ПРОВЕРЬ, меняет ли тест дефолт или явно задаёт mode) и
  строки ~239-250 (`root_meta_defaults_to_system_0o755` — переименуй в
  `root_meta_defaults_to_system_0o750`, поправь assert).
- `crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs:385`
  — проверь контекст, если это про Root-дефолт — поправь.
- `crates/shamir-db/src/shamir_db/tests/root_user_group_meta_tests.rs`:
  строка ~87 (`root_meta_defaults_to_system_0o755_when_absent` →
  переименовать + поправить assert на 0o750), строки ~75,102,125,154 —
  проверь по контексту: если это ЯВНО заданный mode в
  `set_resource_meta` (не дефолт) — не трогай, оставь 0o755 как явное
  значение теста.
- `crates/shamir-db/tests/access_ddl.rs:395,420` — аналогично, проверь
  дефолт vs явное значение.
- `crates/shamir-db/tests/create_function_gating.rs:50` — комментарий
  "Root's default meta is { owner: System, mode: 0o755 }" — поправь текст
  комментария на 0o750 (проверь, влияет ли это на сам тест — если тест
  полагается на то, что "regular user" не имеет прав — с 0o750 это ещё
  СИЛЬНЕЕ верно, тест не должен сломаться, только уточни комментарий).

### 3. `coverage_matrix_tests.rs`

Этот файл (уже трогали в задаче #611, там же таблица) — `Root/List` cell
сейчас помечен `(open)` с комментарием "Root/List stays open ('0o755'
keeps the Read bit for Other unchanged)". С новым дефолтом `0o750` (no
Other bits) — `List` для no-rights actor теперь ДОЛЖЕН быть DENIED.
Обнови:
- Таблицу в doc-комментарии: `Root` row, `List` column — `(open)` → `X`.
- Комментарий-абзац, объясняющий `(open)` cells (строки ~43-58) — убери
  Root/List из перечисления открытых cells, если он там явно называется.
- Добавь assertion в соответствующий тест (там где остальные `X` cells
  проверяются на denial для no-rights actor) для `(Root, List)`.
- Есть companion-тест `_allows_open_default_cells` (упомянут в
  комментарии) — если `(Root, List)` там ассертится как allowed, перенеси
  его в "denied" тест вместо этого.

## Прогон проверок

- `cargo fmt -p shamir-db -- --check`
- `cargo clippy -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-db --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `ResourcePath::WasmCompiler`'s дефолт (тоже `0o755`, но это
  отдельный ресурс из задачи #607, не в scope).
- НЕ трогай `Mode::with_setuid(0o755, true)` в `enforcement_tests.rs` —
  это про Function definer setuid, другая тема.
- НЕ меняй self-lockout guardrail в `set_resource_meta`'s `ResourcePath::Root`
  ветке (строки ~331-344 — "chmod would leave Root owned by non-System
  owner without owner-Execute") — она про owner-Execute, не про
  Other-биты, дефолт-флип её не затрагивает.
- Для каждого `0o755` в тестах — СНАЧАЛА определи, дефолт это (нужно
  менять) или явно заданное тестом значение через `set_resource_meta`
  (оставить как есть, тест специально проверяет явный chmod). Не меняй
  вслепую всё найденное по grep.

## Проверка (сделает оркестратор)

- Диф ограничен `access_control.rs` + перечисленными тестовыми файлами.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-db --full` зелёный.
- `WasmCompiler`'s тесты (`wasm_compiler_permission_tests.rs`) не задеты
  и остаются зелёными без изменений.
