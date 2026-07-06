בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-1 — recovery проглатывает ошибку history-записи → truncation стирает ack-коммит (#435)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #435 —
> CRITICAL находка из панельного ревью (`docs/audits/2026-07-06-durability-storage-wal-tx.md`
> §1.1). Область: `crates/shamir-engine/src/tx/recovery.rs`.

## Дефект

`seed_version_cache_for_entry` (recovery.rs:373-421) вызывает
`mvcc.write_committed_to_history(&ops, v).await` и на ошибке делает только
`log::warn!` — не пропагирует. Функция возвращает `()`, не `Result`.

`replay_v2_entry` (recovery.rs:321-354) вызывает
`seed_version_cache_for_entry(entry, repo).await;` без проверки результата
(функция ничего не возвращает) и безусловно возвращает `Ok(())`.

`recover_inflight_v2` (recovery.rs:243-305) вызывает
`replay_v2_entry(&entry, repo).await?;` — раз `replay_v2_entry` ВСЕГДА `Ok`
(независимо от того, упала ли history-запись), код БЕЗУСЛОВНО продолжает:
`gate.completion().mark(entry.commit_version, Materialized)` и
`gate.mark_durable(entry.commit_version)`.

## Сценарий провала (из аудита)

crash → рестарт → recovery реплеит entry v=100, `write_committed_to_history`
падает (ENOSPC / transient I/O) → warn → `mark_durable(100)`. Дальше:
(а) читатели видят `last_committed ≥ 100`, но значения нет ни в overlay
(пустой после рестарта), ни в history → отдаётся старая версия — тихая
потеря подтверждённого коммита; (б) следующий drain-pass видит
`has_truncatable(100)`, `flush_all_history`, `truncate_below` удаляет
sealed-сегмент с единственной копией v=100 → **невосстановимая потеря**.

Комментарий на recovery.rs:409-412 («leaving a partial history write
inflight is safe — the WAL marker is untouched») — ложь пост-F6: маркеров
нет, truncation идёт по watermark, не по маркеру.

## Задача

1. **`seed_version_cache_for_entry`**: изменить сигнатуру на
   `async fn seed_version_cache_for_entry(entry: &WalEntryV2, repo: &RepoInstance) -> DbResult<()>`.
   Убрать `log::warn!`-проглатывание — пропагировать ошибку. Функция
   итерирует по `by_table` (несколько таблиц на entry) — если ХОТЯ БЫ ОДНА
   таблица падает, собери ПЕРВУЮ ошибку и верни её ПОСЛЕ попытки всех
   таблиц (паттерн уже используется в `flush_buffers` — грепни и повтори),
   не обрывай цикл на первой же ошибке (остальные таблицы всё равно должны
   получить попытку записи — best-effort по остальным, fail по факту любой
   неудачи).
2. **`replay_v2_entry`**: заменить
   `seed_version_cache_for_entry(entry, repo).await;` на
   `seed_version_cache_for_entry(entry, repo).await?;` — теперь ошибка
   пропагируется наружу через существующий `?` в `replay_v2_entry`'s
   caller (`recover_inflight_v2`).
3. **Убедись, что порядок операций в `recover_inflight_v2` уже безопасен**
   (не нужно менять код там, только подтвердить в докладе): цикл
   `replay_v2_entry(&entry, repo).await?;` идёт ДО
   `gate.completion().mark(...)` и `gate.mark_durable(...)` — раз
   `replay_v2_entry` теперь честно пропагирует ошибку через `?`, весь цикл
   `for entry in entries` прерывается на первой неудачной entry, НЕ
   помечая её (и последующие) durable/materialized. `recover_inflight_v2`
   возвращает `DbResult<usize>` — ошибка всплывает к caller'у open()
   (грепни, что уже задекларировано в `db_management.rs:337-343` — «repo
   that cannot recover must not be served» — подтверди, что твой фикс
   реально приводит именно к этому эффекту, не только компилируется).
4. **Обнови устаревший комментарий** на recovery.rs:409-412 — убрать
   ложное «the WAL marker is untouched», описать РЕАЛЬНЫЙ новый контракт:
   ошибка history-записи теперь фатальна для recovery/open, entry не
   помечается durable до успешной записи.

## Тесты

Существующая инфраструктура crash-recovery — `crates/shamir-engine/tests/crash_recovery.rs`
(child-process crash-injection паттерн, см. существующие
`crash_at_phase5c_recovers_full_tx` и т.п. как образец). Добавь:

1. **Fault-injection тест**: entry, чья `write_committed_to_history`
   гарантированно падает (нужен test-only hook — грепни существующие
   `FAIL_VECTOR_DELTA_TX_ID`-подобные статики в `commit_phases.rs`/
   `materialize.rs` как образец паттерна для инъекции сбоя по конкретному
   `txn_id`/`table_id` в тестах) → recovery/open ДОЛЖНО вернуть Err
   (repo не открывается), НЕ должно тихо продолжить с `mark_durable`.
2. **Regression на существующее поведение**: обычный (без инъекции сбоя)
   recovery-сценарий продолжает работать (существующие
   `crash_at_phase*_recovers_full_tx` тесты не должны сломаться).
3. Если возможно — unit-тест на `seed_version_cache_for_entry` напрямую
   (мокнутый/тестовый `MvccStore`, у которого `write_committed_to_history`
   можно заставить упасть) — проверить, что ошибка пропагируется, а НЕ
   проглатывается логом.

## Гейт

- `./scripts/test.sh @engine --full` (в т.ч. crash_recovery.rs) 1×;
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`;
- `cargo fmt -p shamir-engine -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: recovery.rs + новый
fault-injection тест. НЕ трогай drainer.rs (там отдельная, уже
существующая, гейтинг-логика Phase B/C — за пределами этой задачи, хотя
похожа). stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

`seed_version_cache_for_entry` возвращает `Result`, ошибка пропагируется
через `replay_v2_entry` до `recover_inflight_v2` → до open(). Entry с
неудавшейся history-записью НЕ помечается durable/materialized. Устаревший
комментарий исправлен. Fault-injection тест доказывает: recovery падает
(не молча продолжает) при сбое history-записи. Существующие
crash-recovery тесты зелёные. Гейт зелёный. Финал: точные diff-места,
вывод тестов (включая новый fault-injection), вывод гейта.
