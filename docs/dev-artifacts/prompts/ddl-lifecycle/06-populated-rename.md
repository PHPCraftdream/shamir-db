בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief — E.4-followon Phase F.2: populated-table RENAME via overlay drain-migration

Делегированная стадия (ЯДРО кампании, durability-критично). Снять архитектурный
барьер: сделать так, чтобы `RENAME TABLE` работал для таблиц **С ДАННЫМИ**.

Источник плана: `docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md` §«Phase F.2». Образец RENAME
для пустых таблиц — Phase E.4 (commit `a7dcda5`). F.1 RENAME INDEX (commit-предок)
— рядом стоящий образец DTO/handler/тестов.

---

## ⛔ Git — ЗАПРЕЩЕНО (verbatim)

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
> or any git command that mutates the working tree or index. Only edit files;
> the orchestrator commits.

---

## Барьер (заземлён чтением кода — ОБЯЗАТЕЛЬНО перечитай эти места)

- **Guard, который снимаем**: `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:~240`
  — `rename_table_as` отказывает при `mvcc.cell_count() > 0`.
- **Где живут данные**: `crates/shamir-tx/src/mvcc_store/mod.rs`:
  - `cells` (key→версия, in-memory, rebuildable; ~:117),
  - `overlay` (`VersionedOverlay`, не-дренированный хвост
    `(durable_watermark, visibility_watermark]`; ~:139),
  - `history` (durable `Store`, но вакуумится при `Retention::current_only` —
    остаётся только текущая версия каждого ключа).
- **Почему store-copy переносит пустоту**: `flush_history()` (~:353) =
  `self.history.flush()` (сброс буферов Store), **НЕ** дренаж overlay→history.
  Материализацию overlay→history делает `write_committed_to_history`
  (`crates/shamir-tx/src/mvcc_store/mvcc_history.rs:~457`) / фоновый `Drainer`.
  `rename_table_stores` (`crates/shamir-engine/src/repo/repo_instance.rs:~380`)
  зовёт `flush_history` → `__history__<from>` пуст → `copy_store` копирует пустоту.

---

## Шаг 0 — ИССЛЕДОВАНИЕ перед кодом (обязательно; выбор стратегии)

Прочитай: `mvcc_store/mod.rs`, `mvcc_store/mvcc_history.rs`
(`write_committed_to_history`), всё про watermarks (`durable_watermark`,
`visibility_watermark`, `gc_overlay_to`/аналог), `Drainer`
(`crates/shamir-tx/src/.../drainer*`), cold-start путь восстановления `cells` из
`history` (range-scan в `mod.rs`).

**Стратегия A (РЕКОМЕНДОВАНА) — force-drain → copy → cold-start**:
- собрать надёжный синхронный примитив `MvccStore::drain_to_history()`: прогнать
  все overlay-версии текущего окна через `write_committed_to_history`, продвинуть
  `durable_watermark` до `visibility_watermark`, освободить overlay (gc).
- в `rename_table_stores` заменить `flush_history()` на этот синхронный дренаж
  **перед** `copy_store` для `__history__<from>`.
- новый `MvccStore` для `<to>` cold-start'ит `cells` range-scan'ом из
  скопированного `history`. Под `current_only` в history остаётся текущая версия
  каждого живого ключа → данные целы.
- **Плюс**: переиспользует cold-start, не вторгается во внутренности overlay.

**Стратегия B** (только если A нереализуема): прямой перенос `cells`+`overlay` в
новый `MvccStore` над скопированным history (сложнее: watermark-инварианты).
Если выбрал B — обоснуй однострочно почему A не годится.

> Если ни A ни B не доводимы до durable-зелёного — НЕ снимай guard вслепую:
> оставь guard, задокументируй найденное стопор-условие в финальном отчёте, верни
> «blocked» с точной причиной. Молчаливая потеря данных при rename НЕДОПУСТИМА.

---

## Шаг 1 — реализация (Стратегия A)

1. `MvccStore::drain_to_history()` (новый pub метод в `crates/shamir-tx/src/mvcc_store/`)
   — синхронный дренаж как выше; идемпотентен (повторный вызов на пустом overlay
   — no-op).
2. `rename_table_stores` (`repo_instance.rs:~380`): `drain_to_history` ПЕРЕД
   `copy_store` для `__data__`/`__info__`/`__history__`.
3. `rename_table_as` (`table_management.rs:~240`): убрать guard `cell_count>0`.
   **Оставить** guard'ы schema-bearing и destination-exists (они про другое).
4. **schema-bearing** (под-вопрос): если простой rename таблиц со схемой требует
   миграции auto-bound schema-validator — НЕ растягивай F.2; оставь schema-bearing
   guard как есть с явным комментарием-follow-on. F.2 = данные, не схема.

---

## Дисциплина проекта (обязательно)

- **Тесты — ТОЛЬКО `./scripts/test.sh`** (raw `cargo test` заблокирован).
  Вывод в ФАЙЛ, не inline-grep: `./scripts/test.sh ... > run.log 2>&1; rc=$?`
  затем `grep -aE "Summary|FAIL|TIMEOUT|SLOW|panic" run.log`.
- `serde_json::Value` ЗАПРЕЩЁН (кроме napi/FFI, serde round-trip, WASM-bridge,
  protocol-spec). Queries — только через builder.
- **`scc::*::len()` ЗАПРЕЩЁН** (O(N)) — AtomicUsize-mirror или
  `#[allow(clippy::disallowed_methods)] // O(N) ack: <why>`.
- Concurrency: lock-free/scc/atomics; `std::sync::Mutex`/`RwLock` на hot-path
  запрещены. `use` — в шапке. `#[serde(default)]` на новых полях. Один файл =
  один export. `mod.rs` — только re-exports. Тесты — в `tests/`-директории.

---

## Тестовая лестница (durability-зона — ГЛУБЖЕ обычного; ВСЯ обязательна)

1. **Rust unit** (`shamir-tx`, в `tests/`-директории mvcc_store): `drain_to_history`
   — после дренажа overlay пуст; history содержит текущую версию каждого ключа;
   повторный дренаж идемпотентен (no-op, без паники).
2. **Rust integration** (`crates/shamir-db/tests/rename_table_e2e.rs`): переписать
   `rename_table_refuses_populated` → `rename_table_migrates_populated`: таблица +
   N строк (insert/update) → rename → старое имя НЕ резолвится; **новое резолвится
   со ВСЕМИ данными** (read-back каждой строки); индексы на новой таблице работают;
   дозапись новой строки в переименованную таблицу + read-back (overlay новой
   таблицы жив).
3. **Rust durability** (критично — ДО доверия снятию guard): после rename данные
   переживают «перечитывание» (drop in-memory TableManager/MvccStore → cold-start
   из history → все строки на месте). Сверь с `crates/shamir-engine/tests/crash_recovery.rs`.
4. **TS e2e** (`crates/shamir-client-ts/src/__tests__/e2e-rename-table.test.ts`,
   новый): createTable → insert несколько строк → rename → query новой таблицы
   возвращает ВСЕ строки; старое имя не резолвится; insert/update в новую таблицу
   работает. Образец — `e2e-rename-index.test.ts` (F.1) + `e2e-harness.ts`.

> e2e требует СВЕЖЕСОБРАННЫЙ release-сервер: `cargo build --release -p
> shamir-server` (бинарь `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target\release`;
> ~28 мин — заложи). Без новой wire-op изменений сервера здесь может не быть (op
> RENAME TABLE уже существовала в E.4) — но поведение сервера меняется, так что
> пересборка нужна для e2e.

---

## Гейт перед сдачей (прогони сам, отчитайся вердиктами с числами)

```
cargo fmt -p <затронутые крейты> -- --check          # НЕ fmt --all
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-tx -p shamir-engine -p shamir-db --full > run.log 2>&1; rc=$?
# TS: vitest e2e-rename-table против release-сервера
```

## Что вернуть (final_text — данные оркестратору, не сообщение человеку)

Структурированный отчёт: выбранная стратегия (A/B) и почему; новый примитив
`drain_to_history` (где, как обеспечена идемпотентность/durability); снятый guard;
список изменённых/созданных файлов; вердикты гейта (fmt/clippy/Rust-unit/
integration/durability/TS-e2e — pass/fail с числами); что именно проверяет
durability-тест (cold-start re-read); schema-bearing решение (мигрировал/оставил
guard); компромиссы/TODO. Если durability недостижима — верни «blocked» с точной
причиной, guard НЕ снимай. НЕ коммить — оркестратор верифицирует и коммитит.
