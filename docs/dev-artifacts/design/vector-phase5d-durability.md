# VR-3 — Durability of Phase 5d (vector promote + delta-append)

> Issue #425. Design-only. No code changes.

## 0. Контекст и подтверждённый дефект

Транзакция, пишущая векторное поле, проходит на коммите:

```
Phase 4  wal.begin            → COMMIT POINT (WAL entry durable)
Phase 5a apply_data_batch     → overlay / history (data durable via drainer)
Phase 5c apply_index_batch    → info_store postings
Phase 6  version_guard.commit → last_committed_version published
Phase 6.5 persist_markers     → last_committed / next_tx_id persisted
[commit_lock released]
Phase 5d promote_vectors      → live HNSW graph + append_vector_delta
```

Phase 5d (`crates/shamir-engine/src/tx/commit_phases.rs:261`, `promote_vectors`)
выполняется ПОСЛЕ снятия WAL-маркера (Phase 7, которым после D2-cutover владеет
background `Drainer` — `crates/shamir-engine/src/tx/drainer.rs:461`). TxOutcome
возвращается клиенту строго ПОСЛЕ `promote_vectors` на синхронных путях
(`finalize_sync_post_publish`, `crates/shamir-engine/src/tx/finalize.rs:65-88`)
и ДО `promote_vectors` на AsyncIndex-пути (`commit_tx_inner_legacy_async`,
`crates/shamir-engine/src/tx/commit.rs:563-587` — background-task).

Сам `apply_vector_batch` (`commit_phases.rs:453-517`) для каждого vector-backend'а
выполняет подряд три шага:
1. `apply_staged_vector_deletes` — tombstone удалённых rid в live графе;
2. `apply_staged_vectors` — upsert новых векторов в live граф;
3. `append_vector_delta` — durable-запись дельта-чанка в info_store
   (`snapshot::append_delta`, `crates/shamir-index/src/vector/snapshot.rs:1172`),
   затем `trigger_snapshot_check` (fire-and-forget generation flip).

### Дефект (HIGH design, подтверждён ревью)

`commit_phases.rs:250-260` и `:293-302` обещают:

> the graph reconciles via `VectorBackend::rebuild` on the next open

Это **устарело** с V2.2/V2.3. `VectorBackend::restore_on_open`
(`crates/shamir-index/src/vector/vector_backend.rs:619-715`) при живом снапшоте
загружает снапшот + дельта-лог (`snapshot::replay_delta`, `snapshot.rs:1213`) и
**rebuild НЕ выполняет**. Rebuild (`vector_backend.rs:536-583`) — это лишь ветка
fallback при `NotFound` / `Corrupt` / `VersionMismatch` снапшота.

Следствие: если Phase 5d окончательно провалилась ПОСЛЕ ack (или процесс упал
между ack и delta-append), vector-мутация **никогда не материализуется** ни в
живом графе, ни в delta-log. Следующий снапшот сдампит тот же неполный граф →
расхождение индекса с data store **перманентно**. WAL-replay не помогает:
векторы НЕ сериализуются как `IndexPut` (см. `wal_ops_from_tx`,
`crates/shamir-engine/src/tx/commit.rs:145-225` — в `WalOpV2` идут только
`Put`/`Delete`/`IndexPut`/`IndexDel`/`CounterDelta`/`InternerOverlayMerge`;
вектор идёт в тело `Put` как обычное поле записи, а HNSW-промоут НЕ эмитируется
в WAL вообще).

Дельта-лог здесь — единственный durable-мост между data store и live-графом, и
именно его запись (`append_vector_delta`) оказалась за пределами WAL-гарантии.

---

## 1. Crash-окно (текущее состояние)

Чтобы рассуждать о вариантах, фиксируем два окна:

| Окно | Где | Что durable на диске | Что видит рестарт |
|------|-----|----------------------|-------------------|
| **W-1** (до `version_guard.commit`) | Phase 5a..5c / `materialize` до `:215` | WAL inflight + overlay | `recover_inflight_v2` → replay данных+индекса; Phase 5d не выполнена, но и не должна — tx ещё не committed-visible |
| **W-2** (после publish, до/во время Phase 5d) | `finalize_sync_post_publish:73-86` между `post_publish_cleanup` и концом `promote_vectors` | WAL inflight (drainer не успел trunc), маркеры persisted, версия published | Данные+индекс восстановятся; векторное поле ЕСТЬ в data store (тело `Put`), но HNSW-узел отсутствует в live-графе **И** дельта-чанк не записан → `restore_on_open` загрузит снапшот+дельту без этой мутации |

На AsyncIndex-пути (`commit.rs:563-587`) окно **W-2 шире**: ack уходит ДО
spawned-task, так что клиент видит COMMITTED ещё до начала `materialize_async_tail`,
а `promote_vectors` бежит в фоне — сбой задачи (panic, OOM) не доходит до клиента
вовсе.

Падение в W-2, при котором `apply_staged_vectors`/`apply_staged_vector_deletes`
успели изменить live-граф в RAM, но `append_vector_delta` не записалась, —
**безопасно** для одного процесса (граф в RAM умер вместе с процессом). Опасен
случай, когда Phase 5d **окончательно провалилась** (persistent storage error,
превышён `MATERIALIZE_ATTEMPTS = 3`, `commit_phases.rs:15`): warn-лог пишется,
tx остаётся COMMITTED, и больше эту мутацию **никто не повторит**.

---

## 2. Три варианта исправления

Для каждого: crash-матрица, идемпотентность, латентность commit-пути, сложность,
риск. Во всех вариантах `apply_staged_vectors`/`apply_staged_vector_deletes` на
graph остаётся в текущей позиции (они изменяют только RAM-граф; durable-часть —
это `append_vector_delta`).

### Вариант A — delta-append ДО ack, в commit critical section

**Идея.** Перенести `append_vector_delta` внутрь критической секции коммита —
между Phase 5a (data durable) и publish (`version_guard.commit`), но строго ДО
снятия WAL-маркера. Replay при recovery идемпотентен (upsert/delete —
last-write-wins), поэтому повторное применение безопасно.

**Механика.** `apply_vector_batch` уже пишет дельта-чанк сразу после
graph-мутации; перенос сводится к тому, чтобы вызывать `append_vector_delta`
**до** `version_guard.commit()` и до возвращения TxOutcome. На lockfree-пути —
внутрь `materialize` (`materialize.rs:59`), до Phase 6 publish
(`materialize.rs:215`); на AsyncIndex — в `apply_data_phase` или
`apply_data_phase`+отдельный `apply_vector_delta_phase` до
`version_guard.commit()` (`commit.rs:553`).

**Crash-матрица.**

| Упали здесь | Диск | Рестарт |
|-------------|------|---------|
| До `wal.begin` (Phase 4) | ничего tx-related | clean abort, как сегодня |
| Phase 4 durable, delta-append ещё не записан | WAL inflight | `recover_inflight_v2` не эмитит вектор в WAL → как сегодня (W-1) |
| **delta-append записан, Phase 6 publish не дошёл** | WAL inflight + дельта-чанк в info_store | `recover_inflight_v2` материализует данные; `restore_on_open` при следующем открытии table загрузит снапшот+дельту → мутация ЕСТЬ |
| publish + markers, ack ушёл | то же + маркеры | то же; дельта-чанк на месте |

**Ghost-риск (delta опережает data store).** Дельта-чанк может быть записан, а
сама tx затем «провалится» — но это **невозможно** в варианте A: дельта пишется
ПОСЛЕ Phase 5a (overlay уже опубликован) и ПОСЛЕ Phase 4 (WAL durable). Транзакция
в этой точке уже COMMITTED по данным. Единственное окно — «Phase 4 durable, но
Phase 5a ещё не опубликовал overlay, и здесь же дельта-запись»: это окно
сужается до нескольких строк между `apply_data_batch` и `append_vector_delta`.
Упадок здесь оставит WAL-inflight + дельта-чанк, который **ссылается на rid,
которого ещё нет в data store** (overlay не опубликован, history не записан).
На рестарте: `recover_inflight_v2` материализует `Put` (тело записи), затем
`restore_on_open` проиграет дельту — данные консистентны. Дельта НЕ опережает
data store, потому что обе записи идут в один info_store/data_store бэкенд
(shared flush в crash-тестах — `commit.rs:88-93`).

**Связь с generation flip.** Фоновый снапшот (`run_background_snapshot`,
`vector_backend.rs:873`) захватывает `next_delta_idx` HWM ДО дампа
(`vector_backend.rs:901`). Если дельта-чанк записан, но tx ещё не опубликован,
HWM уже продвинут (`fetch_add` в `append_delta`, `vector_backend.rs:756`), и
дамп поглотит этот чанк. Снапшот будет содержать вектор, который «опережает»
data-store-visible версию — но поскольку сам вектор уже в теле `Put` (Phase 4
durable), это **безопасно**: рестарт просто увидит вектор раньше, но и data
replay его подтвердит. Idempotency upsert гарантирует сходимость.

**Латентность commit-пути.** Добавляется ровно одна `Store::set` (запись
дельта-чанка) на критическом пути ДО ack. `snapshot.rs:1165-1193` показывает:
один `store.set(delta_chunk_key, bytes)`. На redb/fjall — одна memtable-вставка
(deferred fsync). Стоимость: ~десятки микросекунд на типичном NVMe. НО: Phase 5d
выполняется per-table, для каждого vector-backend отдельно
(`commit_phases.rs:474`), итерация по `tbl.index2_registry().all_backends()`.
Для tx, пишущего в M таблиц с N векторными индексами каждая — M×N доп.
`Store::set` на ack-пути. На больших пакетах (bulk vector insert) это
заметно: N=10k векторов в одном tx идут в ОДИН чанк (`vecs: &[(RecordId, Vec<f32>)]`
передаётся целиком), так что стоимость — O(индексов), а не O(строк).

**Идемпотентность.** Replay дельты (`replay_delta`, `snapshot.rs:1213-1271`)
применяет `DeltaOp::Upsert` через `adapter.upsert`, `DeltaOp::Delete` через
`adapter.delete`. Обе операции last-write-wins на адаптере. Повторный replay
того же чанка идемпотентен. Дублирующая запись чанка на тот же `idx`
(`next_delta_idx.fetch_add` на ack + возможный re-attempt после рестарта) —
`Store::set` перезаписывает тот же ключ, last-writer-wins.

**Сложность.** Средняя. Нужно:
1. Разделить `apply_vector_batch` на graph-half и delta-half, либо ввести флаг
   «только дельта» для pre-publish вызова.
2. Перенести вызов delta-half в `materialize` до `version_guard.commit()`.
3. На AsyncIndex — аналогично в `commit_tx_inner_legacy_async` до publish.
4. Graph-half (`apply_staged_vectors`) остаётся на текущем месте (после lock).

**Риск.** Изменяет порядок на критическом пути; при ошибке delta-append tx
должен стать `Deferred` (WAL-marker остаётся inflight, recovery повторяет).
Но `recover_inflight_v2` **не повторяет** векторные мутации (векторов нет в
WAL) — поэтому одного «Deferred» недостаточно: нужно, чтобы дельта-чанк был
записан ДО того, как tx может быть объявлен committed. Если delta-append
провалился — tx НЕ публикуется (abort до commit point невозможен, но publish
можно задержать). Это меняет контракт «Phase 4 = commit point» — самый тонкий
момент варианта A.

### Вариант B — reconcile при restore_on_open

**Идея.** После загрузки снапшота+дельты в `restore_on_open`
(`vector_backend.rs:619`) сверять cardinality/версию индекса с data store. При
расхождении — фоновая доливка недостающих векторов через скан таблицы
(по образцу `rebuild`, `vector_backend.rs:536-583`, который уже умеет
`source.iter_stream` + `extract_vec` + `upsert_batch`).

**Механика.** В `restore_on_open` после успешной ветки
(`vector_backend.rs:648-694`) добавить проверку:

- посчитать `live_count` адаптера (HnswAdapter expose len через `hnsw_len` или
  аналог);
- посчитать число записей с embedding-полем в data store (scan или
  pre-computed cardinality из info_store);
- если `live_count < data_count` — запустить `rebuild`-подобный скан, но
  upsert-ами в уже загруженный граф (а не в пустой).

Альтернативный маркер — высоководный tx-id/commit_version, зафиксированный в
снапшоте. Но `SnapshotManifest` (`snapshot.rs:278-309`) **не содержит
commit_version** — только `gen`, `delta_applied_upto`, chunk counts, basename.
Добавить поле можно (`format_version` bump → v3, обратно-несовместимо с
существующими снапшотами).

**Crash-матрица.** Вариант B не меняет commit-путь; меняется только open-path.

| Упали здесь | Диск | Рестарт |
|-------------|------|---------|
| Любая точка W-2 | data durable, дельта-чанк отсутствует | `restore_on_open` грузит снапшот+дельту → `live_count < data_count` → запускается скан-доливка → консистентно |

**Стоимость cold-start скана.** `rebuild` (`vector_backend.rs:551-581`) —
полный `iter_stream(batch_size=1000)` по data store + `upsert_batch` на каждую
страницу. На таблице 10M строк с dim=768 это минуты (построение HNSW с нуля).
Скан-доливка в варианте B дешевле полного rebuild (граф уже есть, добавляются
только недостающие узлы), но всё равно O(N) — каждый rid нужно проверить на
присутствие в графе. Brute-force-проверка `adapter.contains(rid)` по 10M rid —
сотни миллисекунд на hash-lookup'ах, что приемлемо для cold-start, но заметно.

**Ложные расхождения.** Cardinality-проверка `live_count == data_count` хрупка:
- Tombstoned rid (soft-delete в HNSW) уменьшают `live_count`, но не `data_count`
  (запись ещё в data store как удалённая). Нужен отдельный счётчик
  live-not-deleted.
- Non-tx мутации (`plan_insert`/`plan_update`/`plan_delete` из CRUD/репликации,
  см. `docs/dev-artifacts/design/vector-compaction.md:12-24`) **не пишут в delta-log** вообще
  — после рестарта они видны только через `rebuild`. Cardinality всегда будет
  расходиться, если таблица принимает non-tx writes. Это делает «ложное
  срабатывание» нормой, а не исключением → деградация cold-start до O(N)-скана
  на каждом открытии.

**Откуда взять надёжный маркер.** Варианты:
1. `SnapshotManifest.commit_version` (новое поле, v3 manifest) — фиксирует
   версию, до которой снапшот консистентен с data store. На open сравнить с
   `gate.last_committed()`: если `snapshot_cv < last_committed` — есть
   недозахваченные мутации, нужен скан. Но это не отличит tx-мутации от non-tx.
2. Per-index high-water в info_store (`__vec_hwm__<index_id>`), обновляемый
   после каждого успешного Phase 5d. Дёшево, но НОВЫЙ durable write на ack-пути
   → возвращаемся к стоимости варианта A.

**Сложность.** Высокая. Нужно:
1. Релиable cardinality (учёт tombstones, non-tx writes).
2. Либо manifest v3 + back-compat, либо отдельный hwm-маркер.
3. Скан-доливка как новый кодовый путь (частичный rebuild в загруженный граф).
4. Эвристика «когда запускать скан» (не на каждом открытии).

**Риск.** Самый высокий из трёх. Cardinality-расхождение — симптом, а не
причина; оно срабатывает на ВСЕХ non-tx мутациях, превращая cold-start в
постоянный O(N)-скан. Не решает корневую проблему (отсутствие durable-гарантии
на Phase 5d), а лишь обнаруживает её следствие.

### Вариант C — WAL-повтор Phase 5d

**Идея.** Не снимать WAL-маркер до успешного promote+delta. При recovery
(`recover_inflight_v2`) повторять Phase 5d по WAL: если tx-carried ops указывают
на векторное поле, повторить `apply_vector_batch`.

**Механика.**
1. Эмитить в WAL признак «есть векторные мутации» (новый `WalOpV2::VectorPromote`
   с `(table_token, descriptor_id, [(rid, vec_bytes)], [deleted_rid])`) либо
   флаг на существующем `Put`.
2. `promote_vectors` переносится внутрь commit critical section (как вариант A),
   либо WAL-маркер не снимается до его успеха.
3. `recover_inflight_v2` после replay данных+индекса вызывает `promote_vectors`
   для каждой replayed tx, у которой есть векторные ops.

**Проблема 1: векторы в WAL.** Сейчас векторы НЕ сериализуются в WAL отдельно
(см. `wal_ops_from_tx`, `commit.rs:145-225` — только тело `Put`). Чтобы
повторить Phase 5d из WAL, нужно положить туда `(rid, embedding)` — это
**дублирование** данных (embedding уже в теле `Put`). Либо парсить тело `Put`
при replay, чтобы извлечь embedding (связь engine↔index в обратную сторону —
нарушение слойности). Оба варианта плохи: первый раздувает WAL вдвое для
векторных таблиц, второй ломает модульность.

**Проблема 2: идемпотентность повторного promote.**
- **Upsert** идемпотентен (`adapter.upsert(rid, vec)` повторно — no-op, тот же
  вектор).
- **Delete** на адаптере идемпотентен (`apply_staged_vector_deletes` повторно —
  tombstone уже стоит).
- **Интерливинг с последующими коммитами.** Если tx T1 (rid=R, vec=V1)
  провалилась на Phase 5d, а tx T2 (rid=R, vec=V2) успела промоутнуться, то
  replay T1 после T2 **перезапишет** R с V1 (last-write-wins на адаптере, но
  порядок replay — по `commit_version` ascending, `recovery.rs:250`).
  Поскольку T2 имеет `commit_version > T1`, T2 replay-ится позже и снова
  перезапишет R на V2. Корректно — но только если replay идёт строго по
  `commit_version`, что уже гарантируется (`recovery.rs:250`).

**Проблема 3: дельта-чанк при replay.** `append_vector_delta` при replay
запишет **дублирующий** дельта-чанк (новый `next_delta_idx`). Это безвредно для
консистентности (replay применит оба, last-write-wins), но раздувает
delta-log. Нужен guard «не писать дельту, если чанк уже существует» — а это
требует O(1)-проверки по `(tx_id, rid)` или скана delta-keyspace.

**Crash-матрица.**

| Упали здесь | Диск | Рестарт |
|-------------|------|---------|
| Phase 5d провалилась, WAL-маркер НЕ снят | WAL inflight + векторные ops в WAL | `recover_inflight_v2` повторяет Phase 5d → граф+дельта консистентны |
| Phase 5d успешна, WAL-маркер снят | как сегодня (W-2 закрыт, т.к. success) | норма |

**Латентность commit-пути.** Перенос `promote_vectors` в критическую секцию —
это возврат к III.5-problem: HNSW-per-vector work (`spawn_blocking` на hnsw_rs)
под `commit_lock` stall-ит всех остальных коммиттеров. Именно от этого
`materialize.rs:197-204` явно ушёл. Вариант C это **отменяет**.

Если же оставить `promote_vectors` post-lock, но просто НЕ снимать WAL-маркер до
его успеха — drainer (владелец `wal.commit` после D2) должен ждать Phase 5d.
Drainer обрабатывает версии в порядке durable-watermark; привязка
«не-trunc-ать entry, пока его tx не сделала promote» — это новое
синхронизационное ребро между drainer и commit-путём. Риск deadlock: если
promote провалился persistently, drainer никогда не trunc-нёт entry →
WAL-tail растёт без предела → `apply_backpressure` (`commit.rs:328-396`)
park-ует всех коммиттеров навечно (deadlock guard `BACKPRESSURE_MAX_WAIT = 5s`
спасает, но это деградация).

**Сложность.** Самая высокая. Нужно:
1. Новый `WalOpV2` вариант или флаг — schema-изменение WAL.
2. Replay-ветка в `recovery.rs` для векторных ops.
3. Либо перенос promote в critical section (отмена III.5), либо синхронизация
   drainer↔promote.
4. Дедупликация дельта-чанков при replay.

**Риск.** Высокий. Schema-изменение WAL обратно-несовместимо; синхронизация
drainer↔promote — новое deadlock-ребро; дублирование embedding в WAL —
расход места.

---

## 3. Сравнительная матрица

| Критерий | A (delta до ack) | B (reconcile on open) | C (WAL-replay 5d) |
|-----------|------------------|----------------------|-------------------|
| **Закрывает W-2** | ✅ полностью | ⚠️ обнаруживает и чинит на open | ✅ полностью |
| **Идемпотентность** | ✅ upsert/delete LWW | ✅ (скан upsert) | ⚠️ нужен dedup чанков |
| **Латентность ack** | +1 `Store::set` на vector-индекс | 0 | 0 (но promote под lock = отмена III.5) ИЛИ новое rib deadlock |
| **Cold-start** | без изменений | O(N) скан при расхождении (часто — на каждой non-tx таблице) | без изменений |
| **WAL schema** | без изменений | без изменений | обратно-несовместимое изменение |
| **non-tx writes** | не актуально (нет tx → нет Phase 5d) | ложные срабатывания → постоянный скан | не актуально |
| **Сложность** | средняя | высокая | очень высокая |
| **Риск регрессии** | низкий-средний | высокий (cardinality-эвристика) | высокий (WAL + drainer sync) |

---

## 4. Рекомендация: Вариант A

**Обоснование.**

1. **Минимальное изменение contract surface.** Вариант A не трогает WAL schema,
   не добавляет синхронизационных рёбер (drainer остаётся владельцем Phase 7),
   не меняет cold-start. Стоимость — одна дополнительная `Store::set` на
   векторный индекс на ack-пути, что сопоставимо с уже существующей
   `append_vector_delta` (которая и так выполняется, просто позже).

2. **Идемпотентность гарантирована существующим механизмом.** `replay_delta`
   (`snapshot.rs:1213`) уже применяет `DeltaOp` через `adapter.upsert/delete`
   (last-write-wins). Перенос записи чанка на более раннюю точку не меняет
   replay-семантику — меняется только момент, когда durable-копия появляется.

3. **Закрывает именно корневую причину.** Проблема в том, что durable-мост
   (дельта-чанк) записывается за пределами гарантии. Вариант A переносит его
   запись внутрь гарантии. Вариант B лечит симптом (расхождение), вариант C
   строит второй параллельный мост (WAL-replay) поверх уже существующего.

4. **Не конфликтует с non-tx writes.** Non-tx путь (`plan_insert`/CRUD) вообще
   не пишет в delta-log (см. `vector-compaction.md:16`); его консистентность
   обеспечивается `rebuild` на open (fallback-ветка `restore_on_open`).
   Перенос tx-дельты в критическую секцию не затрагивает этот путь.

**Уточнение по AbstractPath / AsyncIndex.** На AsyncIndex-пути ack уходит ДО
spawned-task. Чтобы вариант A работал и там, `append_vector_delta` должен
выполнуться в `apply_data_phase` (`commit_phases.rs:64-93`) или новом
`apply_vector_delta_phase` — синхронно, до `version_guard.commit()`
(`commit.rs:553`). Graph-half (`apply_staged_vectors`) остаётся в spawned-task
(RAM-мутация вне критического пути). Это разделение уже намечено в
`apply_vector_batch` (`commit_phases.rs:453-517`): graph-шаги
(`apply_staged_vector_deletes`, `apply_staged_vectors`) и delta-шаг
(`append_vector_delta`) — отдельные вызовы; нужно лишь разнести их по фазам.

---

## 5. Эскиз реализации

> Design-only. Файлы и функции — для последующей задачи имплементации.

### 5.1. Изменения

**`crates/shamir-engine/src/tx/commit_phases.rs`**
- Разделить `apply_vector_batch` (`:453-517`) на:
  - `apply_vector_graph_batch` — `apply_staged_vector_deletes` +
    `apply_staged_vectors` (остаётся post-lock, в `promote_vectors`);
  - `apply_vector_delta_batch` — только `append_vector_delta` +
    `trigger_snapshot_check` (переносится pre-publish).
- Добавить `pub(crate) async fn apply_vector_delta_phase(tx, repo,
  commit_version)` — вызывается до `version_guard.commit()`, аналогично
  `apply_data_phase` (`:64`). Итерирует `tx.staged_vectors` +
  `tx.staged_vector_deletes`, для каждого vector-backend'а вызывает
  `apply_vector_delta_batch`.
- При ошибке delta-append — `tx.materialization_deferred = true` / возврат
  `Deferred` (WAL-marker остаётся inflight; но т.к. вектора нет в WAL,
  recovery не повторит — см. §5.3 «остаточный риск»).

**`crates/shamir-engine/src/tx/materialize.rs`**
- В `materialize` (`:59`), после Phase 5c и ДО `version_guard.commit()` (`:215`),
  вызвать `apply_vector_delta_phase`.

**`crates/shamir-engine/src/tx/commit.rs`**
- В `commit_tx_inner_legacy_async` (`:492`), между `apply_data_phase` (`:539`)
  и `version_guard.commit()` (`:553`), вызвать `apply_vector_delta_phase`.

**`crates/shamir-index/src/vector/vector_backend.rs`**
- Без изменений. `append_vector_delta` (`:737`) уже отделён от graph-мутаций.

### 5.2. Что НЕ меняется

- `promote_vectors` (`commit_phases.rs:261`) — остаётся post-lock, выполняет
  только graph-half (`apply_vector_graph_batch`). При ошибке graph-promote
  live-граф лагает до следующего open, но дельта-чанк уже durable → `restore_on_open`
  его применит. Контракт из `commit_phases.rs:250-260` становится **верным**.
- `recover_inflight_v2` (`recovery.rs:243`) — без изменений.
- WAL schema — без изменений.

### 5.3. Остаточный риск

Если `apply_vector_delta_phase` провалилась persistently (3 retry исчерпаны),
tx публикуется с `Deferred`. Но recovery НЕ повторит векторную мутацию
(вектора нет в WAL). Это означает: **провал delta-append всё равно теряет
векторную мутацию из live-графа**. Отличие от текущего состояния: теперь
это обнаружимо (`Deferred` в TxOutcome + warn-лог + метрика) и происходит
ДО ack, а не после. Клиент может retry-нуть tx. При варианте A аборт до
publish невозможен (Phase 4 уже durable), но `Deferred`-сигнал даёт
оператору сигнал «требуется внимание».

**Митигация (опционально, вне scope этой задачи):** добавить в
`SnapshotManifest` поле `commit_version` (v3 manifest, back-compat через
`SNAPSHOT_SUPPORTED_VERSIONS`, `snapshot.rs:723`). Тогда `restore_on_open`
может сравнить `manifest.commit_version` с `gate.last_committed()` и при
отставании запустить ограниченный скан (гибрид A+B). Но это самостоятельная
работа — не блокирует вариант A.

---

## 6. Эскиз тестов

По образцу `crates/shamir-engine/tests/crash_recovery.rs` (child-process +
`SHAMIR_TEST_CRASH_AFTER` + `process::abort`).

### 6.1. Новый crash-seam `phase5d_delta`

В `commit.rs::maybe_crash` добавить метку `phase5d_delta` — точка между
`apply_vector_delta_phase` (variant A, pre-publish) и `version_guard.commit()`.
На этой точке `process::abort()` (с `store.flush()` — моделируем durable
delta-chunk).

**Тест:** `crash_at_phase5d_delta_recovers_vector`

```
child: open repo, create vector index (dim=N), insert tx с векторной записью,
       commit → abort at "phase5d_delta"
parent: reopen, recover_v2_inflight, get_table, vector lookup top-1
        → assert hit (rid из tx присутствует в графе)
        → assert rebuild_count == 0 (снапшот+дельта, не full-scan)
```

Доказывает: дельта-чанк durable ДО publish → рестарт применяет его через
`restore_on_open::replay_delta`, вектор виден.

### 6.2. Негативный: провал delta-append

Использовать существующий механизм `FAIL_VECTOR_PROMOTE_TX_ID`
(`commit_phases.rs:54`) — расширить до `FAIL_VECTOR_DELTA_TX_ID` для delta-half.

**Тест:** `phase5d_delta_failure_marks_deferred`

```
in-process: arm FAIL_VECTOR_DELTA_TX_ID = tx_id
            commit tx с вектором
            → assert TxOutcome.materialization == Deferred
            → assert warn-лог содержит "Phase 5d delta"
            → assert live-граф НЕ содержит rid (graph-half не успел или
              намеренно не запущен)
            → reopen → restore_on_open (снапшот пустой) → rebuild из data store
              → rid присутствует (rebuild-fallback закрывает случай без снапшота)
```

### 6.3. Регресс: happy-path latency

Бенч (через `shamir_bench_utils::tune`, изолированный target-dir):

```
CARGO_TARGET_DIR=...\cargo-target-bench cargo bench -p shamir-engine --bench vector_commit_latency
```

Сравнить ack-latency до/после варианта A на bulk-insert (1k, 10k векторов в
одном tx). Ожидаемая дельта: +1 `Store::set` на векторный индекс →
~десятки мкс, в пределах шума для bulk-path.

### 6.4. Существующие тесты (без изменений, должны проходить)

- `crates/shamir-index/src/vector/tests/delta_log_tests.rs` — дельта-механика
  не меняется.
- `crates/shamir-index/src/vector/tests/crash_recovery_tests.rs` —
  snapshot/replay-тесты без изменений.
- `crates/shamir-engine/src/tx/tests/commit_phase5_tests.rs` — проверяет, что
  провал promote НЕ defers tx; с вариантом A graph-half остаётся post-lock, а
  delta-half переносится pre-publish → нужно обновить тестовое ожидание
  (delta-failure → Deferred; graph-failure → NOT Deferred).

---

## 7. Резюме рекомендации

**Вариант A** (delta-append ДО ack, в commit critical section) — рекомендован.

- Закрывает дефект перманентного расхождения индекса с data store.
- Минимальная поверхность изменения (3 файла в engine, 0 в index/wal).
- Идемпотентность гарантирована существующим `replay_delta`.
- Стоимость: +1 `Store::set` на векторный индекс на ack-пути.
- Не отменяет III.5 (graph-promote остаётся post-lock).
- Не требует schema-изменений WAL или manifest.

Вариант B отвергнут: cardinality-эвристика даёт ложные срабатывания на каждой
non-tx таблице, деградируя cold-start до постоянного O(N)-скана.

Вариант C отвергнут: обратно-несовместимое WAL schema-изменение + новое
deadlock-ребро (drainer↔promote) + отмена III.5 — слишком высокая цена для
задачи, которую вариант A решает одним переносом вызова.
