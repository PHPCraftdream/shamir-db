בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Ревью конкурентности: shamir-engine (commit pipeline / drainer / oracle), shamir-tx (примитивы оракула), shamir-collections, shamir-index

_Агент: @fxx (max effort), 2026-07-06. Часть панели из 5 агентов ревью проекта после завершения векторной кампании._

**Объём прочитанного:** tx/{commit,commit_phases,materialize,pre_commit,drainer,finalize,group_commit,recovery,apply_replicated}.rs, repo_instance.rs, repo/group_commit; shamir-tx: repo_tx_gate, completion_tracker, version_guard, mvcc_store/{mod,mvcc_history,mvcc_locks,key_lock,mvcc_gc,drain}, versioned_overlay, cell_reservation_guard, staging_store, layered_interner, changefeed, predicate_set, tx_context, metrics; table/{table_manager*,write_exec,record_counter,interner_manager,streaming}; shamir-index: registry, actor, write_ops, backend, bm25, vector/{vector_backend,brute_force}, legacy/index_manager; shamir-collections (только алиасы — находок нет). hnsw_adapter.rs и известные Б-1/Б-3/Б-4/Б-6 (VR-1/2/8, отдельная кампания VR-фиксов по ревью векторной кампании) не переоткрывались.

---

## Топ-5 «ОБЯЗАНЫ УЛУЧШИТЬ»

| # | Серьёзность | Где | Суть | Эффект |
|---|---|---|---|---|
| A1 | **CRITICAL** (для Serializable) | `tx/commit.rs:598` (`commit_tx_lockfree`), `tx/pre_commit.rs:308` | P2c убрал `commit_mutex`, но валидация (Phase 2/2-bis) и публикация (5a/Phase 6) больше ничем не сериализованы между коммиттерами без unique-guards → два SSI-txа валидируются до чьей-либо публикации | Write-skew коммитится под Serializable |
| A2 | **HIGH** | `mvcc_store/mod.rs:404` (`publish_cell`), `mvcc_history.rs:333,518`, `mvcc_history.rs:244` (`seed_version`) | `publish_cell`/`seed_version` пишут версию ячейки **без max** → drainer/recovery/cold-read откатывают cell.version назад | Stale-read уже закоммиченного значения + маскировка SSI-конфликта |
| A3 | **HIGH** | `table/table_manager_streaming.rs:345-349,421-425,265-266` | В read-set записывается `version_of()` (текущая версия ячейки), а не версия, видимая на снапшоте → первый read, гонящийся с чужим 5a, записывает «новую» версию при чтении «старого» значения | `validate_read_set` пропускает конфликт (current == version_seen) — SSI дыра |
| A4 | **HIGH** | `tx/commit.rs:663,533` + `table_manager_streaming.rs:379` | Level-3 (Pessimistic): (а) локи снимаются ДО публикации 5a; (б) чтение под X-локом идёт по `tx.snapshot_version` (stale) | Классический lost update под «пессимистичной» изоляцией |
| A5 | **HIGH** (узкий триггер, но data loss) | `mvcc_store/drain.rs:86` (`drain_to_history`), `tx/drainer.rs:277-301,493-541` | Per-table `drain_to_history` (RENAME) делает `mark_durable(visibility)` **репо-широкой** версией: помечает durable чужую, ещё не слитую версию другой таблицы | Watermark перепрыгивает недренированную версию → `gc_overlay_to` стирает её единственную RAM-копию, F6b truncate удаляет её WAL-сегмент → потеря закоммиченных данных |

**Примечание: A5 независимо подтверждён durability-агентом (см. `2026-07-06-durability-storage-wal-tx.md`, находка 1.4) — двойное совпадение усиливает уверенность.**

---

## Секция 1. Обязаны улучшить (гонки / ordering / lost update / deadlock)

### A1. CRITICAL — потеря атомарности validate→publish в lock-free коммите (write-skew под Serializable)
**Файлы:** `crates/shamir-engine/src/tx/commit.rs:598-700` (HEAD), `crates/shamir-engine/src/tx/pre_commit.rs:308-397`, `crates/shamir-tx/src/repo_tx_gate.rs:498-522`.

Легаси-путь держал validate+publish под `commit_mutex` — first-committer-wins гарантировался. P2c-комментарий (`commit.rs:471-486`) утверждает: «same-table committers serialize at uwl acquisition» — но `uwl_guards` берутся **только** для таблиц с `unique_guards` (`pre_commit.rs:226-235`). Serializable-tx без unique-записей не сериализуется нигде.

**Интерливинг (write-skew, обе — Serializable, snapshot=10, x=y=v10):**
- A: читает x (`read_set[x]=10`), пишет y. B: читает y, пишет x (write-sets дизъюнктны → `claim_write_set` не конфликтует, uwl нет).
- A: `pre_commit_locked_validate`: `version_of(x)=10` → OK; предикатов нет → OK; claim y → win.
- B: то же для y/x → OK (A ещё не сделал 5a).
- A: WAL, footprint, 5a (`finalize_reservation(y,11)`), publish 11.
- B: WAL, footprint, 5a (x→12), publish 12.
- **Результат:** обе закоммичены; A читала y@10, B перезаписал прочитанное A. Цикл rw-антизависимостей → нарушение Serializable. Тест `ssi_write_skew_one_aborts` (`acceptance_tests.rs:472`) строго последовательный и эту дыру не ловит; `multikey_opposing_order...` тестирует только write-write (закрыто cell-claims).

То же и для phantom-канала: `predicate_conflicts_batch` ограничен `last_committed` — footprint конкурента, вставленный, но ещё не опубликованный, невидим; при взаимных предикатах оба проходят.

Отягчающее: в `commit_tx_inner_legacy_async` (`commit.rs:553-554`) порядок вообще инвертирован — `version_guard.commit()` (publish) **до** `record_commit_writes` (это признано в `finalize.rs:18-22`, но не как дыра): lock-free валидатор, читающий `last_committed` между этими двумя строками, видит окно с версией без footprint — фантом теряется даже при последовательных валидациях.

**Фикс-эскиз (варианты):** (1) вернуть глобальную сериализацию validate→(footprint+cell-bump) только для Serializable (Snapshot остаётся lock-free) — самый дешёвый честный вариант; (2) настоящий OCC backward-validation: вставлять footprint **до** validate и валидировать окно `(snapshot, my_version)` включая in-flight записи (не гейтить `last_committed`), с фильтрацией abort'нутых; (3) SIREAD-locks на ячейках (`RecordCell` уже есть — добавить reader-marks). В любом случае в async-пути перенести `record_commit_writes` до `version_guard.commit()`.

### A2. HIGH — регрессия версии ячейки drainer'ом/recovery (`publish_cell` без монотонности)
**Файлы:** `crates/shamir-tx/src/mvcc_store/mod.rs:404-416`, `mvcc_history.rs:326-335` (batch), `mvcc_history.rs:508-519`, `mvcc_history.rs:244-254` (`seed_version` — `upsert_async`), потребитель: `tx/drainer.rs:408-426`.

`publish_cell` присваивает `cell.version = version` безусловно. Drainer вызывает его для **старой** версии после того, как новый коммит уже опубликовал более новую.

**Интерливинг:**
- A коммитит k@10 (cell=10, overlay(k,10)); WAL 10 offered; dur=9.
- Drainer: vis=10, Phase B: `history.transact` для v10 — **await, точка приостановки**.
- B коммитит k@11: 5a → `finalize_reservation(k,11)` → cell=11, floor=11.
- Drainer возобновляется: `publish_cell(k,10)` → **cell 11→10**.
- Читатель: `get_current(k)`: floor=11, cur_v=10 ≤ floor → direct-path → overlay(k,10) → **значение A**. Committed-write B невидим до следующего прохода drainer'а (обычно мс, но при застрявшем drain — неограниченно).
- Хуже: SSI-валидатор с `version_seen=10` в этом окне видит `current_version=10` → **конфликт замаскирован** (усиливает A1/A3).

Та же регрессия у `seed_version` (cold-read `get_current_bytes` гонится с первым overlay-only коммитом ключа: сеет старую history-версию поверх свежего cell=12) — при этом комментарий «upsert … advances monotonically» фактически неверен (upsert перезаписывает).

**Фикс-эскиз:** сделать `publish_cell`/`seed_version` max-монотонными (`if version > cell.version { cell.version = version }` внутри entry); варианту drainer'а это ничего не ломает (он всегда пишет ≤ актуального).

### A3. HIGH — SSI read-set записывает «текущую» версию вместо прочитанной
**Файлы:** `crates/shamir-engine/src/table/table_manager_streaming.rs:345-349` (`read_one_tx`), `:421-425` (`read_one_tx_bytes`), `:265-266` (`record_scan_reads`); проверка: `crates/shamir-tx/src/tx_context.rs:530-541`.

`version = mvcc.version_of(key)` — это версия ячейки **сейчас** (может быть > snapshot), а значение читается через `get_at(key, snapshot)` (старое).

**Интерливинг:**
- B (Serializable, snap=10) собирается читать k.
- A публикует k@11 (5a).
- B: `version_of(k)=11` → `record_read(k, **11**)`; `get_at(k,10)` → **старое** значение v10.
- B коммитит: `validate_read_set`: current=11, version_seen=11 → `current > version_seen` ложно → **PASS**. B закоммитился, прочитав устаревшее k, после A — first-committer-wins сломан даже при полностью сериализованных валидациях.

Показательно: тесты (`acceptance_tests.rs:495` и др.) записывают read вручную как `tx.snapshot_version` — т.е. корректную семантику, которую production-путь не реализует. Doc-комментарий `record_read_shared` («first-read-wins…») защищает только от повторных чтений, не от гонки на первом.

**Фикс-эскиз:** записывать версию, соответствующую прочитанному значению: либо `version_of(key).min(tx.snapshot_version)` (консервативно: ячейка уже уехала за снапшот → на коммите гарантированный abort), либо вернуть из `get_at` пару `(resolved_version, bytes)` и записывать её.

### A4. HIGH — Level-3 Pessimistic: lost update (ранний release + snapshot-stale чтение под локом)
**Файлы:** `crates/shamir-engine/src/tx/commit.rs:663` (lockfree: `release_pessimistic_locks` ДО `materialize`), `:533` (async-путь: до `apply_data_phase`), `crates/shamir-engine/src/table/table_manager_streaming.rs:379` (чтение под локом по `tx.snapshot_version`).

**Интерливинг:**
- T1 (Pessimistic): X-lock k, читает k=v0, стейджит k=v1; commit: WAL ok → **локи сняты** → (5a ещё не выполнен).
- T2 (Pessimistic, начал раньше, snapshot < версии T1): X-lock k (получил после release T1), `read_one_tx(k)` → `get_at(k, snapshot_T2)` → **v0** (даже если 5a T1 уже прошёл — снапшот старый!), стейджит k=v2(по v0), коммитит.
- Апдейт T1 потерян. Wound-wait здесь не помогает: конфликт разрешён «успешно», оба закоммитились.

Две независимые поломки: (а) 2PL нарушен — экслюзив снят до видимости записи; (б) даже при правильном порядке release чтение под локом обязано быть «latest committed», а не снапшотным, иначе лок бессмыслен.

**Фикс-эскиз:** для Pessimistic: в `read_one_tx*` при `isolation == Pessimistic` читать `mvcc.get_current_bytes` (после захвата лока); `release_pessimistic_locks` перенести после `materialize`/`apply_data_phase` (после публикации) на всех путях.

### A5. HIGH — `drain_to_history` помечает durable репо-широкую версию → потеря данных другой таблицы
**Файлы:** `crates/shamir-tx/src/mvcc_store/drain.rs:42-92` (`mark_durable(visibility)` в :86), `crates/shamir-engine/src/tx/drainer.rs:243-301` (gap-reseed только при пустом префиксе), `:511-514` (repo-wide `gc_overlay_to`), `:531-541` (F6b truncate); вызов: `repo_instance.rs:401-424` (RENAME).

`visibility = gate.last_committed()` — глобальная; `mark_durable(visibility)` вставляет `Materialized` для версии, которая может принадлежать **другой** таблице и быть ещё только в её overlay.

**Интерливинг:**
- v5 = запись в T1 (overlay T1), v6 = запись в T2 (overlay T2), dur=4. Offer v6 дропнут backpressure'ом (или любой gap окна).
- Админ: RENAME T1 → `drain_to_history` (T1): сливает v5 в history, `mark_durable(6)` → states={6:M} (данные v6 НЕ durable!).
- Drainer: окно [5] (6 отсутствует) → Phase B/C: сливает v5, `mark_durable(5)` → watermark 4→5→**6** (за счёт фиктивной метки).
- В том же проходе: `gc_overlay_to(6)` по всем таблицам → overlay-копия v6 (единственная в RAM) **удалена**; чтения T2-ключа: cell=6, overlay miss, history miss → «записи нет».
- Следующий проход: dur=6 ≥ vis → v6 никогда не реплеится; gap-reseed не срабатывает (он только при пустом префиксе). При достижении сегментной границы `truncate_below(6)` удаляет WAL-запись v6 → **перманентная потеря**.

**Фикс-эскиз:** в `drain_to_history` помечать durable только реально слитые версии (`for v in by_version.keys() { gate.mark_durable(*v) }`), не `visibility`; заодно `gc_overlay_to(visibility)` → `gc_overlay_to(gate.durable_watermark())`.

### A6. HIGH — lock-upgrade Shared→Exclusive игнорирует чужих shared-держателей
**Файл:** `crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:77-98` (re_entrant → `compatible=true` → `state.mode = Some(mode)`).

**Интерливинг:** T1 и T2 (обе Pessimistic) держат Shared на k (после `read_one_tx`). T1: `update_tx` → `acquire_pessimistic_write_lock` → `lock_key(Exclusive)`: `held_by(T1)=true` → мгновенный grant, `mode=Exclusive`, **holders=[T1,T2]** (инвариант из key_lock.rs:42-44 нарушен). T2 симметрично «апгрейдится» тоже. Обе пишут «под эксклюзивом» → lost update / грязное чтение на уровне протокола локов.

**Фикс-эскиз:** в re-entrant ветке при запросе Exclusive и наличии ДРУГИХ держателей — не грантовать, а падать в wound-wait разбор (wound младших shared-держателей, ждать старших). Осторожно с симметричным апгрейдом двух старых — wound-wait по tx_id разрулит (младший будет wounded).

### A7. MEDIUM-HIGH — `GroupCommit::run`: отмена/паника лидера навсегда клинит synced-flush
**Файл:** `crates/shamir-engine/src/repo/group_commit/mod.rs:30-76`.

`leader_busy=true` ставится под локом, сбрасывается только при нормальном выходе из цикла. Если future лидера дропнут (например `tokio::time::timeout` вокруг `synced_flush`) на любом `.await` внутри `flush()` — `leader_busy` остаётся `true` навсегда.

**Интерливинг:** C1: `run()` → лидер → `flush().await` (внутри `flush_buffers` → диск) → caller отменяет по таймауту → future dropped. C2..Cn: регистрируются, видят `leader_busy=true`, ждут `rx`, который никто никогда не пошлёт → **все synced-коммиты этого repo зависают навсегда** (это ровно класс «tokio guard через await», только через флаг).

**Подтверждено независимо durability-агентом (находка 2.1) — двойное совпадение.**

**Фикс-эскиз:** RAII-guard (по образцу `SnapshotFlightGuard` из vector_backend.rs:846-857): на Drop — `leader_busy=false` + отослать всем ожидающим Err(«leader cancelled, retry»); либо выделенный spawn-нутый flush-таск, которому отмена вызывающего не страшна.

### A8. MEDIUM — interner-delta теряется, если «первый toucher» абортится (undecodable records после recovery)
**Файлы:** `crates/shamir-tx/src/layered_interner.rs:176-202` (`is_new` решает, кто несёт delta), `crates/shamir-engine/src/table/write_exec.rs:134-150` (C5 base-intern путь), `tx/pre_commit.rs:173-201`.

**Интерливинг:** tx1 и tx2 конкурентно интернируют новое имя "foo": tx1 получает `New(42)` (delta у tx1), tx2 — `Exists(42)` (delta пустая). tx1 **абортится до WAL** (SSI/WAL-begin). tx2 коммитится: его байты ссылаются на id 42, но WAL-entry без delta; A5-гейт для него «тривиально safe» → truncate возможен. Checkpoint интернера гейтится `interner_delta_max_id.is_some()` (`materialize.rs:318`) — у tx2 None → чекпойнт пропущен. Crash до чьего-либо чекпойнта → на восстановлении интернер не знает id 42 → записи tx2 **не декодируются** при чтении.

**Фикс-эскиз:** каждый коммиттер обязан включать в свою delta все (name,id), на которые ссылаются его staged-байты и которые выше persisted hwm (реплей `touch_with_id` идемпотентен, дубли безвредны).

### A9. MEDIUM-HIGH — online CREATE/RENAME INDEX: запись между backfill-снапшотом и регистрацией дефиниции теряется
**Файлы:** `crates/shamir-index/src/legacy/index_manager.rs:241-279` (`create_index_from_records`: постинги → **потом** `add_index`), аналогично `index_manager_unique.rs:368`; `table_manager_index_mgmt.rs` (HEAD) `:414-450` (`rename_index`: drop→rebuild окно), `:498-538` (`rekey_sorted_prefix` scan/copy без блокировки записей).

**Интерливинг:** админ собирает снапшот (`collect_all_current_records`); писатель вставляет R (дефиниция ещё не зарегистрирована → хук записи постинг не создаёт); админ дописывает постинги снапшота и регистрирует индекс. R **навсегда** отсутствует в индексе → неверные результаты запросов до `repair()`. Для unique-rename хуже: в окне drop→create дубликаты проходят, затем `create_unique_index_from_records` падает на дубликатах → таблица остаётся вообще без unique-индекса.

**Фикс-эскиз:** порядок «register-def-first, then backfill» (конкурентные записи индексируются хуками, backfill идемпотентно доливает старое; для unique — валидация на backfill'е под `unique_write_lock`), либо держать `unique_write_lock`/таблично-широкий барьер на время DDL.

### A10. MEDIUM — vacuum fast-path vs открытие снапшота (TOCTOU на `active_snapshots_empty`)
**Файлы:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:66-85`, `repo_tx_gate.rs:223-231` (`open_snapshot`: сначала читает floor, потом регистрирует).

**Интерливинг:** Reader: `version = last_committed()` (=V_old) — вытеснен до `insert_async`. Writer: публикует new_v, `vacuum_key`: `active_snapshots_empty()` → true → **удаляет old_v из history (+ overlay)**. Reader: регистрирует снапшот V_old, читает ключ: cell=new_v>V_old → fallback «newest ≤ V_old» → пусто → **запись исчезла для валидного снапшота** (retention по умолчанию CurrentOnly → fast-path — обычный случай). Тот же класс TOCTOU у scan-path через `min_alive()` и у `prune_commit_log_below`.

**Фикс-эскиз:** в `open_snapshot`: register(v0) → перечитать floor → если сдвинулся, перерегистрироваться на новый (insert-new-then-remove-old); либо в fast-path vacuum всегда сохранять один якорь (последнюю предыдущую версию) как в scan-path.

### A11. MEDIUM — recovery: `wal.commit` без A5-гейта и без персиста интернера
**Файл:** `crates/shamir-engine/src/tx/recovery.rs:243-305`.

Drainer тщательно гейтит truncate на persisted-hwm интернера (`drainer.rs:457-484`), а `recover_inflight_v2` реплеит delta в память (`touch_with_id`) и **безусловно** снимает маркер (`wal.commit`, :280) без персиста. Двойной сбой: crash → recovery → crash до первого чекпойнта → history содержит записи с id, которых нет в персистентном интернере → нечитаемые записи (WAL уже пуст).

**Фикс-эскиз:** после реплея всех entries и до `wal.commit` — один `repo_interner.persist()` (или тот же `interner_delta_safe_to_truncate`-гейт: не коммитить маркер, пока hwm не покрыл delta).

### A12. MEDIUM — `apply_replicated`: версия без `VersionGuard` клинит completion-watermark видимости
**Файл:** `crates/shamir-engine/src/tx/apply_replicated.rs:138` (`assign_next_version()` голый), `:252` (на ошибке — только `mark_durable_aborted`, visibility-tracker не помечается никогда, и на успехе тоже).

**Интерливинг:** follower применяет событие → local_version=N (не помечен в `completion`). Затем локальный tx коммитит M>N: `guard.commit()` → `mark(M, Materialized)` → `try_advance` застревает на N → `advance_last_committed` больше никогда не двигает floor через watermark. Пока 5a есть — floor тянет `publish_committed_max`; но **Deferred**-tx (5a не прошёл) полагается только на `version_guard.commit()` → его версия не публикуется вовсе, drainer её не видит (vis не растёт) → данные ждут рестарта.

**Фикс-эскиз:** использовать `assign_next_version_guarded()`; на успех — `guard.commit()`, на ошибку — drop (Aborted).

### A13. MEDIUM — `remove_table` не чистит `per_table_mvcc`: split-brain после DROP+CREATE одноимённой таблицы
**Файл:** `crates/shamir-engine/src/repo/repo_instance.rs:375-389` (нет удаления из `per_table_mvcc`), `:318-321` (`insert` молча проигрывает при повторном create).

**Интерливинг:** DROP t; CREATE t; `create_table_context` создаёт **новый** MvccStore, `per_table_mvcc.insert` проваливается (старый жив) → TableManager читает через новый стор, а commit-pipeline/SSI-provider/drainer пишут через **старый** (commit_phases.rs:511, pre_commit.rs:63, drainer.rs:410). Committed-tx-записи живут в overlay старого стора → читателям невидимы до слива в общий физический history; версии ячеек/SSI расщеплены.

**Фикс-эскиз:** `remove_table` обязан `per_table_mvcc.remove(&token)` (и `rename_table_stores` — для from-token).

### A14. LOW-MEDIUM — конкурентные `drain_all` затирают commit-time ts
**Файлы:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:292-296, 471-476` (`pending_ts.remove` — первый забирает, второй пишет `now_millis()` поверх), конкуренты: фоновый цикл (`drainer.rs:647`) и `flush_buffers` (`repo_instance.rs:1146`), `drain_to_history` (RENAME).

Двойной реплей одной entry (идемпотентен по данным) вторым проходом перезаписывает `ts_key(v)` временем дренажа → ломается T1c-контракт (as-of-ts, age-retention). Фикс: `pending_ts.read` + удалять после успешного transact, или ts-ключ писать через «insert-if-absent» семантику (проверка `history.get(ts_key)` перед записью в fallback-ветке).

---

## Секция 2. Где осторожность упускает

1. **`bump_write_counter` — залипание single-flight при панике verify** — `table/table_manager.rs:379-394`: `verify_running.store(false)` в конце тела таска; паника `verify()` пропускает сброс → фоновые verify отключены навсегда, JoinHandle дропнут — паника проглочена. Рядом (`vector_backend.rs:846`) тот же паттерн сделан правильно через Drop-guard — перенести `SnapshotFlightGuard`-паттерн сюда.
2. **Устаревшие «санкционирующие» комментарии:**
   - `layered_interner.rs:163-165`: «Must be called under RepoTxGate::commit_mutex — no internal synchronisation» — в P2c вызывается в `pre_commit_prelock` **вне** мьютекса (корректно лишь благодаря CAS-идемпотентности `touch_ind`, что нигде не заявлено как контракт).
   - `repo_tx_gate.rs:495-497` (`predicate_conflicts_batch`: «Called … UNDER commit_lock») и `staging_store.rs:157` («Must be called under commit_lock») — lock-free путь этого не делает.
   - `repo_tx_gate.rs:62-70`: описание `commit_mutex` как «серилизующего commit-секцию» — фактически его берёт только AsyncIndex-путь.
3. **Оптимистичные ordering-комментарии, не соответствующие коду:**
   - `tx/commit.rs:158-160` (HEAD): «`is_empty()` is lock-free on scc::HashMap (atomic length check)» — у scc нет атомарной длины (потому `len()` и забанен); `is_empty` — обход бакетов до первой записи.
   - `mvcc_history.rs:242-243`: «`upsert_async` … advances monotonically rather than silently keeping a stale value» — upsert именно перезаписывает, в т.ч. назад (см. A2).
   - `repo_tx_gate.rs:391-399` + `table_manager_changefeed.rs:36`: обоснование Relaxed-чтения `active_serializable_count` («tx opened AFTER the write committed») не выдерживает интерливинга «floor прочитан → инкремент счётчика» vs «версия выделена после проверки счётчика»; окно есть, следствие ограничено (порядок T-до-W остаётся валидным serial order — блайнд-врайт), но обоснование в комментарии неверное. Правильный паттерн — повторная проверка счётчика после выделения версии (Dekker store-load).
   - `repo_instance.rs:70-72`: «a lost race just drops a redundant un-spawned Drainer» — у `std::sync::OnceLock::get_or_init` closure выполняется ровно один раз; комментарий описывает несуществующую гонку.
4. **`RecordCounter::persist` — маскировка dirty-флага** — `record_counter.rs:143-163`: инкремент между `cache.load(cur)` и `dirty.store(false)` теряет свой dirty до следующего инкремента (метрика, drift допустим — но фикс тривиален: `dirty.swap(false)` до чтения `cur`, при неравенстве — вернуть true).
5. **Fire-and-forget interner-checkpoint spawn'ы** — `materialize.rs:322`, `commit_phases.rs:189`: JoinHandle дропается, паника проглатывается молча; допустимо (best-effort), но ни счётчика неудач, ни single-flight (шторм чекпойнтов при бурсте кратных INTERVAL версий).
6. **`InternerManager::save_new_keys`** — `interner_manager.rs:243-249`: `last_persisted_len += new_keys.len()` без проверки плотности id; `persisted_high_water()` — это **A5-гейт truncate**; вызванный с «не следующими» id метод завысит hwm → преждевременный truncate. Сейчас мёртвый API (0 вызовов) — удалить или закрепить контракт assert'ом.
7. **`bm25.rs:89-91`**: `fetch_sub(Relaxed)` на `doc_count`/`sum_doc_len` без floor — двойной remove (реплей) андерфлоуит в u64::MAX → мусорный avgdl в ранжировании до rebuild.

---

## Секция 3. Lock-free ради галочки / упрощения

1. **Мёртвый group-commit-механизм** — `tx/group_commit.rs` (`run_leader`, `run_single_tx`), `repo_tx_gate.rs:107,280,408-417` (`pending_commits: std::sync::Mutex`, `try_commit_lock`, `enqueue_pending`, `drain_pending`), `pending_commit.rs`: на HEAD-пути вызовов нет (`commit_tx_inner` → lockfree/legacy_async). Внутри лежит готовая ловушка: follower получает `Ok(commit_version)` **до** materialize (`group_commit.rs:353-361`) → при реанимации пути это RYOW-аномалия. Либо удалить, либо явно закомментировать как выключенное с указанием бага.
2. **AsyncIndex-путь — «lock-free» с глобальным мьютексом** — `commit.rs:499`: единственный пользователь `commit_lock`; все AsyncIndex-коммиты полностью сериализованы (ложная скалируемость для этого режима). После починки A1, вероятно, оба пути сведутся к одному механизму.
3. **Дублированные валидационные блоки** — `pre_commit_locked` (:412-544) и `pre_commit_locked_validate` (:308-397) — два почти идентичных Phase 2/2-bis/C6/claim/WAL-build; уже разошлись по деталям (см. A1-async). Слить в одну функцию + флаг «кто пишет WAL».
4. **`publish_committed` (plain store)** — `repo_tx_gate.rs:319-323`: опасный «наследный» сеттер рядом с корректным `publish_committed_max`; живых вызовов на горячем пути не осталось — удалить, чтобы никто не вернул немонотонную публикацию.
5. **`gc_upto(durable, floor)`** — `versioned_overlay.rs:154`: второй аргумент всегда `u64::MAX` (см. `gc_overlay_to`), min-логика мертва — упростить сигнатуру.
6. **Single-flight паттерн** уже есть в репо трижды (verify_running CAS, snapshot_in_flight+guard, GroupCommit leader) в трёх разных степенях надёжности — вынести общий `SingleFlightGuard` (CAS + Drop-сброс) и переиспользовать (закрывает п.2.1 и A7 разом).

---

## Секция 4. Контендед-точки (очевидное по дороге)

1. **`TxMetrics` — false sharing** — `shamir-tx/src/metrics.rs:7-25`: 10 смежных `AtomicU64` (80 байт, 1-2 кэш-линии); `txs_started`+`txs_committed` бьются каждым коммиттером. Обернуть горячие в `CachePadded` (или разнести по 64-байтным слотам).
2. **`prune_commit_log_below` — двойной обход** — `repo_tx_gate.rs:528-534`: `range(..=floor).count()` + `remove_range` — два прохода по дереву на каждый GC-тик; count нужен только для телеметрии — убрать или считать в одном проходе.
3. **Repo-wide overlay-GC на каждый drain-pass** — `drainer.rs:511-514`: `per_table_mvcc().scan(gc_overlay_to)` → полный `tree.iter()` каждого overlay каждой таблицы (внутри — collect в Vec + поштучный remove; `versioned_overlay.rs:160-186`), даже для таблиц, не тронутых проходом. Отслеживать множество «грязных» таблиц за проход; в overlay — version-major вторичный индекс (уже помечен как P1e-TODO в коде).
4. **`min_alive()` / `active_snapshots_empty()`** — `repo_tx_gate.rs:368-380`: O(N-снапшотов) scan на каждый `vacuum_key` scan-path (т.е. на каждую запись при живых снапшотах + не-CurrentOnly retention). Держать атомарный мин-хип/эпоху или AtomicUsize-счётчик снапшотов (для `_empty`-проверки — уже есть счётчик serializable, добавить общий).
5. **Drainer-атомики в одной линии** — `drainer.rs:74-103`: `window_depth` (горячий: каждый `offer` + каждый remove) соседствует с `drained_total`/`offer_dropped_total`/`recover_calls` — паддинг по желанию, эффект небольшой.
6. **`record_scan_reads`** — `table_manager_streaming.rs:262-267`: на Serializable-скане — `version_of` (scc read) + `record_read_shared` (scc entry) на **каждую строку**; для больших сканов это доминирует. После A3-фикса можно записывать версию скопом = `min(cell, snapshot)` без второго lookup'а (версия скана всегда ≤ snapshot).

---

**Примечание по методике:** находки A1–A5, A6, A10 — интерливинги, не покрываемые текущими тестами (SSI-тесты последовательны либо write-write-only; drain/rename-гонки без конкурентной нагрузки). Для каждой из них воспроизводимый тест — `#[tokio::test(flavor="multi_thread")]` с `Barrier` между validate и publish (A1/A3), между Phase B await и publish конкурента (A2), RENAME под конкурентной записью во вторую таблицу + принудительный window-gap через `set_window_high_watermark(0)` (A5).
