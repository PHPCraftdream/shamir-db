בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Ревью durability / crash-safety: shamir-storage, shamir-wal, shamir-tx (+ границы engine/tx)

_Агент: @fxx (max effort), 2026-07-06. Часть панели из 5 агентов ревью проекта после завершения векторной кампании._

Контекст, проверенный по коду: commit point = Phase 4 `wal.begin_grouped(..., Buffered)` (page cache); durable-путь = фоновый Drainer (WAL entry → `history.transact` → `mark_durable` → F6-truncation с fsync-гейтом `flush_all_history`); все дисковые сторы по умолчанию обёрнуты в write-back `MemBufferStore` поверх fjall 3.0.1 (журнал буферизован, fsync только на `persist(SyncAll)`).

---

## Секция 1 — Дыры durability (обязаны улучшить)

### 1.1 CRITICAL — Recovery глотает ошибку записи в history и всё равно объявляет версию durable
`crates/shamir-engine/src/tx/recovery.rs:413-421` (`seed_version_cache_for_entry` — `log::warn!` и продолжить) + `recovery.rs:265-277` (`recover_inflight_v2` безусловно делает `completion().mark(Materialized)` + `gate.mark_durable(v)` после `replay_v2_entry`, который вернул Ok несмотря на проглоченную ошибку).

Сценарий провала: crash → рестарт → recovery реплеит entry v=100, `write_committed_to_history` падает (ENOSPC / transient I/O) → warn → `mark_durable(100)`. Дальше: (а) читатели видят `last_committed ≥ 100`, но значения нет ни в overlay (пустой после рестарта), ни в history → отдаётся старая версия — тихая потеря подтверждённого коммита; (б) следующий drain-pass с `drained > 0` видит `has_truncatable(100)`, `flush_all_history` (флашит то, чего нет), `truncate_below` удаляет sealed-сегмент с единственной копией v=100 → **невосстановимая потеря**. Комментарий «leaving a partial history write inflight is safe — the WAL marker is untouched» — ложь пост-F6: маркеров нет, truncation идёт по watermark.

Фикс: в recovery ошибка history-записи обязана быть фатальной для open (`seed_version_cache_for_entry` → `Result`, пропагировать; «repo that cannot recover must not be served» уже задекларировано в `db_management.rs:337-343`). Минимум — не вызывать `mark_durable` для entry с неудавшейся history-половиной (зеркалировать `failed_tables`-гейтинг drain_step Phase B/C).

### 1.2 CRITICAL — Truncation WAL не гейтится на interner-hwm, а сам hwm двигается по НЕ-durable записи
Три сцепленных факта:
- F6-truncation (`crates/shamir-engine/src/tx/drainer.rs:531-542`) проверяет только `has_truncatable(durable)`; A5-гейт `interner_delta_safe_to_truncate` (drainer.rs:457-484) охраняет лишь `wal.commit(txn_id)` — который **no-op** (`crates/shamir-tx/src/repo_wal_manager.rs:109-111`). Гейт стал бутафорией.
- `InternerManager::persist` (`crates/shamir-engine/src/table/interner_manager.rs:316-320`) пишет chunk через `info_store.set` → это MemBuffer-dirty (RAM), затем сразу двигает `last_persisted_len`. `persisted_high_water()` (263-265) заявлен как «durably persisted» — фактически ни на диске, ни под fsync (двойная буферизация: MemBuffer 500 ms + fjall-журнал без fsync). `flush_buffers` (repo_instance.rs:1203-1216) делает persist интернера ПОСЛЕДНИМ и не флашит `__interner__`-store.
- Чекпоинт срабатывает только если `commit_version % 64 == 0` **и именно этот** коммит нёс interner-delta (`materialize.rs:318` HEAD, `commit_phases.rs` async-двойник) — при редком минте новых полей hwm отстаёт произвольно долго.

Сценарий: tx минтит поле `"foo"` (id=42), body записей в history закодирован этим id; сегмент с entry sealed и truncated (durable покрыл его — данные в history есть, а вот mapping id↔имя был ТОЛЬКО в WAL-entry и в RAM); crash до удачного чекпоинта+fsync → рестарт: интернер без id 42 → записи не декодируются, а следующий минт **переиспользует id 42 под другое имя** → тихая порча данных (поля читаются под чужими именами). Комментарий в `shamir-tunables/src/lib.rs:64-68` («recovery-time cost, not correctness») сегодня неверен.

Фикс: (1) сегментная truncation должна учитывать interner-потолок: не удалять sealed-сегмент, если он содержит entry с `delta_max_id > persisted_high_water` (потолок版本а вместо/вместе с durable); (2) `persist()` обязан `info_store.flush()` (drain + fjall SyncAll) ДО продвижения `last_persisted_len`.

### 1.3 HIGH — Отравление WAL-сегмента после частичной записи / неудачного fsync: последующие подтверждённые коммиты нечитаемы на replay
`crates/shamir-wal/src/wal_segment.rs:112-147` (`append_batch` — при ошибке `write_all` частично записанные байты остаются в файле, отката/усечения нет), `wal_group_commit.rs:264-271` (circuit breaker лишь отпускает лидерство — следующий leader пишет в **тот же** файл), `wal_segment.rs:217-241` (replay останавливается на первом torn/CRC-frame и молча выбрасывает всё дальше).

Сценарий: ENOSPC на середине `write_all` окна N → в файле обрубок кадра; место освободили, окно N+1 пишет дальше и даже fsync-ается (Synced ack) → power loss → replay доходит до обрубка, «torn tail», break → **все ack-нутые записи окна N+1 потеряны молча**. Аналогично после неудачного fsync (fsyncgate: ядро может пометить страницы clean без записи — повторный «успешный» fsync не спасает окно N; окно N+1 остаётся за дырой).

Фикс: при любой ошибке write/sync — карантин сегмента: запомнить offset до записи и `set_len()` назад к последней целой границе, либо принудительная ротация (открыть новый сегмент) с запретом дозаписи в отравленный; ошибка fsync = не доверять файлу (rotate + громкий лог), не «retry на том же fd».

### 1.4 HIGH — `drain_to_history` помечает durable ЧУЖУЮ версию (rename-путь)
`crates/shamir-tx/src/mvcc_store/drain.rs:86`: `self.gate.mark_durable(visibility)`, где `visibility = gate.last_committed()` — **репо-глобальная** версия, а дренится overlay ОДНОЙ таблицы.

Сценарий: таблица A имеет undrained v=5, таблица B — v=6 (= last_committed); `RENAME A` → `drain_to_history(A)` пишет v=5 в history, но помечает durable v=6 (версию B, которая в history не попадала). Когда drainer позже дометит v=5, contiguous-watermark перепрыгнет и v=6 → `gc_overlay_to(6)` по всем таблицам выбрасывает overlay-копию B(v=6) → чтения B тихо откатываются к старому значению; `flush_all_history` + truncation могут удалить сегмент с v=6 → потеря навсегда. Комментарий «advances the durable watermark to visibility» описывает не то, что делает код (маркируется одна версия, не префикс).

**Подтверждено независимо агентом «конкурентность engine» (A5) — двойное совпадение усиливает уверенность в находке.**

Фикс: маркировать durable ровно те версии, которые реально записаны (`for v in by_version.keys() { gate.mark_durable(v) }`), и не трогать `visibility`; либо вовсе не маркировать — drainer сойдётся сам (реплей идемпотентен).

### 1.5 MEDIUM-HIGH — Ограничение «250 ms data-at-risk» для Buffered-коммитов не выдерживается при сбое фонового fsync
Точка коммита — `Buffered` (`crates/shamir-engine/src/tx/pre_commit.rs:515`, `commit.rs:642` HEAD, `group_commit.rs:299-303`), окно потерь ограничивается фоновым fsync 250 ms (`repo_instance.rs:656-661`). Но `spawn_background_fsync` (`crates/shamir-wal/src/wal_group_commit.rs:315-330`): `take_dirty()` сбрасывает флаг ДО попытки, `let _ = g.sync_now()` глотает ошибку → после одного неудачного fsync dirty-состояние потеряно, ретрая не будет до следующего append; на затихшей системе окно потерь неограничено, и об этом нет даже лога.

Фикс: восстанавливать dirty при ошибке (`sync_now` err → `dirty_since_sync.store(true)`), логировать error, и считать повторные fsync-ошибки поводом для ротации сегмента (см. 1.3).

### 1.6 MEDIUM — `begin_grouped_many`: частичный append + «abort всех» = воскрешение отказанных транзакций
`crates/shamir-tx/src/repo_wal_manager.rs:84-97` (append по одному, `?` на середине) + `crates/shamir-engine/src/tx/group_commit.rs:299-319` («WAL begin failed — nothing durable» и abort всех участников).

Сценарий: batch-лидер пишет 5 entries, №3 падает (диск полон) → entries №1-2 уже durable в сегменте; все 5 клиентов получают ошибку, версии помечены Aborted → рестарт → recovery «durable = committed» (`recovery.rs:262-264`) реплеит №1-2 → транзакции, о которых клиент получил явный отказ, материализуются. Та же семантика у одиночного частичного write_all (комментарий `pre_commit.rs:488-491` «nothing durable exists» — неверен для частичной записи).

Фикс: сделать batch-append атомарным на уровне окна (одно `append` со всеми payload'ами одним вызовом группы — интерфейс `WalGroupCommit` это уже почти умеет), а частичную запись лечит карантин из 1.3.

### 1.7 MEDIUM — Регрессия recovery-маркера: `save_last_committed(commit_version)` из параллельных коммитов
`crates/shamir-engine/src/tx/commit_phases.rs:521-529` — каждый коммиттер пишет СВОЮ версию, не максимум; параллельные Phase 6.5 могут записать 9 поверх 10.

Сценарий: маркер регресснул до 9, сегмент с entry v=10 сдренен и truncated, новых коммитов нет, crash → гейт сидится `max(marker=9, max_inflight=0)` = 9 → `assign_next_version` повторно выдаёт 10 → в history появляется вторая, чужая v=10 (`ts_key(10)` перезаписан) — порча версионной шкалы. Узко, но реально.

Фикс: писать `gate.last_committed()` (монотонный максимум) вместо `commit_version`, либо CAS-max при записи маркера.

### 1.8 MEDIUM — Replay молча выбрасывает валидный хвост sealed-сегмента после одиночного CRC-сбоя
`crates/shamir-wal/src/wal_segment.rs:231-238`. Для sealed-сегмента (по I4 — полностью fsync-нут, torn tail невозможен) CRC-mismatch в середине = порча диска, но код логирует warn и выбрасывает все последующие целые кадры. Формат кадра `[len][payload][crc]` без magic/seq — ресинхронизация невозможна в принципе.

Фикс: для sealed-сегментов CRC-fail = громкая ошибка recovery (операторское решение), не warn; в формат кадра добавить magic+seq, чтобы уметь перескакивать одиночную порчу.

### 1.9 MEDIUM — Нет fsync каталога WAL после создания/ротации сегмента (Linux)
`crates/shamir-wal/src/segment_set.rs:214` (rotation открывает новый файл), `wal_segment.rs:80-106`. `sync_all()` файла не гарантирует durability directory entry: на ext4/xfs после power loss свежесозданный сегмент может отсутствовать в каталоге → ack-нутые (в т.ч. Synced) записи в нём потеряны, replay даже не узнает о файле. На Windows не проявляется, но продукт кроссплатформенный.

Фикс: после создания сегмента/ротации — fsync родительского каталога (unix: `File::open(dir)?.sync_all()`).

---

## Секция 2 — Где осторожность упущена (error-пути, Windows, диск полон)

### 2.1 HIGH — `GroupCommit::run`: отмена лидера навсегда вешает все будущие `synced_flush`
`crates/shamir-engine/src/repo/group_commit/mod.rs:44-76`: `leader_busy = true` ставится под lock, сбрасывается только в конце leader-loop. Если future лидера отменяют (клиент с `durability:"synced"` отвалился, соединение закрыто, shutdown-select) во время `flush().await`, `leader_busy` остаётся true → каждый последующий `synced_flush` паркуется в waiters и никогда не обслуживается — все synced-запросы репо висят до рестарта. Это durability-endpoint DoS.

**Подтверждено независимо агентом «конкурентность engine» (A7) — двойное совпадение.**

Фикс: RAII-guard на leader_busy (сброс в Drop) + добить waiters ошибкой; либо выполнять flush в отдельной spawn-задаче, не в теле отменяемого запроса.

### 2.2 MEDIUM — MemBuffer: фоновый flush молча глотает ошибки; сканы молча теряют dirty-хвост
`crates/shamir-storage/src/storage_membuffer.rs:263` — `let _ = Self::drain_once(...)` без единого лога (диск полон → dirty растёт бесконечно, сигналов ноль). Хуже: `iter_stream`/`scan_prefix_stream`/range-стримы (529-532, 546-551, 569-574, 592-597) делают `drain_once(...).unwrap_or(0)` — при ошибке flush цикл завершается и стрим отдаёт inner БЕЗ недофлашенных записей: полные сканы (индекс-ребилды, doctor, copy_store при RENAME) молча видят устаревшие данные.

Фикс: логировать ошибку в флашере + счётчик-телеметрия; в стримах — пропагировать ошибку drain как элемент стрима, не проглатывать.

### 2.3 MEDIUM — `MemBufferStore::transact` теряет конкурентную запись: `dirty.remove(&k)` вместо `remove_if`
`storage_membuffer.rs:643-649`: после `inner.transact` из dirty удаляется ключ безусловно. Конкурентный `set(k)` между `inner.transact` и `remove` кладёт в dirty новое значение — оно удаляется, во внутренний стор не попадает никогда; после eviction кэша/рестарта durable-состояние — старое. (drain_once рядом делает это правильно, snapshot+`remove_if`.)

Фикс: убрать удаление вовсе (drain_all в начале уже опустошил эти ключи) или `remove_if` со сравнением.

### 2.4 MEDIUM — `WalSegment::replay`: `PermissionDenied` → `Ok(vec![])` даже на старте
`crates/shamir-wal/src/wal_segment.rs:202-210`. Обоснование (delete-pending при конкурентном truncate) корректно только для гонки в работающем процессе. Но тот же код выполняется при `SegmentSet::open`/recovery на старте: реальный ACL-отказ или файл, удерживаемый антивирусом/бэкапом, превращается в «пустой WAL» → recovery молча пропускает durable-записи. (BACKLOG-пункт про lingering-файл — смежный, но другой: там утечка, тут — тихая потеря на replay.)

Фикс: терпеть `PermissionDenied` только когда truncation конкурентна (флаг в SegmentSet «этот path claimed на удаление»), на open/recovery — ошибка.

### 2.5 MEDIUM — Проглоты в recovery-репле
`crates/shamir-engine/src/tx/recovery.rs:141` — broadcast-IndexPut: `let _ = tbl.info_store().set(...)` (IndexDel рядом ошибки пропагирует — несимметрично); `recovery.rs:192-199` — `InternerOverlayMerge`: `if let Ok(...)` на резолве интернера, `let _ = interner.touch_ind(...)`, `let _ = repo_interner.persist()` — отказ мержа интернера на recovery молча съеден, следствия те же, что в 1.2.

Фикс: пропагировать; для broadcast-веток собирать первый Err после попытки всех таблиц (паттерн `flush_buffers`).

### 2.6 MEDIUM — Загрузка интернера считает порчу «пропускаемой»
`interner_manager.rs:163-167` (битый legacy-blob → «пустой словарь») и 182-197 (битый chunk → skip; ошибка скана → break). Продолжение работы с усечённым словарём = минт новых id поверх занятых = молчаливая порча всех старых записей. Чексуммы у chunk'ов нет (заявка «checksums everywhere» здесь не выполняется — целостность делегирована fjall, но decode-fail всё равно обрабатывается как «пропустить»).

Фикс: любой сбой декода при загрузке интернера — фатальная ошибка open.

### 2.7 LOW-MEDIUM — `Clone for BoxRepoFactory` молча заворачивает raw-fjall в MemBuffer
`crates/shamir-engine/src/repo/repo_types.rs:301-313`: `Fjall(f) → BoxRepoFactory::fjall(path)` = `wrapped(...)`. Клон фабрики, созданной `fjall_raw()` (инструментарий, тесты «bit-for-bit»), меняет durability-семантику на write-back.

Фикс: клонировать вариант as-is.

### 2.8 LOW — `CachedStore` WriteMode::Async: неупорядоченные фоновые записи
`storage_cached.rs:196-219, 248-268`: каждый set/remove — отдельный `tokio::spawn`; порядок недетерминирован → для одного ключа старое значение может лечь в inner ПОСЛЕ нового; после рестарта durable-состояние перепутано (это не «потеря при crash», а порча при чистой работе). Opt-in режим, но контракт («data may be lost on crash») описывает не тот риск.

Фикс: per-key последовательность (очередь) или honest-doc + запрет для info/history.

### 2.9 LOW — `rekey_sorted_prefix` (RENAME INDEX): non-atomic copy+delete
`table_manager_index_mgmt.rs` (HEAD, ~525-535): `set_many` новых ключей, затем `remove_many` старых, без `transact`, без WAL. Crash между — дубликаты постингов под старым префиксом навсегда (направление хотя бы безопасное — не потеря).

### 2.10 LOW — `wal_ops_from_tx` молча теряет op с не-RecordId ключом
`commit.rs:202, 212` (HEAD): `if let Some(rid) = try_from_bytes(&k)` без else. Сегодня недостижимо (ключи всегда RecordId), но при появлении иного формата ключа запись попадёт в overlay (видимость), не попадёт в WAL → после drain+GC исчезнет. Нужен `debug_assert!`/error.

---

## Секция 3 — Контракты между слоями и устаревшие комментарии-ложь

1. **`crates/shamir-wal/src/lib.rs:33-55`** — вся модульная дока описывает удалённый KV-marker дизайн (`__wal_active_` в info_store, «marker removed after»); production — файловые сегменты, маркеров нет (F5c/F6). Это дока входной точки крейта — вводит в заблуждение первой. Переписать.
2. **`drainer.rs:33-42`** («Scope of P1d-2a — additive, NOT wired... cutover is P1d-2b») и шапка шагов (реплей через `replay_v2_entry`, «truncate the inflight marker») — устарело: drainer давно wired как единственный history-писатель, шаг — Phase A/B/C.
3. **`wal_segment.rs:15`** «Wired to nothing yet» — ложь, это production-путь.
4. **`repo_tx_gate.rs:554-560`** — «under the current inline-materialize path this equals last_committed... P1d-2 will decouple» — уже decoupled.
5. **Семантика `mark_durable`** — означает «записано в history (page cache)», не «durable на диске»; настоящий fsync живёт только в truncation-гейте. Либо переименовать (`mark_history_written`), либо явный контракт в доке гейта — сейчас имя провоцирует ошибки вида 1.1/1.4.
6. **`interner_manager.rs:253-263`** — «durably persisted to the chunk store» у `persisted_high_water` — неправда (RAM+буфер, см. 1.2).
7. **`recovery.rs:409-412`** — «leaving a partial history write inflight is safe — the WAL marker is untouched» — пост-F6 ложь (см. 1.1).
8. **`pre_commit.rs:488-491, 518-521`** — «failed wal.begin ⇒ nothing durable» — неверно при частичной записи окна (см. 1.6).
9. **Владение retry**: drainer ретраит (Phase B failed_tables, следующий pass) — хорошо; recovery НЕ ретраит и НЕ фейлится (1.1) — контракт должен быть «recovery либо довела до durable, либо open падает». Idempotency реплея заявлена и в целом выдержана (Put/Del/IndexPut/Del — LWW; CounterDelta — намеренно skip, документировано), опора корректная.
10. **Контракт `flush_all_history` ↔ truncation** структурно хрупок: fsync-гейт итерирует `per_table_mvcc` — history-store, выпавший из карты (rename/drop table между drain и truncate), не флашится; сегодня спасает только то, что fjall `persist(SyncAll)` — общий на всю БД. Контракт стоит закрепить: «flush по физическому Database handle, не по списку таблиц».
11. **`repo_wal_manager.rs:62-63`** «cancel-safe: yes» для `begin_grouped` — формально да (future паркуется), но семантически отмена НЕ отменяет append: entry станет durable и воскреснет на рестарте, а вызывающий увидит отмену. Комментарий стоит дополнить (в `commit_tx` это уже честно описано).

---

## Секция 4 — Что ускорить (замечено по дороге)

1. **Двойная запись индекс-постингов**: ack-путь применяет их в Phase 5c (`materialize`/`apply_index_batch`), а drainer Phase A реплеит те же IndexPut/IndexDel второй раз через `replay_v2_op` (`drainer.rs:346-363`) — 2× записи в info_store на каждый индексный op каждой транзакции + транзиентная возможность воскресить постинг, который параллельный более новый коммит уже удалил (сходится только на следующем pass'e). Дренить индекс-опы стоит только для НЕприменённых entries (или убрать из Phase A, оставив recovery-путь).
2. **`flush_all_history` = N полных fsync fjall**: `repo_instance.rs:1238-1256` вызывает `mvcc.flush_history()` на каждую таблицу, каждый — `db.persist(SyncAll)` всей базы (`storage_fjall.rs:270-279`). На репо с 50 таблицами — 50 полных fsync на одно truncation-событие. Дедуплицировать по физическому Database.
3. **Открытие репо читает WAL целиком до 4 раз**: `tx_gate` pre-scan (`repo_instance.rs:547`), floor txn_id (`repo_instance.rs:676`), `recover_v2_inflight`, drainer seed (`drainer.rs:605-608`); плюс `SegmentSet::open` полностью реплеит каждый sealed-сегмент ради `max_version` (`segment_set.rs:133-137`). Один replay с шарингом результата закрыл бы всё.
4. **Recovery реплеит уже-сдренённые entries активного сегмента** на каждом open (идемпотентно, но O(WAL) записей в history + повторная вставка версий, которые vacuum уже удалил). Фильтр по `durable`-floor из маркера снял бы основную массу.
5. **`MemBufferStore::transact` клонирует весь ops-vec** (`storage_membuffer.rs:633`) ради пост-обновления кэша — можно обновлять кэш из тех же KvOp без `ops.clone()` (Bytes дешёвые, но Vec+обход двойной).

---

## Топ-5 «обязаны сделать»

| # | Что | Где | Серьёзность | Суть фикса |
|---|-----|-----|-------------|-----------|
| 1 | Recovery: ошибка записи history фатальна, `mark_durable` только после успешной записи | `shamir-engine/src/tx/recovery.rs:413-421, 265-277` | CRITICAL | Пропагировать Err из `seed_version_cache_for_entry`; не маркировать durable упавшие entry — иначе truncation удаляет единственную копию ack-нутого коммита |
| 2 | Interner: гейтить F6-truncation на persisted-hwm и делать hwm честным (flush до продвижения) | `drainer.rs:531-542`, `interner_manager.rs:316-320`, `materialize.rs:318` | CRITICAL | Потолок truncation = min(durable, версия с покрытым delta_max_id); `persist()` → `info_store.flush()` перед `last_persisted_len.store` |
| 3 | Карантин WAL-сегмента при ошибке write/fsync (ENOSPC, fsyncgate) | `shamir-wal/src/wal_segment.rs:112-147`, `wal_group_commit.rs:264-271` | HIGH | ftruncate до последней целой границы кадра / принудительная ротация; запрет append в отравленный сегмент — иначе ack-нутые Synced-коммиты за torn-frame теряются молча |
| 4 | `drain_to_history`: маркировать durable только реально записанные версии | `shamir-tx/src/mvcc_store/drain.rs:86` | HIGH | `mark_durable` по ключам `by_version`, не по репо-глобальной `visibility` — иначе rename под нагрузкой «удуравливает» чужую недренённую версию (GC overlay + truncation → потеря) |
| 5 | `GroupCommit::run`: устойчивость к отмене лидера | `shamir-engine/src/repo/group_commit/mod.rs:44-76` | HIGH | RAII-сброс `leader_busy` + fail waiters (или detached-flush-task) — сейчас отменённый synced-запрос навсегда вешает durability-флаш всего репо |

Следом по приоритету: 1.5 (потерянный dirty-флаг фонового fsync + молчание), 1.6/1.7 (воскрешение отказанных tx; регрессия маркера → переиспользование версии), 2.2/2.3 (MemBuffer: молчаливые flush-ошибки и гонка `transact`), 2.4 (PermissionDenied-as-empty на старте), и переписывание лживой доки `shamir-wal/src/lib.rs` + `drainer.rs` (секция 3), чтобы следующий инженер строил рассуждения не на удалённой архитектуре.
