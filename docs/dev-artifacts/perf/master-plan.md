# Master Plan — WAL-консолидация → durability → speedup

Единая точка «где мы и что дальше». Опирается на:
`durability-model.md` (модель долговечности), `wal-refactor.md` (один файловый
движок), `AUDIT_all_crates_speedup.md` (бэклог ускорений).

Дисциплина на всю дорогу:
1. WAL — единственный источник истины; data-store/индексы/маркеры — производный кэш.
2. Каждый шаг: агент → зеро-trust проверка диффа → перегон gate своими руками → атомарный коммит.
3. Строго по очереди через агентов — нулевой конфликт по файлам.
4. /opti-фаза — только measure-first (impact аудита = гипотезы, не замеры).
5. Тесты только через `./scripts/test.sh`; gate = fmt + clippy --workspace --all-targets + scope.

---

## ✅ Сделано (закоммичено, проверено)

- **Version Oracle** — lock-free commit.
- **Durability-фундамент B**: `WalSegment` (файловый WAL, write/fsync split),
  `WalGroupCommit` (двухуровневый group-commit), `WalSink {File, Noop}`.
- **Tx-путь на файловом WAL** (F0–F3): default=ур.2 (Buffered), synced=ур.3,
  recovery через `replay()`, crash-тест.
- **Тест-периметр**: perimeter-guard, баннер, пин nextest.
- **Ревью-правки R1–R8**: sync_all, CRC-warn, circuit-breaker, удалён мёртвый
  GroupFsync, inert-I/O убран, и пр.
- **clippy-hardening**: `disallowed_methods` (Fx-only) + `absolute_paths`
  (guardrail) как workspace-линты во всех 22 крейтах.

### Переходное состояние (леса помечены задачами)
- tx → файловый WAL; **non-tx ещё на KV V1** (F4).
- default обещает bounded-window, но **фонового fsync нет** (RF1).
- сегмент растёт **без усечения** (F6); KV-сток **не удалён** (F5).
- `wire_pipelining` (in-memory) F3 НЕ трогал → его регресс — зона D2.

---

## Очередь (строго по порядку)

### Фаза 1 — WAL review-fixes + быстрый win
| # | Шаг | Файлы | Риск | Gate |
|---|---|---|---|---|
| 1 | **RF1** фоновый fsync-таймер (рев.🔴#2): `tokio::time::interval`→`sync_now`, weak-ref lifecycle, чистый shutdown; замыкает default-контракт | `wal_group_commit.rs`, `repo_instance.rs` | средний | @storage @oracle |
| 2 | **H3** (аудит, быстрый win): `append_batch` 3N→1 — коалесить кадры в один `Vec<u8>`, один `write_all` | `wal_segment.rs:79` | низкий | @storage |
| 3 | **RF2** убрать мёртвый txn_id-floor + поправить CRIT-B коммент; опц. floor gate `next_tx_id` из `recover()` | `repo_instance.rs:452`, `repo_wal_manager.rs` | средний | @oracle @e2e |
| 4 | **RF3** гигиена: f3-тест→`process_crash` (+опц. fsync-kill), коммент к `recovery:273` no-op, тест replay-идемпотентности | `recovery_tests.rs`, `recovery.rs` | низкий | @oracle |

### Фаза 2 — достроить консолидацию WAL
| # | Шаг | Что | Риск |
|---|---|---|---|
| 5 | **F4** cutover non-tx → **вариант (a)**: `table_manager_crud`/`write_exec` эмитят полный `WalEntryV2` в repo-WAL; per-table `WalManager` отвязан от write-пути | высокий |
| 6 | **F5** удалить KV-WAL: KV-методы `RepoWalManager` + `WalManager` V1 + V1-кодек + magic-sniff + `info_store_for_test`; **решить in-memory**: `WalSink::Noop`+disk-тесты ЛИБО `WalSink::Mem` | средний |
| 7 | **F6** truncation/checkpoint после durable-materialize + streaming replay (LOW: `read_to_end`→стрим) + `encode` thread-local scratch | средний |

### Фаза 3 — фоновый батчер + полнота крах-тестов
| # | Шаг | Поглощает из аудита |
|---|---|---|
| 8 | **D2** `run_leader` → фоновый materialize+truncation-батчер (lock-free, вне ack-пути) | M3 (O(M²)→O(M) конфликт), M10 (lock-free append-очередь вместо `Mutex<Vec>`), M12 (`begin_grouped_many` батч), LOW group-commit клоны |
| 9 | **D4** единые crash-injection: torn-tail, крах WAL-durable↔materialize, materialize↔truncate; idempotent-replay над 20 прогонами | — |

### Фаза 4 — флака (независимо, можно вклинить)
| # | Шаг |
|---|---|
| 10 | **#344** `reaper_task_reaps_past_deadline_tx` → детерминированный сигнал вместо реального дедлайна |

### Фаза 5 — /opti-кампания по аудиту (после WAL, measure-first)
Снять **чистый baseline** в стабильной точке (пост-F6/D2) — он якорь кампании. Затем:
| # | Шаг | Приз |
|---|---|---|
| 11 | **H1+H2** Interner: `UserKey(Arc<str>)` + `boxcar::Vec`/сегментированный reverse-vec | доминанта bulk-insert/cold-start |
| 12 | **H4** `delete_many_returning_version` батч | закрыть асимметрию INSERT/UPDATE/DELETE |
| 13 | **H5** `iter_stream` lazy-batch вместо `collect` всего датасета | память full-scan |
| 14 | **H6** BruteForce COW snapshot (`Arc<[Vec<f32>]>`) | vector-write |
| 15 | хвост MED/LOW по таблице ROI аудита | — |

---

## Развилка F4 (за пользователем)

- **(a) [план]** объединить *сток* (non-tx эмитит V2 в один WAL), шов tx/non-tx
  оставить — ниже риск, WAL ещё достраивается.
- **(b) поздний кейстоун U1** «всё есть транзакция» (non-tx = неявная одно-оп tx):
  один путь записи, одна модель долговечности, одно восстановление — предельное
  единство. После того, как один WAL докажет себя.

---

## Бенчи
Не сейчас (переходное состояние: in-memory F3 не трогал; disk-путь недо-оптимизирован
— H3/RF1/begin_grouped_many ещё впереди). Чистый baseline — пост-F6/D2, как якорь
Фазы 5.
