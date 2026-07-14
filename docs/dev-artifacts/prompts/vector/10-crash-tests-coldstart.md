בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V2.4 — crash-тесты персистентности + cold-start бенч + отчёт (закрывает P2)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 2.4 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (последний лист фазы P2). Предыдущие: V2.1 кодек (c80d99f9), V2.2 load-on-open
> (6596ac24), V2.3 delta-log+фоновый снапшот (a33cc120).

## Зачем

P2 дал персистентность (снапшот+delta+load-on-open+фоновый flip). Этот лист —
жёсткие crash-тесты (снапшот битый/обрезанный → корректный fallback без паники),
e2e-проверка «рестарт восстанавливает recall», cold-start бенч (load vs
rebuild) и итоговый отчёт фазы P2. Плюс закрыть отложенный из V2.3 gap#1.

## Задача

### 1. Crash / corruption тесты (раскладка vector/tests/)
Расширить (или новый файл `crash_recovery_tests.rs`) сценарии, которых ещё нет:
- **truncated chunk** (обрезать байты одного chunk-значения в store) → load →
  `SnapshotError::Corrupt` → в реальном open-пути (`restore_on_open`) fallback
  rebuild, `rebuild_count==1`, без паники, данные из data_store целы.
- **битый манифест** (обрезать/испортить манифест bytes) → load → Corrupt/
  VersionMismatch → fallback rebuild.
- **несовпадение hnsw_rs-версии** (подменить hnsw_rs_version в sidecar) →
  VersionMismatch → fallback rebuild.
  (V2.1 уже покрыл corrupted-chunk-crc / foreign-version на уровне КОДЕКА;
  здесь — на уровне ОТКРЫТИЯ backend'а через restore_on_open, с проверкой
  rebuild_count и целостности данных. Если дублирует — переиспользуй, но
  проверь именно open-путь + fallback + recall.)
- **e2e restart preserves recall**: create → insert 10K → dump_snapshot →
  «переоткрыть» backend (restore_on_open из того же store) → recall@10 на
  наборе запросов совпадает (в пределах HNSW-нойза) с pre-restart.

### 2. Отложенный gap#1 из V2.3 — tx-путь vector-deletes
Сейчас Phase 5d (`commit_phases.rs`) передаёт `deleted = &[]` в
`append_vector_delta` — tx-удаления вектора не попадают в delta-log (delta-
МЕХАНИЗМ `DeltaOp::Delete` работает, но проводки нет). Выбери:
- **(A)** провести реальные tx-delete через staging: при коммите удаления
  записи с vector-индексом — собрать удалённые rid и передать их в
  `append_vector_delta(deleted=...)` (+ убедиться, что delete применяется в
  граф на промоуте). Найди, где в commit-пути известны удалённые rid для
  таблицы с vector-индексом. ЕСЛИ это большой объём — вариант B.
- **(B)** если полноценная проводка велика/рискованна — ЯВНО задокументировать
  как отдельную будущую задачу (комментарий в commit_phases.rs + строка в
  VECTOR_PRODUCTION_EXECUTION.md «tx-path vector deletes — deferred»),
  добавить unit-тест, фиксирующий текущее поведение (delete через delta работает
  когда rid переданы). Обоснуй выбор A/B в финале.

### 3. Cold-start бенч + отчёт (QUICK)
- Бенч (или расширить vector_report / отдельный пример): cold-start ВРЕМЯ
  `load_snapshot` vs полный `rebuild` сканом на 100K (QUICK; 1M — только за
  env, не в дефолте). Меряем wall-time восстановления графа обоими путями.
  Можно как example-бинарь (perimeter-guard блокирует cargo run → билд+прямой
  запуск) ИЛИ criterion-группа с tune_tiered.
- Отчёт `docs/dev-artifacts/benchmarks/vector/<date>-persisted-hnsw.md`: cold-start числа
  (load vs rebuild), сколько сэкономлено, reproducibility key. DoD P2:
  зафиксировать порог «рестарт 100K без full-scan» фактом.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector
  @oracle --full` зелёный (если тронул Phase 5d для gap#1) + workspace clippy.
  Если gap#1 = вариант B (только доки+тест) — @oracle можно не гонять, но
  @vector --full обязателен.
- fmt/clippy тронутых крейтов `-- -D warnings`.
- Бенч — QUICK, CARGO_TARGET_DIR-изоляция.
- НЕ грепать/пайпать тесты на лету. Импорты в шапке. Раскладка tests/.
- stray-логи в корне — отметь, НЕ удаляй сам.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits.

## Definition of done

- Crash/corruption тесты на open-пути (truncated chunk, битый манифест, чужая
  версия → fallback rebuild без паники, данные целы) + e2e restart-preserves-
  recall.
- gap#1 закрыт (A: проводка tx-delete) ИЛИ явно отложен (B: доки+тест+коммент).
- Cold-start бенч (load vs rebuild, 100K) + отчёт persisted-hnsw.md.
- `./scripts/test.sh @vector [@oracle] --full` + workspace clippy зелёные.
- Финал: тронутые файлы, выбор A/B по gap#1 + обоснование, cold-start числа,
  вывод гейта. Фаза P2 закрыта.
