# Оставшиеся оптимизации — пошаговый план

После 33 коммитов оптимизаций (см. `x-speedup-playbook.md`) остались
структурные изменения, упирающиеся в один корневой инвариант:
**`interner.persist()` на критическом пути коммита** держит глобальный
`commit_lock` и блокирует cross-tx параллелизм.

План снимает блокер через изменение носителя истины: WAL-запись
становится самодостаточной для recovery (интернер-дельта едет внутри),
после чего lock сжимается до микросекундного секвенсора и группа коммитов
делит один fsync.

---

## Этап A — WAL v3: интернер-дельта в записи

**Цель**: убрать `interner.persist()` с критического пути. WAL-запись
содержит дельту новых `(table_token, field_name, intern_id)`, recovery
применяет её перед replay ops.

### Шаги

**A1. Собрать дельту в overlay-merge.** `commit_interner_overlay(...)` в
`crates/shamir-tx/src/layered_interner.rs` уже знает, какие пары новые
для base-интернера. Расширить возвращаемое значение: помимо `remap`,
отдавать `delta: Vec<(String, u64)>` — только реально новые ID.

**A2. Расширить `WalEntryV2`.** В `crates/shamir-wal/src/wal_entry_v2.rs`
добавить поле `interner_delta: Vec<(u64, String, u64)>` —
`(table_token, field_name, intern_id)`. Бамп `WAL_V2_VERSION` → 3.
Decode: версия 2 читается по-старому (пустая дельта), версия 3 — с
дельтой. Round-trip тест на оба формата.

**A3. Прокинуть дельту в Phase 4.** `pre_commit.rs` Phase 1: merge выдаёт
дельту → сохранить в `TxContext` (новое поле
`interner_deltas: HashMap<u64, Vec<(String, u64)>, THasher>`) →
`wal_ops_from_tx` / построение WAL-записи включает её.

**A4. Recovery применяет дельту до декодирования.** `recover_inflight_v2`
(в `repo_wal_manager.rs` / `wal_manager.rs`): перед replay ops каждой
записи — `interner.touch_with_id(name, id)` для каждой тройки дельты.
Нужен новый метод интернера «вставить с заданным ID» (idempotent: если
ID уже есть и совпадает — no-op; если конфликт — ошибка recovery).
Тест: write → crash до persist (симулируется пропуском persist) →
recover → записи читаются.

**A5. Убрать `persist().await` из Phase 1.** Заменить на checkpoint-
механизм: счётчик коммитов на таблицу (`AtomicU64`), каждые N коммитов
(tunable, например 64) — `tokio::spawn` фоновый persist; плюс persist
при graceful shutdown и при WAL-truncate (Phase 7 cleanup — перед
удалением WAL-записей интернер обязан быть на диске, иначе дельты
потеряны). **Ключевой инвариант: WAL-запись можно удалять только после
того, как её интернер-дельта персистнута.**

**A6. Гейт + бенчи.** Полный гейт; `wire_pipelining` (sync/n_*) —
ожидание: небольшой выигрыш уже сейчас (один durable write меньше на
коммит), главное — отсутствие регрессий.

### Файлы
`wal_entry_v2.rs`, `repo_wal_manager.rs`, `layered_interner.rs`,
`interner.rs` (+touch_with_id), `pre_commit.rs`, `tx_context.rs`,
`commit_phases.rs` (Phase 7 checkpoint-gate), тесты.

### Риск
Средний. Recovery-инвариант (A5) — главное место для ошибки; покрыть
тестом «commit → no persist → recover → read».

---

## Этап B — сжатие commit_lock до секвенсора

**Цель**: lock держится микросекунды —
`{SSI-валидация, assign_version, record_footprint, publish}`.
Materialize и интернер-merge — вне.

### Шаги

**B1. Вынести Phase 1 (merge+remap) до lock'а.** Merge теперь без
persist (этап A) и CAS-безопасен. Перенести merge + `rewrite_set_inner`
в «pre-lock» секцию `commit_tx_inner`.

**B2. Вынести Phase 2.5/2.6 (unique-locks + recheck).** Это I/O
(`info_store.get`) под глобальным lock'ом. Unique-локи per-table уже
существуют (`uwl_guards`) — глобальный lock для них не нужен; брать их
ДО commit_lock (сортированный порядок сохраняется — дедлоков нет).

**B3. Сжать критическую секцию.** Под lock'ом остаются: Phase 2 (SSI
validate), 2-bis (phantom), Phase 3 (version), Phase 4 (WAL begin —
пока внутри, уйдёт в этапе D), Phase 6 (publish). Phase 5a-5c
(materialize) — уже параллельный — выносится за lock, гейтится
per-table `uwl_guards`.

**B4. Тесты-добивка.** SSI phantom suite, concurrent-commit, конфликтные
сценарии, recovery. Любой недетерминизм — стоп и анализ.

**B5. Бенч.** `wire_pipelining` sync/n_8..n_128 — ожидание: рост с
конкуренцией, особенно на disjoint-table нагрузках (стоит добавить
бенч-вариант two-tables).

### Риск
Высокий (SSI-ordering). Делается только после A. Materialize вне lock'а
меняет момент видимости данных на диске относительно publish —
проверить, что read-path смотрит в MVCC/staging, а не напрямую в Store
(иначе грязные чтения).

---

## Этап C — write_set_keys для cross-tx конфликтов

**Цель**: дешёвая структура для проверки пересечения write-set'ов двух
транзакций (нужно group commit'у).

### Шаги

**C1.** В `TxContext` — метод
`write_set_keys(&self) -> impl Iterator<Item = (u64, &RecordKey)>`
поверх `write_set: HashMap<u64, StagingStore, THasher>`
(StagingStore теперь IndexMap — итерация дешёвая, без snapshot_ops-клона).

**C2.** Хелпер `conflicts_with(&self, other: &TxContext) -> bool` —
построить `HashSet<(u64, &[u8]), THasher>` по меньшему write-set'у,
проверить больший. O(W₁+W₂).

**C3.** Юнит-тесты: пересечение/непересечение, разные таблицы — один
ключ, один стол — разные ключи.

### Риск
Низкий. Маленький, независимый, можно параллельно с B.

---

## Этап D — group commit (leader/follower)

**Цель**: N одновременных коммиттеров → один fsync (через готовый
`begin_many`, commit `f2fb99c`), один проход публикации.

### Шаги

**D1. Pending-очередь на гейте.** В `RepoTxGate`:
`pending: Mutex<Vec<PendingCommit>>` (короткий lock — только push/drain),
где `PendingCommit = { tx-данные после pre-lock фаз,
oneshot::Sender<Result<Version>> }`.

**D2. Leader-ветка.** `try_lock()` на commit_mutex успешен → я leader:
drain pending → для каждого follower'а: cross-follower конфликт через
`conflicts_with` (C) — конфликтующие получают `Err(Conflict)` в
oneshot сразу; принятые валидируются SSI последовательно под lock'ом
(это быстро — merge уже сделан до очереди, этап B).

**D3. Batch WAL.** Все принятые + leader → `begin_many(&entries)` —
один flush (31a, уже в master).

**D4. Версии и публикация.** Последовательные версии в порядке очереди;
materialize всех (параллельный `join_all` уже умеет); publish по
порядку; уведомить oneshot'ы.

**D5. Follower-ветка.** Lock занят → собрать `PendingCommit`, push,
`rx.await`.

**D6. Collect-window (опционально, тюнабл).** Leader перед drain может
подождать 20-100 µs, чтобы собрать больше попутчиков, — выставить через
`shamir-tunables`, по умолчанию 0 (без ожидания).

**D7. Тесты**: конфликт двух follower'ов (один Err), crash между
`begin_many` и publish (recovery реиграет только durable), порядок
версий, abort follower'а не травит batch.

**D8. Бенч**: `wire_pipelining` n_32/n_128 — здесь должен быть главный ×N.

### Риск
Высокий, но после A+B+C он локализован в орекстрации, а не в инвариантах.

---

## Этап E — writev fan-out (независимый, можно в любой момент)

**Цель**: убрать последний memcpy payload'а на подписчика.

### Шаги

**E1.** `PushSink::try_push_event` принимает `(header: &[u8], payload: &Bytes)`
или `&[IoSlice]`.

**E2.** TCP-транспорт: `write_vectored` (tokio `AsyncWriteExt::write_vectored`,
учесть partial writes — цикл).

**E3.** WS-транспорт: tungstenite требует целый Message — здесь либо
оставить склейку (WS медленнее по своей природе), либо собрать через
`BytesMut::chain`. Не блокер: выигрыш забираем на TCP.

**E4.** `subscription_fanout` бенч + e2e.

### Риск
Низко-средний (partial-write цикл — единственное тонкое место).

---

## Этап F — format bump v1 (один раз, перед публикацией)

Собрать в одно протокольное изменение, последним перед релизом:

### Шаги

**F1.** WAL v3 уже включён (этап A) — сюда же framed codec вместо
bincode (zero-copy decode, см. playbook WAL-Target 1/10).

**F2.** Positional msgpack для ответов (`to_vec` вместо `to_vec_named` +
фиксированный порядок полей) — клиенты TS/Rust обновляются синхронно.

**F3.** `sub_id: u64` (intern при subscribe) — упрощает envelope,
открывает full-precompute fan-out (E-плюс).

**F4.** Прогнать все e2e + клиентские SDK, обновить
`docs/client-server-protocol-spec/`.

### Риск
Средний по объёму, низкий по тонкости — механика, не инварианты.

---

## Сводный порядок и зависимости

```
A (WAL v3 delta) ──→ B (shrink lock) ──→ D (group commit)
                            ↑
C (write_set_keys) ─────────┘        E (writev) — параллельно в любой момент
                                     F (format bump) — последним, вбирает A
```

| Этап | Размер  | Риск          | Ожидаемый эффект                                    |
|------|---------|---------------|-----------------------------------------------------|
| A    | средний | средний       | разблокирует всё; −1 durable write/commit           |
| B    | средний | высокий       | lock → микросекунды; disjoint-commits параллельны   |
| C    | малый   | низкий        | prerequisite D                                      |
| D    | средний | высокий       | ×N коммиттеров на fsync — главный приз              |
| E    | малый   | низко-средний | финал подписок                                      |
| F    | средний | низкий        | чистый протокол к релизу                            |

---

## Глубинная идея

Оставшиеся оптимизации — не «ещё циклы /opti», а **одна архитектурная
идея** (WAL как единственный носитель истины при коммите) плюс **одна
транспортная идея** (scatter-gather вместо склейки). Всё остальное —
следствия.
