בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-3 — drain_to_history метит repo-глобальную версию durable (#437)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #437 —
> CRITICAL находка панельного ревью (`docs/audits/2026-07-06-durability-storage-wal-tx.md`
> §1.4 + `docs/audits/2026-07-06-concurrency-engine.md` A5, найдено НЕЗАВИСИМО
> двумя разными агентами-ревьюерами — высокая достоверность). Область:
> `crates/shamir-tx/src/mvcc_store/drain.rs`.

## Дефект (подтверждён моим личным чтением кода)

`MvccStore::drain_to_history` (`drain.rs:39-93`) — используется при RENAME
таблицы (Phase F.2) для форс-дренажа overlay ОДНОЙ таблицы в history перед
`copy_store`. Метод:
1. `let visibility = self.gate.last_committed();` — читает **repo-глобальную**
   видимость (поле `gate: Arc<RepoTxGate>` — ОДИН shared инстанс на ВСЕ
   таблицы репо, см. `mvcc_store/mod.rs:115`).
2. Дренирует overlay ТОЛЬКО ЭТОЙ таблицы до `visibility`.
3. `self.gate.mark_durable(visibility);` (строка 86) — помечает durable
   ЭТУ repo-глобальную версию `visibility`, хотя реально продренированы
   были ТОЛЬКО версии ИЗ `by_version` (те, что относятся к ЭТОЙ таблице).

### Сценарий провала (из аудита, подтверждён)

Таблица A имеет undrained v=5 (её собственный overlay), таблица B —
undrained v=6 = `visibility` (repo-wide last_committed). Админ делает
`RENAME A` → `drain_to_history` (вызванный для A) сливает v=5 (A's версию)
в history, но помечает durable **v=6** (версию B, которая НИКОГДА не
проходила через ЭТОТ вызов — B's overlay не трогался). Дальше:
- Фоновый `Drainer` (или следующий drain-pass) видит durable уже покрывает
  до v=6 → `gc_overlay_to(6)` (repo-wide, в drainer.rs) стирает overlay-
  копию B(v=6) — **единственную RAM-копию**, раз B ещё не в history.
- Чтения B по ключу: cell=6, overlay miss, history miss → **«записи нет»**
  — тихая потеря видимого значения.
- Следующий drain-pass: `dur(6) >= vis` → B's v=6 никогда не реплеится
  drainer'ом (он думает, что всё уже durable); WAL truncation может
  впоследствии удалить сегмент с v=6 → **перманентная потеря**.

## Задача

### Фикс — минимальный, точечный

В `drain_to_history` (`drain.rs`), заменить:
```rust
// Advance the durable watermark to visibility. `mark_durable` is
// idempotent — if the non-tx path already marked each version durable
// inline, this is a no-op. For tx-path versions that were only in the
// overlay, this is the first time they become durable.
self.gate.mark_durable(visibility);
```
на маркировку ТОЛЬКО реально продренированных этим вызовом версий:
```rust
// CRIT-3 (#437): mark durable ONLY the versions THIS call actually
// drained (the keys of `by_version`), NOT the repo-global `visibility`.
// `visibility` may include a NEWER commit on a DIFFERENT table that this
// call never touched — marking it durable would falsely advance the
// shared repo-wide watermark past that table's still-undrained entry,
// letting a repo-wide overlay GC (drainer.rs) delete its sole RAM copy
// before it's ever written to history. `mark_durable` (via
// CompletionTracker::mark) is explicitly designed for sparse/out-of-order
// marking — it advances the CONTIGUOUS watermark only once a prefix is
// complete, so marking non-contiguous per-table versions here is safe by
// construction (see repo_tx_gate.rs::mark_durable's own doc).
for version in by_version.keys() {
    self.gate.mark_durable(*version);
}
```

**НЕ трогай** `self.gc_overlay_to(visibility)` (следующая строка) — это
вызов на `self` (ТОЛЬКО overlay ЭТОЙ таблицы, не repo-wide), и он остаётся
корректным: `by_version` — это ИСЧЕРПЫВАЮЩИЙ список всех overlay-записей
ЭТОЙ таблицы вплоть до `visibility` (из `self.overlay.iter_all_le(visibility)`),
так что после записи всех них в history безопасно вычистить ИМЕННО overlay
этой таблицы вплоть до `visibility` — никакого чужого overlay это не
затрагивает. Убедись в докладе, что ты понимаешь и подтверждаешь эту
асимметрию (mark_durable меняется, gc_overlay_to — нет) — если видишь
контраргумент, опиши его явно, не молчи.

### Проверь смежное: `entry_tables`/rename caller

Грепни, где вызывается `drain_to_history` (`repo_instance.rs`, RENAME-путь)
— убедись, что вызывающий код НЕ полагается на побочный эффект
"`mark_durable(visibility)` заодно продвигает repo-wide durable для ВСЕХ
таблиц" (маловероятно, но проверь — если такая зависимость есть, фикс
нужно скорректировать, задокументировав почему).

## Тесты

1. **Regression на потерю данных другой таблицы**: два table (A, B) в
   одном репо. Коммить в A (v=N1), коммить в B (v=N2 > N1), НЕ дренировать
   B (симулируй — если drain background автоматически подхватывает,
   найди способ придержать: например прямой вызов `drain_to_history` на
   A БЕЗ предварительного вызова drainer'а на B, воспроизводя RENAME
   сценарий вживую). Вызови `drain_to_history` на A. Assert:
   `gate.durable_watermark()` НЕ включает N2 (B's версия) — то есть
   watermark застрял на N1 или ниже, ПОКА B реально не продренирован.
   Затем задренируй B (через drainer или B's собственный
   drain_to_history) и assert watermark теперь корректно продвигается
   до N2.
2. Существующие RENAME-тесты (грепни `rename_table_stores`/аналог) не
   должны сломаться — `drain_to_history` по-прежнему корректно готовит
   `__history__` таблицы A для `copy_store`.
3. Idempotency: повторный вызов `drain_to_history` на уже-продренированной
   таблице — по-прежнему no-op (overlay пуст → ранний return).

## Гейт

- `./scripts/test.sh @oracle --full` (mvcc_store — область shamir-tx) +
  `./scripts/test.sh @engine --full` (repo_instance RENAME) 1×, целевые
  новые тесты 5-10× повторно;
- `cargo clippy --workspace --all-targets -- -D warnings` (затронуты
  shamir-tx + возможно shamir-engine);
- `cargo fmt -p shamir-tx -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: drain.rs +
regression-тест (найди подходящее место — вероятно
`crates/shamir-tx/src/mvcc_store/tests/` или аналог, грепни существующую
структуру тестов этого модуля). НЕ трогай drainer.rs (уже пофикшен в
#436, не пересекается с этой задачей за исключением того, что оба
используют `mark_durable`). stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

`drain_to_history` помечает durable ТОЛЬКО версии, которые реально
продренированы этим вызовом (per-table), никогда репо-глобальную
`visibility`. Regression-тест доказывает: drain одной таблицы НЕ
продвигает watermark мимо другой недренированной таблицы. Существующие
RENAME-тесты зелёные. Гейт зелёный. Финал: точный diff, вывод тестов,
вывод гейта, подтверждение/опровержение gc_overlay_to-асимметрии.
