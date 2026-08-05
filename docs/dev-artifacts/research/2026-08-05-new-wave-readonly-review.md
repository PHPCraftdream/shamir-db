# Read-only review новой волны перед первым релизом

**Дата:** 2026-08-05  
**Репозиторий:** `D:\dev\rust\shamir-db`  
**HEAD:** `37cc59a3` (`master`)  
**Предыдущая точка review:** `72106a908fd817433170496bab376d2fba27f7ab` (отчёт `2026-08-03-new-wave-readonly-review.md`)  
**Диапазон:** 99 коммитов, 215 файлов, `+25 766 / -1 148` (задачи #957–#1005)  
**Режим:** только чтение Git и файлов. Cargo-команды, тесты, бенчмарки и сервер не запускались. Единственное изменение — этот отчёт.

> Примечание о методике: на момент старта ревью по этому же пути лежал незакоммиченный черновик более раннего ревью-прохода. Его утверждения НЕ принимались на веру — каждый значимый тезис перепроверен по текущему коду с чтением конкретных функций; подтверждённые вошли сюда с моими file:line, неподтверждённые/вторичные — отброшены или понижены. Этот файл заменяет черновик.

## Короткий вердикт

Волна #957–#1005 — целевая отработка прошлого NO-GO-списка, и она честная: **5 из 6 прошлых P0 закрыты или закрыты в главной части** (admission mutex, tx-rederive для всех четырёх семейств, durable DROP-tombstone + recovery, fail-closed corrupt unique posting, персистентный rename с resumable recovery). Отдельно ценно, что волна дважды прогонялась адверсариальным ревью и фиксы фиксов (#987, #1000, #1005) реально ловили регрессии в только что написанном коде.

Тем не менее для публичного `v0.1.0-alpha.1` вердикт остаётся **NO-GO**. Причины две:

1. **P0-6 (perf-gate) не тронут вообще**: `bench-baseline.json` в дереве нет, self-hosted runner `shamir-bench` по-прежнему не зарегистрирован, а `release.yml` требует `perf-gate` в `needs` — тег физически не соберётся.
2. Новые механизмы (generation-гейты, tombstones, admission) **пока не образуют единый lifecycle-протокол**, и на их стыках подтверждены НОВЫЕ дефекты корректности (см. §4): рассинхронизация `insert_ticket`/`generation` в `IndexRegistry` (пропуск rederive после DROP→CREATE), sorted RENAME без bump generation (staged tx воскрешает старый namespace), асимметричный reconcile (retraction только для base-семейства), partial-insert реестра без rollback, admission-мьютекс, покрывающий не весь DDL, и fail-open open-path recovery (`restore_on_open`/`drop_all` → warn + Ready).

Расширять DDL/OQL крупно поверх этого всё ещё рано; но объём оставшихся работ заметно меньше, чем 2026-08-03 — это адресные дефекты стыков, а не отсутствующие протоколы.

## 1. Что изучено

- история `72106a90..37cc59a3` (все 99 коммитов, включая briefs в `docs/dev-artifacts/prompts/`);
- `CHANGELOG.md` `[Unreleased]`, checkpoints `2026-08-05-*`;
- `TableManager` (barrier/admission/drain), `table_manager_tx_ops`, `tx/pre_commit.rs` (Phases 2.5–2.7, все три rederive-функции, 2c-retain), `tx_context`;
- `shamir-index`: `registry.rs` (insert/remove/rename_entry/generation), `base_index/{index_manager, index_manager_unique, sorted_index_manager}` (drop/rename tombstones, recovery, sweep), `persistence.rs`;
- open-path recovery в `TableManager::create` (index2 Building self-heal, `recover_index2_drops`, `recover_hash_renames`, `restore_on_open` loop);
- `in_flight_create_guard.rs`, `degraded_index_count.rs`, `shamir-server/src/observability.rs` (#984/#1003/#1005);
- `#983` (Bin-сравнения + `FilterValue::Binary` strict deserializer), `#989/#995` (auth до existence-probe);
- query-builder: `builder_error.rs`, `into_batch_op.rs`, `try_*` API (#965), `ddl/index_spec.rs` + `create_index.rs` (#970/#986), общая fixture-матрица Rust/TS (#998/#1004/#1005);
- `KNOWN_LIMITATIONS.md`, perf-gate/release workflows, `scripts/bench_gate.sh`;
- diff волны на предмет запрещённых примитивов (std::sync::Mutex/RwLock, parking_lot, `scc::len()`, raw HashMap).

Рабочая копия до записи отчёта: только untracked `docs/checkpoints/2026-08-05-2010.md` и черновик этого файла. Git-командами дерево не менялось.

## 2. Статус прошлых P0 (отчёт 2026-08-03)

| P0 | Статус | Чем закрыт / что осталось |
|---|---|---|
| P0-1 barrier multi-owner | **Закрыт** (#957) | `TableManager::ddl_admission: Arc<tokio::sync::Mutex<()>>`, берётся ПЕРВЫМ в `begin_write_barrier` (`table_manager.rs:907-935`), guard встроен в `WriteBarrierGuard` и живёт всю критсекцию; drop-order: бит очищается раньше admission. Ровно рекомендованный в прошлом отчёте вариант. Остаток: admission покрывает не весь DDL → NP-5 (§4). |
| P0-2 tx plan staleness | **В основном закрыт** (#958, #987, #992, #993) | 2a: `rederive_base_index_ops_post_stage` (`pre_commit.rs:1151+`) + generation у `IndexManager` (`base_index/index_manager.rs:1032-1048`, bump на 4 мутационных сайтах); #987 переставил вызов ПОСЛЕ Phase 2.5-lock и ДО Phase 2.6-валидации — unique staged-before-create теперь реально проверяется (тест воспроизводит corruption). 2b: во всех 6 staging-сайтах gen читается ДО снапшота (`table_manager_tx_ops.rs:386-399` + note_*_stage_gen), а `IndexRegistry::insert` публикует ДО `generation.fetch_max(Release)` с выделенным `insert_ticket` (#992, `registry.rs:121-199`). 2c: retract staged ops для DROP'нутых base-индексов через retain по длине ключа + `(is_unique, name_interned)` (`pre_commit.rs:1318-1368`) с lock-in тестом коллизий (#993). Остатки: NP-1 (ticket/generation desync re-открывает угол 2b), NP-3 (retraction нет для sorted/index2; ABA по имени у base). |
| P0-3 DROP без протокола | **3c закрыт, 3b частично, 3a открыт** | 3c (crash-resurrection): durable tombstone → retire → sweep → persist → clear + идемпотентное open-time recovery для ВСЕХ трёх семейств: base (#959, `index_manager.rs:893-1000`, crash-state matrix), sorted (#972, `sorted_index_manager.rs:796+`), index2 (#988, `table_manager_index_mgmt.rs:829-970`, `recover_index2_drops`). 3b: для tx — 2c-retain (base) и generation-гейт (sorted bump на drop, `sorted_index_manager.rs:924`), но retraction sorted/index2 нет (NP-3); для non-tx writers — regular/unique drop обёрнуты в `begin_write_barrier` (`index_mgmt.rs:791-822`), а sorted/index2 drop — НЕТ (NP-5). 3a (in-flight reader видит частично выметенный keyspace): честно задокументирован как KNOWN GAP (`index_manager.rs:1616-1630`), кода нет. |
| P0-4 corrupt unique fail-open | **Закрыт** (#960) | `index_manager_unique.rs:173-201`: длина ≠16 → typed `DbError::Codec` («genuine corruption — fail-closed»), не `Ok(None)`. Мелкий остаток: backfill `create_unique_index` молча `continue`-ит недекодируемые записи (`:372-380`) — см. P2 в §5. |
| P0-5 rename persistence | **Закрыт** (#961, #962, #997, #1000) | 5a: authoritative name/name_interned слоты в `BackendEntry` (`registry.rs:166-187`), `rename_entry` мутирует их (`:367-…`), `all_descriptors` персистит из слотов, `remove_by_id` анлинкует по актуальному имени (`:259-280`) — rename переживает reopen. 5b: sorted rename получил durable `Renaming`-tombstone ДО `rename_definition` + resumable settle-rekey (`sorted_index_manager.rs:1057-1103`, `recover_in_progress_renames:1274`), зависимости от atomic `transact` больше нет (settle-loop — принятый F-85-паттерн). #997 добавил compound-rename recovery для hash/unique с crash-matrix; #1000 (@oh) починил реальную потерю sibling-tombstone в `recover_hash_renames`. Новый остаток другого рода: NP-2 (sorted rename vs staged tx). |
| P0-6 perf-gate | **ОТКРЫТ, не тронут** | `bench-baseline.json` отсутствует и не tracked; `perf-gate.yml:16` по-прежнему говорит, что под label `[self-hosted, shamir-bench]` нет машины; `release.yml` требует job. В волне НИ ОДНОГО коммита в `.github/`/`bench_gate.sh` (пустой diffstat). Всё сказанное 2026-08-03 в силе дословно. |

## 3. Статус прошлых P1

| P1 | Статус | Комментарий |
|---|---|---|
| P1-1 Building без self-heal | Частично (#966, #984, #1003, #1005) | `doctor::verify()` теперь показывает state+message и требует `Ready` для `is_healthy()`; `/metrics` получил `shamir_degraded_indexes_total` (poller в `observability.rs:399-478`), false-positive на живом CREATE закрыт per-identity набором `InFlightCreateSet` (после того как @oh поймал масочный дефект скалярного счётчика). Auto-heal base-семейства на open — по-прежнему нет (сознательно отложен в #966); и `verify()/repair()` **не выведены ни в CLI, ни в admin API** — «run repair()» для владельца бинарника пока неисполнимая инструкция. |
| P1-2 Err после live-mutation | Частично (#967 + RFC #985) | Все партиально-персистные сайты 4 семейств теперь возвращают обогащённую ошибку («что durable, что покажет restart, что делать»). Единый машинный DDL result contract — **только RFC** (`660fd82a`, design-only: op_id + poll-by-status + embedded ddl_status): реализации нет, wire по-прежнему success/error. |
| P1-3 `transact` двусмысленность | **Закрыт** (#968) | Док начинается с «NO cross-op atomicity» по умолчанию, capability описан честно, приведён аудит всех production-callers с их конкретным паттерном толерантности (settle/re-scan и т.д.) (`shamir-storage/src/types.rs:179-…`). |
| P1-4 DDL write stall | Частично (#969) | Alpha bar: progress-лог backfill'а, JSDoc про requestTimeoutMs, KNOWN_LIMITATIONS §3 с ИЗМЕРЕННЫМИ числами (100k rows ≈ 140–160 s write outage, суперлинейно; 1M экстраполируется в часы), бенч f78 расширен. Архитектура (barrier на весь скан) не менялась — это осознанное решение для alpha, но см. §7: для заявленного сегмента это главный продуктовый риск. |
| P1-5 panic-prone builders | Частично (#965) | Добавлены `BuilderError`, `TryIntoBatchOp`, `Batch::try_update/try_upsert/try_delete/try_op(_after)` — аддитивно. Паникующий путь (`into_batch_op.rs:53/68/83`, `replication.rs:290/297`, `schema.rs:508/515`) остался ДЕФОЛТНЫМ. До API freeze нужно либо сделать fallible-путь основным, либо переименовать паникующие в `*_unchecked`. |
| P1-6 stringly CreateIndex | В основном закрыт (#970, #986, #998, #1004, #1005) | `try_build()` валидирует 12 классов invalid state; внутренний typed `IndexSpec` (Hash/Sorted/Fts/Functional/Vector, `dim: NonZeroU32`) делает нелегальные комбинации непредставимыми пост-конверсии (`ddl/index_spec.rs`); Rust и TS гоняются по ОДНОМУ файлу фикстур (`shamir-query-builder/tests/fixtures/create_index_matrix.json` — TS-тест резолвит тот же путь, копий нет), с настоящей per-variant-coverage проверкой (после @oh-фикса #1005). Остаток: публичный builder всё ещё принимает строки (index_type/metric/tokenizer/quantization), `build()` permissive by design. |

Также закрыты из прошлого §8: `RenameIndexOp.if_exists` теперь существует и документирован (`index_ops.rs:108-115`, #971); стейл-комменты про `rekey_sorted_prefix`-atomicity переписаны; `legacy` → `base_index` переименован по всему репо (#973).

## 4. НОВЫЕ P0 — подтверждены по коду этой волны

Все сценарии ниже проверены трассировкой актуального кода (не скопированы из черновика).

### NP-1. `IndexRegistry`: `insert_ticket` и `generation` рассинхронизируются после remove — rederive-гейт молча не срабатывает

**Код:** `crates/shamir-index/src/registry.rs:164` (`insert_ticket.fetch_add`), `:198` (`generation.fetch_max(my_gen)`), `:275` (`remove_by_id` → `generation.fetch_add(1)`); гейт — `crates/shamir-engine/src/tx/pre_commit.rs:825` (`if reg.generation() == stage_gen { continue }`) и `registry.rs:237-248` (`backends_newer_than`).

#992 корректно устранил гонку одинаковых тегов у конкурентных insert'ов, но ввёл второй счётчик, который НЕ двигается на remove. Детерминированный последовательный сценарий:

1. `CREATE INDEX A` (index2): ticket=1, publish, `generation = max(0,1) = 1`.
2. `DROP INDEX A`: `remove_by_id` → `generation.fetch_add` → **2**; `insert_ticket` остаётся 1.
3. Транзакция stage-ится: `note_index2_stage_gen = 2` (A уже нет — план корректно пуст).
4. `CREATE INDEX B`: ticket=2, publish, `fetch_max(2)` — **generation остаётся 2**, entry.gen=2.
5. Commit: `generation() == stage_gen (2)` → Phase 2.7 rederive пропущен целиком; даже без гейта `backends_newer_than(2)` вернул бы пусто (`entry.gen > 2` ложно).

Итог: строка committed, но навсегда отсутствует в B — то самое «tx plan устаревает», которое P0-2 закрывал. Никакой конкуренции не нужно, `DROP → CREATE` между stage и commit достаточно. Существующий тест (#992) проверяет уникальность тегов, а не инвариант «то же наблюдаемое generation ⇒ тот же planner-visible набор».

**Исправление:** один источник монотонности для ЛЮБОЙ мутации снапшота — например, remove тоже тикетирует (`insert_ticket.fetch_add` + `generation.fetch_max`), либо generation = один `fetch_add` на mutation с publish-first порядком (тогда `fetch_max` не нужен, а инвариант 2b сохраняется, т.к. bump всё равно после publish). Тесты: `CREATE A → DROP A → stage → CREATE B → commit`; инвариант равенства generation ⇔ равенство множества `(id, entry.gen)`.

### NP-2. Sorted RENAME не двигает generation — staged tx пишет в старый namespace и минует новый

**Код:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:1024-1055` (`rename_definition` — RCU-замена `name_interned`, перенос epoch-entry, `persist_defs`; **нет** `generation.fetch_add`, в отличие от register `:615` и drop `:924`); гейт — `pre_commit.rs:1028`.

Сценарий: tx stage-ит sorted-ops под `SORTED_TAG||old_id`; RENAME (tombstone → `rename_definition` → rekey settle-loop) завершает перенос; tx commit-ится: sorted generation не изменился → rederive не запускается → SetPosting уходит под `old_id`. Результат: orphan postings в снятом namespace (а `old_id` = intern(старое имя) — повторный CREATE под старым именем получает ghost-postings) и **отсутствующая строка в переименованном индексе**. Write-barrier здесь не спасает, даже если его добавить: барьер сериализует, но не инвалидирует staged план — план инвалидирует только generation-гейт, а он не срабатывает.

**Исправление:** bump generation в `rename_definition` (симметрично drop) — этого достаточно, чтобы Phase 2.7-sorted rederive перепланировал по текущим defs; плюс retraction старых sorted-ops (NP-3), иначе rederive добавит новые, но старые тоже применятся. Тест: stage → RENAME → commit → проверка обоих namespace.

### NP-3. Reconcile асимметричен: retraction устаревших ops есть только у base-семейства; у base — ABA по имени

**Код:** `pre_commit.rs:1311-1368` (retain фильтрует ТОЛЬКО ключи 41/25 байт с первым байтом ≤1 — т.е. base regular/unique); `rederive_index2_ops_post_stage:797+` и sorted-ветка `:1011+` — только `extend`, никакого remove.

Следствия:

- **stage → DROP sorted → commit**: generation-гейт срабатывает, rederive по текущим defs корректно ничего не добавляет, но СТАРЫЕ staged sorted-ops (25-байтный ключ с первым байтом `SORTED_TAG=0x80` и/или длинные ключи — retain их пропускает) применяются в Phase 5c → воскрешение postings под удалённым id. Так как sorted id = `name_interned`, последующий `CREATE` с тем же именем наследует ghost-postings **чужого определения** — прямое загрязнение нового индекса.
- **stage → DROP index2 → commit**: аналогично, orphan postings под старым числовым id (новый CREATE берёт новый id, поэтому «только» storage leak + противоречие с только что построенным crash-recovery инвариантом «после drop postings нет»).
- **base ABA**: retain оставляет ops, чей `(is_unique, name_interned)` жив. `DROP x → CREATE x` (то же имя, другое поле) между stage и commit → старые ops с хэшами старого поля retained → загрязнение нового индекса x. Для unique — ложные конфликты/чужой owner в posting.

**Исправление (одно на всё):** перестать идентифицировать staged ops эвристикой по байтам. Каждой `IndexWriteOp` — typed provenance: семейство + immutable instance id (для base это должен стать отдельный id, не `name_interned`; или catalog epoch). Reconcile тогда «заменяет план инстанса целиком»: drop retired instance ops, derive для новых. Тестовая матрица — как в прошлом отчёте P0-2, плюс `stage → DROP → CREATE same-name-different-field → commit` для всех семейств.

### NP-4. `IndexRegistry::insert` при коллизии имени оставляет полусостояние; open path игнорирует ошибку insert

**Код:** `registry.rs:178-196` — `by_id.insert_async` успешен, `by_name.insert_async` падает → `Err` без отката `by_id`; `table_manager.rs:511` — `let _ = mgr.index2_registry.insert(backend).await;` на open path.

Последствия: backend жив по id (виден `all_backends`/`backends_newer_than`/planner-обходам по полям), но недоступен по имени; `DROP` по имени удалит другой инстанс; на restart дубликат метаданных молча проглатывается, а Building-ветка выше уже успела сделать полный backfill и `set_state(Ready)`. Каталог и registry расходятся без диагностики.

**Исправление:** проверка/резервирование имени ДО вставки в `by_id` (или откат `by_id` при ошибке `by_name`); на open path — fail-closed (ошибка открытия таблицы или descriptor → `Failed`), а не `let _`. Плюс invariant-check `by_id ↔ by_name ↔ persisted` в `verify()`.

### NP-5. Admission/barrier покрывает не весь DDL

**Код:** `table_manager_sorted_index.rs:297-304` — `drop_sorted_index` зовёт `sorted_indexes.drop_index` напрямую, без `begin_write_barrier`; `table_manager_index_mgmt.rs:877-970` — `drop_index2` делает tombstone → retire → sweep без барьера/admission; RENAME-пути также не держат единую admission-секцию на весь compound-переход.

Следствия: (а) заявление «per-table DDL admission» сильнее фактического — DDL разных точек входа могут пересекаться (частично компенсировано `dropping_*`-guard'ами, напр. `sorted_index_manager.rs:588-600` отклоняет CREATE при живом DROP, но не наоборот); (б) для sorted/index2 DROP остаётся окно non-tx writer'а: fast-path writer, снапшотнувший defs до retire, допишет posting после sweep — тот самый класс 3b, который для regular/unique закрыт именно барьером (`index_mgmt.rs:791-822`).

**Исправление:** единая точка входа — все CREATE/DROP/RENAME всех четырёх семейств через `begin_write_barrier` (bits уже есть) либо через общий DDL-координатор; внутренние manager-API сделать `pub(crate)`/требующими guard-token, чтобы обход был невозможен синтаксически.

### NP-6. Open-path recovery fail-open: ошибка восстановления оставляет индекс Ready

**Код:** `table_manager.rs:491-503` — `drop_all` при self-heal Building: `log::warn!` + продолжить backfill («partial postings may persist» — для FTS это ещё и двойной счёт статистики); `:579-585` — `restore_on_open` failure: `log::warn!`, backend остаётся в registry со state Ready → planner-visible с пустым/неполным in-memory состоянием (vector: молча неполные результаты поиска).

Для БД silent wrong result хуже отказа открыться. **Исправление:** неудачный restore → `Failed`/`Building` (planner-invisible, попадает в degraded gauge #984 — инфраструктура для этого уже есть!) либо ошибка открытия; `Ready` — только после доказанного полного восстановления.

### Перенос из прошлого отчёта: P0-3a (in-flight reader при DROP) — открыт сознательно

`index_manager.rs:1616-1630` теперь честно документирует: reader, разрезолвивший индекс до retire, может увидеть частично выметенный keyspace (меньше кандидатов → неполный результат). Механизма (reader-epoch/grace period) в кодовой базе нет. Для alpha допустимо задокументировать в `KNOWN_LIMITATIONS.md` (сейчас признание живёт только в doc-comment) и закрыть в R-волне read-side epochs — но решение «документируем» должно быть принято явно, поэтому оставляю в P0-списке решений, не кода.

## 5. Новые P1/P2

1. **P1: doctor/repair недоступен оператору.** Все новые error-тексты и метрика отсылают к `TableManager::verify()/repair()`, но ни CLI-команды, ни admin-endpoint нет. Минимум для alpha: `shamir-server doctor --data-dir` (offline) или authenticated admin op.
2. **P1: CI не зелёный на HEAD.** По checkpoint `2026-08-05-2010.md`: run `31032501528`, job `cargo test integration (windows-latest)` красный — 4 batch-e2e теста дважды упали с `batch execution exceeded its 30s time budget` (~140 s wall). Гипотеза «Windows-runner flakiness» не доказана. Релизный SHA обязан быть зелёным без ручного rerun; нужен разбор (либо тест получает явный budget, либо это деградация Windows-пути).
3. **P1: CHANGELOG-конфликт для тега.** Есть и `[Unreleased]` (вся волна), и `[0.1.0-alpha.1] - 2026-07-26` (`CHANGELOG.md:17,92`). Тег `v0.1.0-alpha.1` сейчас вытащит июльские notes, а 99 коммитов останутся в Unreleased. Решить: перенести Unreleased в alpha.1 с новой датой ЛИБО релизиться как alpha.2. (Node `0.1.0` — документированный scope gap, не дефект, но решение о составе релиза server-only vs SDK надо зафиксировать письменно.)
4. **P2: fail-open пропуски в backfill.** `create_unique_index` (`index_manager_unique.rs:372-380`) молча `continue`-ит ключи ≠16 байт и недекодируемые записи — недекодируемая строка не попадает в unique-индекс, и её значение позже может быть «свободно». Противоречит fail-closed политике #960; в идеале — typed error, минимум — счётчик+warn с отражением в verify().
5. **P2: контракт 2c-retain хрупок by design.** Байтовая эвристика 41/25+prefix задокументирована и залочена тестом (`p02c_retain_filter_key_collision_tests`), но остаётся миной для нового backend'а; NP-3-фикс (typed provenance) её устранит — не вкладываться в её локальные улучшения.

## 6. Соответствие идеологии CLAUDE.md (вопрос 3)

Волна дисциплинированная, нарушений пяти столпов на hot paths **не найдено**:

- Все новые `std::sync::Mutex` — DDL-only guard-sets (`InFlightCreateSet`, `renaming_*`, `dropping_*`) с inline-обоснованием contention-модели, по прецеденту; hot paths не трогают. (@oh-нит про миграцию `InFlightCreateSet` на `scc` отклонён осознанно — согласен, это не приоритет.)
- Новые структуры — `TFxSet` в retain (`pre_commit.rs:1339`), `TSet` в тестах; `scc::len()` не появлялся (clippy-бан), O(N)-обходы registry аннотированы `O(N) ack` и вне hot path.
- `degraded_index_count` — O(индексов), zero store I/O, корректно вынесен в push-poller.
- Rederive-гейты zero-cost на common path (одно Acquire-load на семейство при commit).
- TDD-протокол соблюдён образцово: каждый фикс несёт discriminating regression test, а #1005-follow-up даже чинил недискриминирующий тест.
- Замечание не по коду, а по модели: `name_interned` как физический id (base/sorted) — первопричина и ABA (NP-3), и ghost-postings при reuse имени. Это единственное место, где текущая архитектура системно конфликтует с O(x→0)/lock-free-протоколами: immutable instance id снимет целый класс проблем.

## 7. Производительность (вопрос 6)

Сверка со списком 2026-08-03:

| Пункт | Статус в волне |
|---|---|
| DDL write stall | Измерен (#969: 100k ≈ 140–160 s outage, суперлинейно), задокументирован, НЕ исправлен. Это пункт №1 после correctness: online build (snapshot scan → delta capture → catch-up → короткий cutover) либо жёсткий enforced-лимит для alpha. |
| Unique build memory | Не тронут — по-прежнему две O(table) структуры (`index_manager_unique.rs:393-399`, F-78 deferred, теперь хотя бы честно в KNOWN_LIMITATIONS). |
| DROP memory | Не тронут — `sweep_index_postings` копит ВСЕ ключи в `Vec<RecordKey>` до одного `remove_many` (`index_manager.rs:900-911`). Уместно чинить заодно с NP-3/NP-5 (bounded batches внутри settle-паттерна, который уже освоен в #962). |
| GROUP BY/DISTINCT spill, result streaming | Не тронуты; остаются главным memory/latency ceiling для средних проектов (полный `Vec` результата). |
| Backfill batch size, unique validation batching | Не тронуты (`FULL_SCAN_BATCH` захардкожен). |
| Perf gate quality | Не тронут (см. P0-6). |

Новых пессимизаций волна не внесла: rederive и retain оплачиваются только при реально прошедшем DDL; барьерные пути прежние.

## 8. DDL: что развивать (вопрос 4)

Прямой ответ: **расширять DDL-поверхность поверх текущего lifecycle всё ещё рискованно** — но список причин сузился с «нет протоколов» до «протоколы есть, стыки дырявые» (NP-1…NP-6). Порядок:

**До релиза (R0):** NP-1…NP-6 + решение по 3a. Ядро — два системных шага, закрывающих по 2-3 дефекта разом: (1) immutable `IndexInstanceId` + typed op provenance (NP-2-остаток, NP-3, хрупкость 2c); (2) единый DDL-координатор поверх уже существующего `ddl_admission` (NP-5) + fail-closed open path (NP-4, NP-6).

**Сразу после (R1):** реализация RFC #985 (op_id + `Accepted/Building/Ready/Retired/Failed` + poll) — RFC хороший, и без него online build/ALTER только умножат неоднозначность; вывод doctor/verify/repair в operator-интерфейс; unified `DROP INDEX name` без клиентского `unique: bool` (каталог уже классифицирует — `DropIndexOp` это последний stringly-рудимент); единый cross-family namespace имён (NP-обнаруженный пробел признан в `in_flight_create_guard.rs:39-47`).

**После alpha.1:** `DESCRIBE/LIST INDEXES` с state/progress/last_error; `ALTER INDEX REBUILD/VALIDATE`; online/resumable build + cancel; composite sorted/unique; partial indexes. Новое семейство индексов — только после подчинения четырёх существующих одному контракту.

## 9. OQL: что развивать (вопрос 4, часть 2)

Приоритеты прошлого отчёта не устарели, волна их не трогала (она и не должна была):

1. computed SELECT expressions (`SelectItem::Expression` есть в wire, executor отвергает — dead surface, реализовать или убрать из контракта);
2. `EXPLAIN ANALYZE` (actual rows / выбранный instance / причина отказа от индекса) — станет ещё ценнее ПОСЛЕ введения instance id;
3. настоящий streaming/cursor на уровне engine (не повторный full scan на страницу);
4. `EXISTS`/alias-lookup как безопасная ступень до JOIN;
5. composite index-aware ORDER/RANGE.

JOIN/window/set-ops — по-прежнему только после bounded execution + spill + cost model. E2E-покрытие OQL волной существенно расширено (#974–#981) — хорошая база, чтобы эти шаги делать test-first.

## 10. Query Builders (вопрос 5)

`IndexSpec`-подход (#986) — **правильный образец, распространять стоит**:

- Уже сделано: typed IR + 12 структурных проверок + единая Rust/TS fixture-матрица с настоящей variant-coverage (единственный файл фикстур, обе стороны читают его — дрейфа нет). Это и есть та «declarative schema/fixture matrix», которую просил прошлый отчёт.
- Следующие кандидаты в порядке отдачи: (1) сделать fallible-путь Batch ОСНОВНЫМ (#965 добавил `try_*`, но `.expect()`-конверсии остались дефолтом — до API freeze это дёшево, после — breaking); (2) `UpdateSpec`/`UpsertSpec`/`DeleteSpec` — их invalid-состояний мало, typestate (или spec-enum) тривиален; (3) публичные typed-методы у CreateIndex (`.fts(field, Tokenizer)`, `.vector(field, NonZeroU32, Metric)`) поверх уже существующего IR — строки останутся только в wire DTO; (4) fixture-матрицы по образцу create_index для остальных DDL-builders (rename/drop/replication) — дешёвая parity-страховка.
- DDL-builders должны будут вернуть operation handle, когда ляжет RFC #985 — закладывать сигнатуры уже сейчас (Result<DdlHandle>, не Result<()>).

## 11. Документация и release engineering

Хорошо: KNOWN_LIMITATIONS пополнен измеренными числами (§3 write stall, unique memory); CHANGELOG ведётся честно вплоть до самокритики фиксов; briefs всех делегированных задач закоммичены до запуска (протокол prompt-first соблюдён); push сопровождён checkpoint'ом с открытым CI-вопросом, а не замолчан.

Перед тегом (дельта к прошлому списку): P0-6 (runner+baseline+dry-run); CHANGELOG-heading конфликт (§5.3); Windows integration red (§5.2); зафиксировать состав релиза (server-only vs SDK, Node 0.1.0); задокументировать 3a в KNOWN_LIMITATIONS, если он сознательно остаётся; backup/restore drill на замороженном SHA.

## 12. Punch list до `v0.1.0-alpha.1`

**P0 (блокируют тег):**
1. NP-1 registry generation/ticket unification + инвариант-тест (маленький фикс, большой эффект).
2. NP-2 sorted rename generation bump (однострочный + тест).
3. NP-3 typed op provenance / retraction для sorted+index2 + base-ABA (самый крупный пункт волны R0).
4. NP-4 атомарная двух-проекционная вставка registry + fail-closed open path.
5. NP-5 admission на sorted/index2 DROP и rename-пути.
6. NP-6 Failed-state вместо warn+Ready при неудачном restore/drop_all.
7. Решение по 3a (grace period ИЛИ документированное ограничение).
8. P0-6 perf-gate: runner, baseline, dry-run тега.
9. Зелёный CI (включая Windows integration) на замороженном SHA.

**P1 (сильно желательно до тега):**
10. doctor/repair в CLI или admin API.
11. CHANGELOG/версии/состав релиза.
12. Fallible-путь Batch как основной (пока breaking дёшев).
13. Реализация RFC #985 хотя бы в объёме op_id + status enum в ответе.

**После alpha:** online CREATE INDEX (или enforced-лимит уже в alpha), unique build/DROP memory, streaming results, unified DROP INDEX, cross-family namespace, публичные typed builder-методы, остальное из §8-§10.

## 13. Минимальный exit-checklist

- [ ] `CREATE A → DROP A → stage → CREATE B → commit` даёт rederive для B (NP-1).
- [ ] stage → sorted RENAME → commit: строка в новом индексе, старый namespace пуст (NP-2).
- [ ] stage → DROP → commit не воскрешает postings ни в одном из 4 семейств; stage → DROP → CREATE same-name → commit не загрязняет новый индекс (NP-3).
- [ ] duplicate-name insert реестра не оставляет полусостояния; open path fail-closed на дубликат (NP-4).
- [ ] sorted/index2 DROP и все rename-пути проходят через admission/barrier (NP-5).
- [ ] Ошибка restore_on_open/drop_all → индекс не Ready и виден в degraded gauge (NP-6).
- [ ] 3a: grace period реализован ЛИБО ограничение внесено в KNOWN_LIMITATIONS и release notes.
- [ ] `bench-baseline.json` committed, perf-gate прошёл на зарегистрированном runner'е, release dry-run не в очереди.
- [ ] Тот же SHA: fmt / clippy `-D warnings` / `./scripts/test.sh --full` / loom / TS+Node e2e / Windows integration — зелёные без rerun.
- [ ] CHANGELOG-heading соответствует тегу; состав релиза задокументирован.
- [ ] Backup → destructive mutation → restore → verification drill пройден.

## Итог

Направление верное, и темп впечатляет: прошлый NO-GO-список отработан почти полностью, с настоящей recovery-инфраструктурой (tombstones, crash-matrix, resumable settle) и рабочей культурой адверсариальных ревью, которая уже дважды спасла от регрессий. Оставшаяся работа — не «переделать фундамент», а сшить швы: один источник монотонности generation, один identity для инстанса индекса, одна точка входа DDL, один fail-closed рефлекс на open path — плюс чисто инфраструктурный P0-6. После этого alpha-тег станет честным.
