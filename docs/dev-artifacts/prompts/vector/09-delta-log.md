בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V2.3 — delta-log + триггер фонового снапшота (generation flip)

> Ты — суб-агент в репозитории S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db.
> Реализуешь лист 2.3 плана `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`
> (фаза P2). Даёт инкрементальную durability между полными снапшотами +
> механизм создания снапшота в фоне. Предыдущие: V2.1 (кодек, c80d99f9),
> V2.2 (load-on-open, 6596ac24).

## Зачем

V2.2 грузит снапшот на старте, но снапшот статичен — записи ПОСЛЕ последнего
снапшота теряются при рестарте (граф не персистится инкрементально). Нужно:
(1) **delta-log** — после каждого коммита апдейты вектора аппендятся durable
в info_store; при старте после load снапшота дельта доигрывается; (2)
**триггер снапшота** — по порогу изменений фоново создаётся НОВОЕ поколение +
атомарный generation-flip манифеста + prune старого поколения и проигранной
дельты.

## Контекст (что уже есть)

- `crates/shamir-index/src/vector/snapshot.rs` — `dump_snapshot`/`load_snapshot`;
  манифест уже несёт `gen`; `dump_snapshot_with_gen(gen)` — заготовка (V2.1
  всегда gen=0, старые чанки не чистятся). Есть SnapshotError.
- `crates/shamir-index/src/vector/vector_backend.rs` — `restore_on_open`
  (V2.2), `adapter: Arc<ArcSwap<AdapterSlot>>`, `full_rebuild_count`.
- `crates/shamir-engine/src/tx/commit_phases.rs` — Phase 5d `promote_vectors`
  (~261-291) применяет staged-вектора в граф после коммита. Точка, где
  аппендить delta.
- **Образец chunk-persist + HWM**: `crates/shamir-engine/src/table/
  interner_manager.rs` (last_persisted_len / next_chunk_idx / zero-padded).
- `crates/shamir-tunables/src/lib.rs` — const + env-override паттерн.
- Store::transact атомарен.

## Задача

### 1. Delta-log (инкрементальные чанки в info_store)
- Формат: delta-чанк = `Vec<DeltaOp>` где `DeltaOp = Upsert(RecordId, Vec<f32>)
  | Delete(RecordId)`, bincode в MetaEnvelope, zero-padded ключ
  `<keyspace>.delta.NNNNNN` (монотонный индекс, HWM-паттерн InternerManager).
- **Append на промоут**: в Phase 5d (commit_phases.rs) после успешного
  `apply`/promote staged-векторов — аппендить delta-чанк с теми же (rid,vec)/
  delete. Это durable запись ПОСЛЕ применения в память (граф уже обновлён;
  delta нужна для рестарта). Продумай: где VectorBackend/адаптер доступен в
  Phase 5d, как получить keyspace/info_store (аналогично dump-пути).
  ⚠️ §5.6: это НЕ должно блокировать commit-ack надолго — append дельты дёшев
  (один Store::set), но если хочешь фон — обоснуй. Для V2.3 синхронный append
  в Phase 5d приемлем (дёшев), но не тормози commit сверх необходимого.
- **Replay на старте**: в `restore_on_open`/`load_snapshot`-пути — после load
  базового снапшота доиграть все delta-чанки с индексом > зафиксированного в
  манифесте (манифест несёт «delta_applied_upto» или снапшот делался на
  известном delta-index). Применять через `upsert_batch`/delete.

### 2. Триггер фонового снапшота + generation flip
- **Порог**: tunable в shamir-tunables (напр. `VECTOR_SNAPSHOT_DELTA_THRESHOLD
  = 10_000` изменений ИЛИ N дельта-чанков; env-override). Счётчик изменений с
  последнего снапшота (AtomicU64 в backend).
- При достижении порога → **фоновая задача** (`tokio::spawn` + single-flight
  `AtomicBool`, §5.6 неблокирующе): `dump_snapshot_with_gen(gen+1)` из текущего
  состояния адаптера → **generation flip**: записать новый манифест (указывает
  на gen+1, delta reset) ОДНИМ `Store::transact` → **prune**: удалить чанки
  старого gen + проигранные delta-чанки (`remove_many`). Prune идемпотентен
  (краш между flip и prune → orphan'ы зачищаются на следующем снапшоте).
- Load всегда читает манифест → однозначный активный gen; orphan'ы прошлых gen
  не подхватываются.

## Тесты (TDD red-first)

- **delta replay восстанавливает без снапшот-порога**: insert 1k (порог не
  достигнут, снапшота нового нет) → «рестарт» (load снапшот + replay delta) →
  все 1k на месте, поиск корректен.
- **generation flip**: набить изменений > порога → дождаться фонового снапшота
  (или вызвать триггер детерминированно в тесте) → манифест указывает на gen+1,
  старые delta-чанки/старый gen удалены; load читает новый gen.
- **краш-инъекция flip-без-prune**: смоделировать (flip записан, prune нет —
  оставить orphan-чанки в store) → load корректен (манифест→новый gen),
  следующий снапшот зачищает orphan'ы идемпотентно.
- **delete в delta применяется**: удалить rid → delta → replay → удалённый не
  всплывает.
- **staged (незакоммиченная tx) в delta НЕ попадает**: staged-вектор до коммита
  → в delta-log его нет (только на промоуте в Phase 5d).

## Дисциплина + гейт

- Тесты ТОЛЬКО через `./scripts/test.sh`. Гейт: `./scripts/test.sh @vector
  @oracle --full` зелёный + workspace clippy. @oracle = tx+engine (Phase 5d
  тронут).
- fmt/clippy тронутых крейтов `-- -D warnings`.
- Пиллары: AtomicU64/AtomicBool (счётчик/single-flight), tokio::spawn для фона,
  Store::transact для атомарного flip, §5.6 (снапшот не блокирует commit).
- НЕ грепать/пайпать тесты на лету. Импорты в шапке. НЕ трогать код вне задачи.
  stray-логи в корне — отметь, НЕ удаляй сам.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree или index. Only edit
files; the orchestrator commits.

## Definition of done

- delta-log (append в Phase 5d + replay на старте) + триггер фонового снапшота
  (порог tunable, single-flight, generation flip одним transact, prune orphan'ов).
- 5 тестов зелёные. `./scripts/test.sh @vector @oracle --full` + workspace
  clippy зелёные.
- Финал: тронутые файлы, форма delta-чанка/манифеста (delta_applied_upto),
  порог+как считается, как flip атомарен, §5.6-обоснование (снапшот в фоне,
  append дёшев), вывод гейта, что оставлено на #403.
