# Brief: WASM-compile как отдельное право доступа (taskId #607, переформулированный CRITICAL)

## Контекст и директива пользователя

Аудит изначально формулировал это как CRITICAL: `crates/shamir-wasm-host/src/compile.rs`
компилирует НЕДОВЕРЕННЫЙ Rust-исходник хоста через host `cargo build` (см.
doc-комментарий модуля, строки 1-30) без полной OS-изоляции (нет
container/seccomp/rlimit/отдельного низкопривилегированного процесса) —
рекомендация была "изолировать компилятор в отдельный процесс/контейнер".

**Пользователь (2026-07-14) явно переформулировал требование**: это НЕ
должно решаться OS-sandbox'ом. Вместо этого — компиляция WASM должно быть
**отдельным правом доступа**, гейтящимся как обычный POSIX-style mode-bit
("как в линуксе") в уже существующей ACL-модели этого репозитория, с
дефолтным режимом **0755** (как у `Root`, см. ниже — это ПРЕЦЕДЕНТ, не
изобретение с нуля).

## Существующая инфраструктура (полностью готова, только не переиспользована)

`crates/shamir-types/src/access.rs` — `ResourcePath` enum (строка ~371) уже
имеет паттерн для ГЛОБАЛЬНЫХ singleton-ресурсов с persisted mode:

- `ResourcePath::Root` (`access_control.rs:137-148`) — persisted meta по
  ключу `"root_meta"` в `system_store.load_setting`/`save_setting`,
  дефолт (если ключ отсутствует) — **`ResourceMeta { owner: Actor::System, group: None, mode: 0o755 }`**
  — ЭТО ТОЧНО ТА СЕМАНТИКА, КОТОРУЮ ПРОСИТ ПОЛЬЗОВАТЕЛЬ, уже есть в коде,
  просто для другого ресурса.
- `ResourcePath::FunctionNamespace` (`access_control.rs:122-131` для read,
  `access_control.rs:282-293` для write) — тот же паттерн, персистится под
  ключом `"fn_namespace_meta"`, дефолт `ResourceMeta::default()` (open).

`Action::Execute` (`crates/shamir-types/src/access.rs:588-596`) уже
существует как enum-вариант, маппится на POSIX `Perm::Execute` (bit "x")
— уже используется для "разрешить вызов КОНКРЕТНОЙ функции"
(`ResourcePath::Function{name}` + `Action::Execute`, см.
`function_management.rs:620,659,708,762`). Переиспользование `Execute`
для НОВОГО ресурса (`WasmCompiler`, глобальный singleton, не
per-function) — НЕ семантическая коллизия: это как POSIX переиспользует
бит "x" и для "выполнить файл", и для "войти в директорию" — один и тот
же бит, разный смысл в зависимости от типа ресурса, который проверяется.

`create_function_with_opts_as` (`crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:135-172`)
уже содержит ТОЧНО такой прецедент эскалации для недомоделированного
класса риска — `secret_grants` (строки 145-160):

```rust
if !opts.secret_grants.is_empty() {
    self.authorize_access(&actor, &ResourcePath::Root, Action::Manage)
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
}
let (wasm, lang_tag, source_str) = match source {
    FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
    FunctionSource::Source(src) => {
        let compiled = compile_rust_source(src).map_err(|e| match e { ... })?;
        (compiled, "rust", Some(src.to_string()))
    }
};
```

## Задача

### 1. Новый securable resource — `ResourcePath::WasmCompiler`

В `crates/shamir-types/src/access.rs`:

- Добавить вариант в enum `ResourcePath` (рядом с `FunctionNamespace`,
  строка ~402):
  ```rust
  /// Global singleton gating the "compile Rust source into WASM"
  /// capability (task #607) — separate from `FunctionNamespace`'s
  /// bare Create right. Mode-bearing, POSIX-style ("x" bit = may
  /// trigger host compilation), default `0o755` (mirrors `Root`'s
  /// default — see `resource_meta`'s `ResourcePath::Root` arm).
  WasmCompiler,
  ```
- `parent()` (строка ~498): добавить `ResourcePath::WasmCompiler => Some(ResourcePath::Root),`
  (рядом с `ResourcePath::FunctionNamespace => Some(ResourcePath::Root),`).
- `Display` impl (строка ~573): добавить
  `ResourcePath::WasmCompiler => f.write_str("compiler://"),`.

### 2. `resource_meta`/`set_resource_meta` для `WasmCompiler`

В `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`:

- В `resource_meta`, добавить ветку (рядом с `ResourcePath::Root`'s
  веткой, строки ~137-148 — СКОПИРОВАТЬ ТОЧНО ЭТОТ ПАТТЕРН, только другой
  settings-ключ):
  ```rust
  // WasmCompiler — persisted meta, settings key "wasm_compiler_meta".
  // Mirrors Root's default: absent key -> System-owned, 0o755 (task
  // #607 — user's explicit directive: POSIX-style mode gate, not an
  // OS sandbox).
  ResourcePath::WasmCompiler => {
      match self.system_store.load_setting("wasm_compiler_meta").await {
          Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
          Ok(None) => Ok(ResourceMeta {
              owner: Actor::System,
              group: None,
              mode: 0o755,
          }),
          Err(e) => {
              log::warn!("resource_meta: failed to load wasm compiler meta: {e}");
              Err(e)
          }
      }
  }
  ```
- В `set_resource_meta`, добавить симметричную write-ветку (рядом с
  `ResourcePath::FunctionNamespace`'s веткой, строки ~282-293 — тот же
  паттерн, без Root'овского self-lockout guardrail, он специфичен для
  Root и не нужен здесь):
  ```rust
  ResourcePath::WasmCompiler => {
      let mut m = shamir_types::types::common::new_map();
      m.insert(
          "key".to_string(),
          QueryValue::Str("wasm_compiler_meta".to_string()),
      );
      let mut rec = QueryValue::Map(m);
      meta.inject_into(&mut rec);
      self.system_store
          .save_setting("wasm_compiler_meta", &rec)
          .await
  }
  ```

### 3. Гейт в `create_function_with_opts_as`

В `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs`,
ТОЛЬКО в ветке `FunctionSource::Source(src)` (НЕ трогай `FunctionSource::Wasm`
— загрузка уже скомпилированного WASM не запускает host-компилятор, вне
scope этой задачи):

```rust
let (wasm, lang_tag, source_str) = match source {
    FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
    FunctionSource::Source(src) => {
        // Task #607: compiling Rust source into WASM runs a host
        // compiler process — gate it as a separate POSIX-style
        // permission (Execute bit on the WasmCompiler singleton),
        // not folded into bare FunctionNamespace Create. Mirrors the
        // secret_grants escalation pattern above.
        self.authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)
            .await
            .map_err(|e| DbError::Function(e.to_string()))?;
        let compiled = compile_rust_source(src).map_err(|e| match e {
            FunctionError::ToolchainUnavailable(msg) => {
                DbError::Function(format!("toolchain unavailable: {}", msg))
            }
            other => DbError::Function(other.to_string()),
        })?;
        (compiled, "rust", Some(src.to_string()))
    }
};
```

Порядок проверок: сначала уже существующий `authorize_access(..., FunctionNamespace, Create)`
(строка 142, не трогать), потом `secret_grants` эскалация (строки
156-160, не трогать), потом НОВЫЙ WasmCompiler-гейт (только для
`Source`-ветки), потом сама компиляция.

### 4. Обнови doc-комментарий модуля `compile.rs`

`crates/shamir-wasm-host/src/compile.rs:8-30` — после существующего
"## Security posture" блока добавь абзац, что defense-in-depth теперь ДВА
слоя: (1) permission-гейт ДО вызова этой функции (task #607 — только
актёры с `Execute` на `WasmCompiler` могут триггерить компиляцию,
проверяется в `function_management.rs`, НЕ здесь — этот модуль остаётся
policy-agnostic), (2) существующие forbidden-macro scan / env allowlist /
timeout ВНУТРИ самого процесса компиляции (уже реализовано). Не меняй
код `compile.rs` — это только doc-комментарий, сам permission-check живёт
на уровень выше, в `shamir-db`.

## Тесты

В `crates/shamir-db/src/shamir_db/tests/` — найди подходящий существующий
файл (`coverage_matrix_tests.rs` или создай тест рядом с `access_meta_tests.rs`
паттерном, смотри как там тестируется `resource_meta`/`authorize_access`
для `Root`/`FunctionNamespace` — используй тот же стиль):

1. Дефолт: `resource_meta(&ResourcePath::WasmCompiler)` без предварительного
   `set_resource_meta` возвращает `owner: Actor::System, mode: 0o755`.
2. `authorize_access` неким non-System `Actor::User(id)` на
   `(WasmCompiler, Execute)` — под дефолтным 0o755 (other has execute bit)
   должно быть `Ok(())` (0o755 = everyone-execute по дизайну, "как в
   линуксе").
3. После `set_resource_meta(&WasmCompiler, ResourceMeta{mode: 0o700, ...})`
   (owner-only) — тот же non-System actor теперь получает `Err` на
   `Execute`.
4. **End-to-end**: `create_function_with_opts_as(..., FunctionSource::Source(valid_rust), ..., non_system_actor)`
   — под 0o700-ужесточённым `WasmCompiler` (actor не owner) должно
   вернуть `Err` ДО того, как реально запустится `cargo build` (проверь
   через таймер/мок, что компиляция не стартовала — или просто проверь
   текст ошибки соответствует permission-denial, не toolchain/compile
   error). Под дефолтным 0o755 — тот же вызов должен УСПЕШНО
   скомпилироваться (не сломай существующий рабочий путь!).
5. `FunctionSource::Wasm(bytes)` путь — убедись, что он НЕ требует
   `WasmCompiler`-права вообще (существующие WASM-upload тесты должны
   остаться зелёными без изменений).

## Прогон проверок

- `cargo fmt -p shamir-types -p shamir-db -- --check`
- `cargo clippy -p shamir-types -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-types -p shamir-db --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-wasm-host/src/compile.rs`'s код (только
  doc-комментарий) — сам процесс компиляции (forbidden-macro scan, env
  allowlist, timeout) уже правильно реализован, не в scope.
- НЕ добавляй OS-sandbox/seccomp/container/rlimit — пользователь явно
  отклонил этот подход в пользу permission-гейта.
- НЕ трогай `FunctionSource::Wasm` путь — только `Source`.
- НЕ добавляй wire/chmod DDL exposure для `WasmCompiler` (нет `ResourceRef::WasmCompiler`
  варианта в `crates/shamir-query-types/src/admin/access.rs`) — `Root`
  сам по себе тоже не chmod-абелен через wire сегодня (нет
  `ResourceRef::Root`), так что это последовательно с существующим
  прецедентом и оставлено как отдельный follow-on, не в scope этой
  задачи. Chmod для `WasmCompiler` пока доступен только через embedded
  Rust API (`set_resource_meta` напрямую).

## Проверка (сделает оркестратор)

- Диф ограничен `access.rs` (types), `access_control.rs`,
  `function_management.rs` (оба shamir-db), `compile.rs` (только
  doc-комментарий), плюс новые тесты.
- fmt/clippy по `shamir-types`/`shamir-db` чисты.
- `./scripts/test.sh -p shamir-types -p shamir-db --full` зелёный,
  включая новые 5 тестов.
- Существующий рабочий путь создания функции из Rust-исходника (под
  дефолтным 0o755) не сломан — все существующие
  `create_function_from_source`/`create_function_with_opts` тесты
  остаются зелёными.
