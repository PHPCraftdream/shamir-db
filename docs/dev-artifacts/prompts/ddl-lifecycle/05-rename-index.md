בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief — E.4-followon Phase F.1: RENAME INDEX

Делегированная стадия. Реализовать операцию **RENAME INDEX** (переименование
индекса в рамках таблицы без потери данных индекса) end-to-end: DTO → dispatch →
permission → handler → билдеры (Rust + TS) → тесты вплоть до e2e TS.

Источник плана: `docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md` §«Phase F.1 — RenameIndex».
Это самая чистая фаза (без MVCC-overlay). Образец во всём — **RENAME TABLE**
(Phase E.4, commit `a7dcda5`).

---

## ⛔ Git — ЗАПРЕЩЕНО (verbatim)

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
> or any git command that mutates the working tree or index. Only edit files;
> the orchestrator commits.

Ты НЕ коммитишь, НЕ пушишь, НЕ трогаешь версии. Только правишь файлы.

---

## Дисциплина проекта (обязательно)

- **Тесты — ТОЛЬКО через `./scripts/test.sh`** (raw `cargo test` заблокирован
  perimeter-guard'ом). Узко: `./scripts/test.sh -p shamir-db -- rename_index`.
  Вывод тестов — в ФАЙЛ, не inline-grep: `./scripts/test.sh ... > run.log 2>&1;
  rc=$?` затем grep по файлу.
- **Бенчи** (если понадобятся) — `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench`.
- **Queries — только через query-builder.** `serde_json::Value` ЗАПРЕЩЁН
  (исключения: napi/FFI, serde round-trip тесты, WASM-bridge, protocol-spec доки).
- **`scc::*::len()` ЗАПРЕЩЁН** (O(N), clippy disallowed) — если нужна
  кардинальность, AtomicUsize-mirror или `#[allow(clippy::disallowed_methods)]
  // O(N) ack: <why>`.
- **Один файл = один primary export.** `mod.rs` — только re-exports. Тесты — в
  `tests/`-директории модуля, не inline `#[cfg(test)] mod tests { ... }`.
- **`use` — в шапке файла.** `#[serde(default)]` на новых wire-полях.
- Хеши — `shamir_collections::THasher`. Concurrency — lock-free/scc/atomics, не
  `std::sync::Mutex` на hot-path.
- Стиль JSON-литералов в тестах — многострочный, с отступами.

---

## Срез (заземлён чтением кода — точные файлы)

### 1. DTO
`crates/shamir-query-types/src/admin/types/index_ops.rs` — рядом с
`CreateIndexOp`/`DropIndexOp` добавь:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameIndexOp {
    pub rename_index: String,   // старое имя индекса
    pub to: String,            // новое имя
    pub table: String,
    pub repo: String,
    // + db/owner поля — СВЕРЬ с RenameTableOp в table_ops.rs (точные имена/типы полей)
}
```
Сверь форму с `RenameTableOp` (`admin/types/table_ops.rs`) — повтори её соглашения
(имена полей repo/db/table, `#[serde(default)]` где есть). Перепиши `mod.rs`
(`admin/types/mod.rs`) на re-export нового типа.

### 2. BatchOp + dispatch + permission
- `crates/shamir-query-types/src/batch/batch_op.rs` — вариант `RenameIndex(RenameIndexOp)`
  (рядом с `RenameTable`).
- `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs` — ветка диспетча →
  `handle_rename_index`.
- `crates/shamir-engine/src/query/auth/session.rs` — permission routing:
  `Action::Write` на целевую таблицу (как RenameTable / drop_index).

### 3. Handler
`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` — `handle_rename_index`
рядом с `handle_rename_table`. Логика: резолв таблицы → index_manager rekey записи
индекса (preserve interned id, как RenameTable rekey'ит каталог) → guard
destination-exists (новое имя уже занято → Err) → guard source-absent (старого
индекса нет → Err, если не if_exists).

Index-механика: `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` —
индексы keyed по имени → interned id. RenameIndex = rekey записи (preserve id).
Учти виды индексов: regular / unique / sorted / index2 (как перечислял E.2
cascade). Если чистый rekey по какому-то виду невозможен — fallback drop+rebuild
под новым именем (индекс — производные данные), но **rekey предпочтительнее**;
если делаешь fallback — оставь однострочный комментарий почему.

### 4. Билдеры
- Rust: `crates/shamir-query-builder/src/ddl/rename_index.rs` (новый, образец
  `ddl/rename_table.rs`) + проводка в `batch.rs` (`crates/shamir-query-builder/
  src/batch/batch.rs`).
- TS: `crates/shamir-client-ts/src/core/builders/ddl.ts` — метод `renameIndex(...)`
  (образец `renameTable`); тип в `crates/shamir-client-ts/src/core/types/ddl.ts`.

---

## Тестовая лестница (ОБЯЗАТЕЛЬНА вся)

1. **Rust integration** — `crates/shamir-db/tests/rename_index_e2e.rs` (новый,
   образец `tests/rename_table_e2e.rs`):
   - таблица + данные + индекс → запрос использует индекс (проверь через EXPLAIN
     из E.7: `plan_type`/`index_used` в ExplainPlan) → rename индекса → запрос
     ВСЁ ЕЩЁ использует индекс под новым именем; данные целы.
   - старое имя индекса больше не резолвится.
   - refuse destination-exists (новое имя занято другим индексом → Err).
2. **TS wire-unit** — `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`:
   `renameIndex` сериализуется в корректную wire-форму (+ счётчик тестов).
3. **TS e2e** — `crates/shamir-client-ts/src/__tests__/e2e-rename-index.test.ts`
   (новый, образец `e2e-fts.test.ts` + `e2e-harness.ts`): createIndex → insert →
   rename → query, assert результаты под новым именем.

> Для e2e нужен СВЕЖЕСОБРАННЫЙ release-сервер (новая wire-op): `cargo build
> --release -p shamir-server` (бинарь в `CARGO_TARGET_DIR=.cargo-target/release`;
> ~15-20 мин). e2e-harness `serverBinPath()` предпочитает release.

---

## Гейт перед сдачей (прогони сам, отчитайся вердиктами)

```
cargo fmt -p <затронутые крейты> -- --check     # НЕ fmt --all
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder -p shamir-engine --full > run.log 2>&1; rc=$?
# TS: vitest по ddl.test.ts + e2e-rename-index
```

Если `clippy` падает на pre-existing лотах в нетронутом коде — НЕ чини в этом
диффе, отметь отдельно.

## Что вернуть (final_text — это данные, не сообщение человеку)

Структурированный отчёт: какие файлы изменены/созданы (список), вердикты гейта
(fmt/clippy/Rust-тесты/TS-тесты pass/fail с числами), какие тесты добавлены и что
именно проверяют, любые компромиссы (fallback drop+rebuild по виду индекса,
schema-bearing нюансы), TODO если что-то не доведено. Не коммить — оркестратор
верифицирует дифф и коммитит.
