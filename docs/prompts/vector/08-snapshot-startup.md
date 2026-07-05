בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V2.2 — startup-интеграция снапшота + fallback rebuild

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 2.2 плана `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P2). Предыдущий лист V2.1 (коммит c80d99f9) дал кодек снапшота.

## Зачем

V2.1 дал `dump_snapshot`/`load_snapshot` (кодек), но НИКТО его не зовёт при
старте. Сейчас vector-индекс на открытии всегда делает полный rebuild сканом
(`VectorBackend::rebuild`). Нужно: при открытии сперва ПОПРОБОВАТЬ загрузить
снапшот; получилось → O(load); нет снапшота / битый / чужая версия → fallback
на текущий полный rebuild (+ warn). Плюс инструментация для тестов: счётчик
full-rebuild, чтобы доказать «снапшот использован, скана не было».

## Контекст кода

- `crates/shamir-index/src/vector/snapshot.rs` — `load_snapshot(store, keyspace)
  -> Result<HnswAdapter, SnapshotError>`; `SnapshotError::NotFound` когда
  снапшота нет. Ключ keyspace — сверь, как V2.1 его формирует (какая строка
  идентифицирует индекс: `__vec_idx__<table>__<idx>` или как реально).
- `crates/shamir-index/src/vector/vector_backend.rs` — `VectorBackend`, метод
  `rebuild(source: Arc<dyn Store>)` (~246-271) делает полный скан + upsert_batch.
  Найди, ГДЕ VectorBackend конструируется/инициализируется при открытии таблицы/
  индекса (вероятно в `crates/shamir-engine/src/table/` — index2-загрузка;
  поищи, где vector backend создаётся из descriptor'а при open).
- `crates/shamir-engine/src/table/table_manager.rs` (~load_index2_metadata /
  open-путь) — где index2-бэкенды восстанавливаются на open.

## Задача

1. **Load-on-open с fallback:** в точке инициализации vector-бэкенда при
   открытии — сперва `load_snapshot(info_store, keyspace)`:
   - `Ok(adapter)` → использовать загруженный HnswAdapter (обернуть в
     VectorBackend), full-scan НЕ делать;
   - `Err(NotFound)` → текущий путь: пустой адаптер + `rebuild(source)` полным
     сканом (как сейчас);
   - `Err(Corrupt | VersionMismatch | Io | ...)` → `log::warn!` с причиной +
     fallback на полный rebuild (НЕ падать — снапшот битый/устаревший, данные в
     store целы, rebuild восстановит).
   ВАЖНО: где взять info_store/keyspace в этой точке — сверь по коду (VectorBackend
   уже имеет доступ к source-store для rebuild; info_store для снапшота —
   тот же или отдельный? V2.1 писал в info_store — сверь, как backend его
   получает на open).
2. **Счётчик full-rebuild** (fail-инструментация): `AtomicU64` (глобальный
   per-index или прокидываемый), инкремент при КАЖДОМ полном rebuild сканом.
   Экспонировать геттер для тестов. При успешном load снапшота счётчик НЕ
   растёт.
3. **Dump НЕ триггерим** (это #402: delta-log + когда дампить). Здесь только
   LOAD на старте. Но если тебе нужно, чтобы тест мог СОЗДАТЬ снапшот для
   проверки load-пути — вызывай `dump_snapshot` напрямую в тесте (это ок,
   тест — не production-триггер).

## Тесты (TDD red-first) — fail-инструментация из плана-дока

Раскладка: где живут index2/vector-backend тесты (напр.
`crates/shamir-engine/src/table/tests/` или shamir-index vector tests — выбери
по месту интеграции).
- **restart с валидным снапшотом → rebuild-счётчик == 0:** построить индекс,
  `dump_snapshot`, «переоткрыть» (сконструировать backend заново из того же
  store) → load сработал, счётчик full-rebuild == 0, поиск возвращает те же
  данные.
- **снапшота нет → счётчик == 1:** открыть без снапшота → fallback rebuild,
  счётчик == 1, данные из скана на месте.
- **снапшот от чужого config (dim/metric поменялись) → fallback:** снапшот с
  dim=X, а индекс пере-декларирован dim=Y (или подменить version) → load
  возвращает VersionMismatch/Corrupt → warn + rebuild, счётчик == 1, не паника.

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector --full`
  зелёный + workspace clippy (open-путь тронут → downstream).
- Существующие index2/mvp тесты НЕ должны сломаться (open-путь изменён) —
  прогони `./scripts/test.sh @engine --full` тоже.
- fmt/clippy тронутых крейтов `-- -D warnings`.
- Пиллары: AtomicU64 для счётчика, async load, spawn_blocking внутри
  load_snapshot (уже есть). warn через `log`.
- НЕ грепать/пайпать тесты на лету. Импорты в шапке. НЕ трогать код вне задачи.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits. НЕ удаляй `run.log`; stray-логи в корне —
отметь, НЕ удаляй сам.

## Definition of done

- Load-on-open с 3-веточным fallback (Ok / NotFound→rebuild / Err→warn+rebuild).
- AtomicU64 full-rebuild счётчик + геттер; при load снапшота == 0.
- 3 теста (валидный снапшот→0, нет снапшота→1, чужой config→warn+rebuild→1).
- `./scripts/test.sh @vector @engine --full` + workspace clippy зелёные.
- Финал: тронутые файлы, где точка врезки open-пути, как получен keyspace/
  info_store, вывод гейта, что оставлено на #402 (dump-триггер).
