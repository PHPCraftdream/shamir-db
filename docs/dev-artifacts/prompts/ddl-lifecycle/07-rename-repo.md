בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief — E.4-followon Phase F.3: RENAME REPO

Делегированная стадия. Переименование репозитория с сохранением его таблиц и их
данных/индексов end-to-end: DTO → dispatch → permission → handler → билдеры
(Rust + TS) → тесты вплоть до e2e TS.

Источник плана: `docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md` §«Phase F.3». Опирается на F.2
(commit-предок): для populated-таблиц внутри репо ПЕРЕИСПОЛЬЗУЙ drain-migration
(`MvccStore::drain_to_history` + copy). Образцы: `handle_rename_table` (F.1/E.4),
`handle_drop_repo`/`handle_create_repo` (cascade-паттерн).

---

## ⛔ Git — ЗАПРЕЩЕНО (verbatim)

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
> or any git command that mutates the working tree or index. Only edit files;
> the orchestrator commits.

## ⛔ НЕ используй внутренний `agent` / sub-agent инструмент

Он падает в этом окружении (context canceled) и обрывает тебя. Читай файлы
напрямую (view/grep/glob), пиши код сам.

---

## Шаг 0 — ИССЛЕДОВАНИЕ перед кодом (определяет объём)

Прочитай `crates/shamir-engine/src/db_instance/db_instance.rs` и
`crates/shamir-engine/src/repo/repo_instance.rs`. Ответь на вопрос:

**Как namespaced стора таблиц репозитория?**
- **Вариант 1 — физический namespace по имени репо**: стора таблиц
  (`__data__`/`__info__`/`__history__`) ключуются с префиксом имени репо. Тогда
  RENAME REPO = для КАЖДОЙ таблицы репо вызвать `rename_table_stores`
  (с F.2-дренажём) под новый namespace + переключить config. Объём L.
- **Вариант 2 — репо логический ключ в реестре**: таблицы держатся в реестре
  репо, а сам репо — запись в каталоге DbInstance. Тогда RENAME REPO = rekey
  записи репо (preserve id) + reverse-index, как `RenameTable` rekey'ит каталог
  таблицы. Таблицы переезжают «бесплатно» (они под логическим ключом репо).
  Объём M.

Определи фактический вариант чтением кода и зафиксируй его в финальном отчёте.
От него зависит реализация. Если данные таблиц рискуют потеряться (как было с
populated-rename до F.2) — переиспользуй drain ПЕРЕД любым копированием.

---

## Срез (точные файлы — заземлено)

1. **DTO**: `crates/shamir-query-types/src/admin/types/repo_ops.rs` —
   `RenameRepoOp { rename_repo: String, to: String, db?: ... }`. Сверь форму с
   `RenameTableOp` (`table_ops.rs`) и существующими repo-ops. `#[serde(default)]`
   на новых полях. Перепиши `admin/types/mod.rs` + `admin/mod.rs` на re-export.
2. **BatchOp + dispatch + permission**:
   - `crates/shamir-query-types/src/batch/batch_op.rs` — вариант `RenameRepo(RenameRepoOp)`.
   - `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs` — ветка → `handle_rename_repo`.
   - `crates/shamir-engine/src/query/auth/session.rs` — permission: `Action::Write`
     на репо/db (как drop_repo).
3. **Handler**: `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs` —
   `handle_rename_repo` рядом с `handle_drop_repo`/`handle_create_repo`. Логика по
   результату Шага 0: rekey реестра репо (preserve id) + reverse-index, ИЛИ
   N×rename_table_stores с drain. Guard'ы: destination-exists (имя репо занято →
   Err), source-absent.
4. **Билдеры**: Rust `crates/shamir-query-builder/src/ddl/rename_repo.rs` (образец
   `ddl/rename_table.rs`) + проводка в `batch.rs`; TS `ddl.ts` метод `renameRepo`
   + тип в `types/ddl.ts`.

---

## Дисциплина проекта (обязательно)

- **Тесты — ТОЛЬКО `./scripts/test.sh`** (raw `cargo test` заблокирован). Вывод в
  ФАЙЛ, не inline-grep. Integration-тесты требуют `--full`.
- `serde_json::Value` ЗАПРЕЩЁН (кроме napi/FFI, serde round-trip, WASM-bridge,
  protocol-spec). Queries — только через builder.
- `scc::*::len()` ЗАПРЕЩЁН — AtomicUsize-mirror/annotate. `use` в шапке. Один файл
  = один export. `mod.rs` — только re-exports. Тесты — в `tests/`-директории.
- Concurrency: lock-free/scc/atomics; `std::sync::Mutex`/`RwLock` на hot-path —
  запрещены.

---

## Тестовая лестница (вся обязательна)

1. **Rust integration** (`crates/shamir-db/tests/rename_repo_e2e.rs`, новый):
   репо с таблицей + данными + индексом → rename repo → старое имя репо НЕ
   резолвится; новое резолвится; таблицы и ВСЕ их данные целы (read-back);
   индексы работают под новым репо; refuse destination-exists.
2. **TS wire-unit** (`crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`):
   `renameRepo` сериализуется (+ счётчик).
3. **TS e2e** (`crates/shamir-client-ts/src/__tests__/e2e-rename-repo.test.ts`,
   новый; образец `e2e-rename-table.test.ts` + `e2e-harness.ts`): createRepo с
   таблицами → insert → rename repo → query через новый репо возвращает все
   данные; старое имя репо не резолвится.

> **e2e — БЫСТРО через debug**: харнесс теперь чтит env `SHAMIR_SERVER_BIN`
> (приоритетный путь к бинарю). Собери DEBUG-сервер (быстро):
> `cargo build -p shamir-server` (бинарь в `D:\dev\rust\.cargo-target\debug\
> shamir-server.exe`), затем гоняй vitest с
> `SHAMIR_SERVER_BIN=D:\dev\rust\.cargo-target\debug\shamir-server.exe`.
> НЕ собирай release (долго).

---

## Гейт перед сдачей (прогони сам, отчитайся числами)

```
cargo fmt -p <затронутые крейты> -- --check       # НЕ fmt --all
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder -p shamir-engine --full > run.log 2>&1; rc=$?
# TS: debug-сборка сервера + SHAMIR_SERVER_BIN + vitest ddl.test.ts + e2e-rename-repo
```

## Что вернуть (final_text — данные оркестратору)

Результат Шага 0 (вариант namespacing + почему); изменённые/созданные файлы;
вердикты гейта (fmt/clippy/Rust-integration/TS-unit/TS-e2e — pass/fail с числами);
что проверяют тесты; как обеспечена сохранность данных populated-таблиц (drain?);
компромиссы/TODO. НЕ коммить — оркестратор верифицирует и коммитит.
