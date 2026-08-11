# Независимое адверсариальное READONLY-ревью «новой волны» — 2026-08-09 (Claude)

> Область: коммиты `237feda4..cf8393d1` (2026-08-05 … 2026-08-09, ~120 коммитов,
> +15.6k / −1.9k строк кода в `crates/`), с фокусом на зонах, которые НЕ покрыли
> предыдущие ревью (RFC online-index #1054-#1062 и аудит Codex #1063-#1071).
> Режим: readonly, без запуска сборки/тестов/бенчей.

## Резюме и вердикт

**Вердикт: NO-GO.** Основание — одна находка уровня CRITICAL, найденная в этом
проходе, плюс четыре уже заведённых P0 из аудита Codex (#1063-#1071), ни один
из которых не закрыт на HEAD.

Собственная критическая находка: **офлайн-инструмент `shamir-server doctor`
(релиз-блокер P1-1 / #1014) не видит ни одной таблицы, созданной клиентом по
проводу, и завершается кодом 0** — он не выполняет реплей `wire_tables.mpack`,
который делает боевой boot. Это fail-open в последнем средстве проверки
целостности: после аварии оператор получает «No tables found» / exit 0 вместо
диагноза. Дефект не был пойман, потому что весь e2e-набор `doctor` тавтологичен —
четыре теста, каждый ассертит только `result.is_ok()`, а один из них ещё и
фильтрует по репозиторию `default`, которого не существует (боевой называется
`main`). То есть тесты зелены ИМЕННО ПОТОМУ, что инструмент слеп.

Что в волне сделано хорошо и подтверждено чтением: **P0-3a (reader-drain gate,
слайсы #1011/#1037/#1038) закрыт по-настоящему полно.** Я поимённо прошёл все
боевые точки резолва индекса — FK RESTRICT, FK ON UPDATE, FK-действия,
валидатор уникальности, валидатор FK, write-путь `SET`, четыре ветки
`read_exec`, covering-путь, AsOf-seek, планировщик — везде «идёт DROP»
корректно отличается от «совпадений нет», нигде не сворачивается в пустой
результат. Это редкий случай, когда сквозная правка доведена до конца.

Всего находок: 13. CRITICAL — 1, HIGH — 1, MEDIUM — 5, LOW — 5, nit — 1.
Ни одна из них не дублирует списки A (online-index RFC) и B (аудит Codex);
два места пересекаются частично и отмечены отдельным разделом.

## Находки

### F-3 (CRITICAL) — offline `shamir-server doctor` (релиз-блокер P1-1 / #1014) НЕ ВИДИТ ни одной wire-созданной таблицы и выходит с кодом 0

`crates/shamir-server/src/doctor.rs:179-304` открывает данные так:

```rust
let meta_path = config.data_dir.join("shamir_db_meta.redb");
... ShamirDb::init(SystemStoreConfig::Fjall(meta_path.clone())).await ...
for db_name in shamir.list_dbs() { ... for repo_name in db.list_repos() { ...
    for table_name in repo.list_table_names() { ... } } }
```

Но per-table конфигурация в system store **не хранится**. `crates/shamir-server/src/tables_registry.rs:3-10`:

> «`shamir-db`'s system store records databases and repositories but NOT per-table configuration — `RepoInstance::add_table` is an in-memory operation. To make tables created over the wire (`BatchOp::CreateTable`) survive a server restart, this module maintains a small MessagePack file at `<data_dir>/wire_tables.mpack` … The boot path replays the file…»

Реальный boot делает этот реплей явно (`crates/shamir-server/src/server/server_launcher.rs:414-437`):

```rust
let tables_registry = Arc::new(TablesRegistry::open(&config.data_dir)?);
let snap = tables_registry.snapshot();
for (db_name, repo_name, table_name) in snap.iter_entries() {
    if let Some(db) = shamir.get_db(db_name) {
        if !db.has_table(repo_name, table_name) {
            if let Err(e) = db.create_table(repo_name, table_name) { ... }
```

`doctor::run` этот шаг **не выполняет** (и `main.rs:315-324` вызывает `doctor::run(&config,&args)` напрямую, без бута). Следствие: `repo.list_table_names()` возвращает пустой список для всех таблиц, созданных клиентом по проводу — то есть для практически всех пользовательских таблиц. Дальше срабатывает `:297-304`:

```rust
if table_reports.is_empty() {
    ...
    eprintln!("No tables found in the database.");
    return Ok(());          // exit code 0
}
```

Сценарий отказа: оператор после аварийного рестарта запускает `shamir-server doctor --apply` (или ставит `doctor` в CI/health-check, как обещает док-строка `:8` «Exits with non-zero if any table is unhealthy»). Инструмент печатает «No tables found», возвращает 0, и повреждённые индексы остаются нетронутыми, будучи «подтверждены здоровыми». Это ровно fail-open там, где нужен fail-closed, в инструменте, чья единственная задача — быть последней линией проверки целостности.

Минимальное исправление: вызвать тот же реплей `TablesRegistry` в `doctor::run` перед обходом (и, отдельно, вернуть ненулевой код/ошибку, когда явный `--table`/`--repo`/`--db` фильтр не сматчил ничего — сейчас это тоже `Ok(())`).

### F-4 (HIGH) — весь e2e-набор `doctor` тавтологичен: тесты проходят и с работающим doctor, и с полностью «слепым»

`crates/shamir-server/tests/doctor_e2e.rs` — четыре теста, и каждый утверждает ровно одно: `result.is_ok()`.

```rust
async fn doctor_with_table_succeeds() {
    let (_original_temp, backup_temp) = setup_data_with_table().await;
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(result.is_ok(), "doctor should succeed with table: {:?}", result);
}
```

`doctor::run` возвращает `Ok(())` и на ветке «No tables found» (`doctor.rs:297-304`), поэтому этот тест зелёный **именно потому**, что F-3 существует: `setup_data_with_table` создаёт таблицу по проводу (`batch.create_table("test_table", ...)`, `doctor_e2e.rs:104-110`), а doctor её не видит. Ни один тест не проверяет ни `total_tables`, ни JSON-вывод, ни имена индексов.

Прямое доказательство того, что вывод никогда не читали, — `doctor_filter_options_work` (`:139-157`):

```rust
let args = DoctorArgs {
    db: Some("default".to_string()),
    repo: Some("default".to_string()),      // <-- репозиторий называется "main"
    table: Some("test_table".to_string()),
    ...
};
assert!(result.is_ok(), ...);
```

Репозиторий, который создаёт сервер, — `main`, а не `default` (`server_launcher.rs:405-411`: `RepoConfig::new("main", factory)`). Фильтр не может сматчить ничего ни при каких обстоятельствах, но тест «проходит». `doctor_json_output_works` (`:159-172`) не захватывает stdout вообще, поэтому не отличает валидный JSON от его отсутствия.

Регрессионной ценности у набора нет: он не отличает рабочий doctor от заглушки `async fn run(..) -> anyhow::Result<()> { Ok(()) }`.

### F-1 (MEDIUM) — `CursorRegistry::register` теряет shard-lock до CAS: перерасход per-session cap и *перманентный* лок-аут сессии через underflow счётчика

`crates/shamir-server/src/cursor_registry.rs:412-437`:

```rust
let slot = self
    .by_session
    .entry(owner_sid)
    .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
let counter = Arc::clone(slot.value());
drop(slot);                                   // <-- shard write-lock отпущен ЗДЕСЬ

// CAS loop: only admit if the count is still below the cap ...
loop {
    let cur = counter.load(Ordering::Acquire);
    ...
}
```

`free_session_slot` (`:567-572`) при этом рассчитывает на противоположное:

```rust
fn free_session_slot(&self, owner_sid: &[u8; 32]) {
    self.by_session.remove_if(owner_sid, |_, counter| {
        // Pre-decrement value == 1 means the NEW value is 0.
        counter.fetch_sub(1, Ordering::AcqRel) == 1
    });
}
```

и её док-комментарий (`:552-566`) явно утверждает:

> «either `register`'s `entry()` call happens strictly before this `remove_if` … or strictly after … Either interleaving preserves correct cap accounting — a session can never exceed `max_per_session` split across two independently-created counters.»

Аргумент неверен, потому что `register` **отпускает** shard-lock (`drop(slot)`) ДО инкремента.
Атомарным относительно `remove_if` является только `entry()`, а не инкремент.

Сценарий 1 (перерасход cap, 2 потока):
1. `register(sid)` берёт `entry`, клонирует `Arc` A, `drop(slot)` — счётчик A == 1 (сессия уже держит 1 курсор).
2. Параллельный `remove()` того же sid → `free_session_slot` → `fetch_sub` даёт 1 → предикат `true` → запись из `by_session` **удалена**; A остаётся жив только по клону в шаге 1.
3. `register` доводит CAS на осиротевшем A: 0 → 1, кладёт курсор в `open`.
4. Следующий `register(sid)` создаёт **новый** счётчик B == 0.
Итог: у сессии `max_per_session + 1` живых курсоров, каждый из которых держит `SnapshotGuard` и, следовательно, пол MVCC-GC.

Сценарий 2 (хуже — перманентный отказ, 3 участника): курсор X зарегистрирован против осиротевшего A; параллельный `register` создал в мапе B == 0; `remove(X)` попадает в окно между `or_insert_with` и CAS → предикат делает `fetch_sub` на **нуле**: `usize` заворачивается в `usize::MAX`, вернулось `0 != 1`, значит запись НЕ удаляется. Теперь `by_session[sid] == usize::MAX`, и любой последующий `CreateCursor` этой сессии вечно получает `CursorLimitExceeded` — восстановиться можно только рестартом сервера (для удаления нужно ровно `fetch_sub == 1`).

Почему это не ложное срабатывание: `PerIpLimiter::release` (`:219-221`) в этом же файле-соседе оборонительно пишет `prev <= 1`, т.е. риск знак-переполнения авторам известен; здесь взято строгое `== 1`.

Исправление: не отпускать `slot` до успешного CAS (инкремент под тем же shard write-lock, что и `remove_if`), либо перейти на `remove_if` с насыщающим декрементом (`fetch_update(|v| Some(v.saturating_sub(1)))`) плюс отдельная проверка нуля.

### F-5 (MEDIUM) — `doctor --apply` завершает процесс через `std::process::exit(1)` без закрытия хранилища

`crates/shamir-server/src/doctor.rs:349-354`:

```rust
if !healthy {
    std::process::exit(1);
}
```

`process::exit` не запускает деструкторы: `ShamirDb` (и через него открытые Fjall/redb-хендлы и их фоновые flush-задачи) не дропаются. На ветке `--apply` прямо перед этим отработал `table_mgr.repair()` (`:259-261`), который перестраивает индексы. Даже если backend журналирует каждую запись, форсированный выход сразу после массовой перестройки — незакрытые файловые хендлы и потенциально несброшенные буферы; на Windows это ещё и оставленный lock-файл, который поймает следующий запуск.

Сценарий: `doctor --apply` чинит таблицу, финальный `verify` всё ещё видит одну нездоровую таблицу (например, `Failed`-index2, который repair не лечит) → `exit(1)` немедленно, минуя корректное закрытие только что переписанного индекса.

Исправление тривиально: вернуть код выхода наверх (`Ok(ExitCode)`/`anyhow` + `std::process::exit` в `main` после явного дропа), а не выходить из середины async-функции.

### F-6 (MEDIUM) — #1039 (intra-tx unique dedup) даёт ложные `UniqueViolation`: guard-лист не знает про освобождение ключа внутри той же транзакции

`crates/shamir-engine/src/tx/pre_commit.rs:600-618`:

```rust
let mut seen: TFxMap<(u64, bytes::Bytes), RecordId> = TFxMap::default();
for g in &tx.unique_guards {
    let key = (g.table_token, g.index_key.clone());
    if let Some(&prior_owner) = seen.get(&key) {
        if prior_owner != g.owner {
            repo.tx_metrics().on_tx_aborted_unique();
            return Err(TxError::UniqueViolation { key: g.index_key.clone() });
        }
        continue;
    }
    seen.insert(key, g.owner);
    ...
```

Guard пишется ТОЛЬКО для НОВОГО значения и только на insert/update
(`crates/shamir-engine/src/table/table_manager_tx_ops.rs:424, 573, 767, 908, 1015, 1059` — во всех случаях
`for index_key in self.index_manager.unique_keys_for(&new_view)` с `owner: id`).
DELETE не пишет guard вообще, а UPDATE, уводящий запись С ключа, не пишет никакой «release»-маркер. Значит список guard'ов — это набор *претензий*, а не состояние на момент коммита, и сравнение двух претензий на один ключ по владельцу некорректно, если между ними ключ был освобождён.

Сценарий ложного отказа (одна транзакция):
1. `INSERT A {email: "x"}` → guard `(K_x, A)`; stage-time проверка смотрит только в durable store, ключа там нет → ОК.
2. `UPDATE A SET email = "z"` → guard `(K_z, A)`; после этой операции A больше не претендует на `K_x`.
3. `INSERT B {email: "x"}` → guard `(K_x, B)`; stage-time проверка снова смотрит в durable store → пусто → ОК.

Phase 2.6 видит `(K_x, A)` … затем `(K_x, B)` → разные владельцы → **`UniqueViolation`**, хотя итоговое состояние (`K_x → B`, `K_z → A`) полностью корректно. До #1039 эта транзакция коммитилась правильно: durable-проверка для всех трёх ключей давала `NotFound`, а Phase 5c применяла ops по порядку (`set K_x→A`, `remove K_x` + `set K_z→A`, `set K_x→B`).

Тот же ложный отказ даёт последовательность `INSERT A{x}` → `DELETE A` → `INSERT B{x}`.

Это fail-closed (порчи данных нет), поэтому не CRITICAL, но это регрессия: легальная транзакция теперь отклоняется. Ни один из четырёх новых тестов (`base_index_tx_tests.rs`, `intra_tx_*`) не покрывает случай освобождения ключа — все четыре проверяют только «две живые претензии» и «одна и та же запись дважды».

Корректная форма проверки — считать не «видели ли ключ», а «кто владеет ключом на конец транзакции»: прогонять guard'ы вместе со staged delete/old-value-снятиями (либо строить `seen` из `tx.index_write_set`'s `SetPosting`/`RemovePosting` в порядке применения, что и есть авторитетная последовательность Phase 5c).

### F-7 (MEDIUM) — типизированные конструкторы `CreateIndex` молча выбрасывают уже выставленные опции, включая `.unique()`

`crates/shamir-query-builder/src/ddl/create_index.rs:73-81`:

```rust
pub fn hash(self, fields: impl Into<Vec<Vec<String>>>) -> BatchOp {
    let spec = IndexSpec::Hash {
        fields: fields.into(),
        unique: false,            // <-- жёстко false, self.unique не читается
        index_type: None,         // <-- self.index_type игнорируется
    };
    BatchOp::CreateIndex(spec.into_op(self.name, self.table, self.repo, self.if_not_exists))
}
```

Из полей билдера типизированные конструкторы читают ровно четыре — `name`, `table`, `repo`, `if_not_exists` — и молча игнорируют `unique`, `sorted`, `fields`, `index_type`, `fts_*`, `functional_*`, `vector_*`, `include`.

Сценарий отказа (тихая потеря ограничения целостности):

```rust
let op = create_index("idx_email", "users")
    .unique()                                   // разработчик просит UNIQUE
    .hash(vec![vec!["email".to_string()]]);     // получает обычный не-уникальный индекс
```

`unique: false` уезжает на провод, сервер создаёт обычный hash-индекс, приложение считает, что у него есть уникальность — и получает дубликаты. Ошибки нет ни на клиенте, ни на сервере.

То же для `.include(...).sorted_index(...)` (covering-поля молча теряются — индекс перестаёт быть покрывающим, запросы тихо деградируют на полный fetch) и `.vector_dim(768).hash(...)`.

Коварство в том, что часть состояния билдера ПЕРЕЖИВАЕТ вызов (`repo`, `if_not_exists`), а часть — нет. Это делает смешанный стиль внешне рабочим и потому вероятным при миграции со стрингового API на типизированный (#1017). Правило проекта «запросы всегда строятся билдером» здесь работает против себя: билдер принимает вызов, который по смыслу невыразим.

Минимальное исправление: типизированные конструкторы должны либо принимать `self` и возвращать `Result<BatchOp, CreateIndexBuildError>` при непустых kind-полях, либо жить на отдельном стартовом типе (`create_index_typed(name, table)`), у которого стринговых сеттеров просто нет.

### F-9 (MEDIUM) — CLAUDE.md утверждает «ровно одно санкционированное исключение» для блокирующих мьютексов; в коде их как минимум пять живых, каждый со своей встроенной «санкцией»

Нормативный текст `CLAUDE.md` (§ Code ideology):

> «**Sanctioned runtime exception (F-79 / #906):** the sole remaining `std::sync::Mutex` on a runtime struct is `RepoTxGate::pending_commits`…»

Фактически на runtime-структурах живут (все — не тестовые, не `#[cfg(test)]`):

| Место | Поле | Когда берётся |
|---|---|---|
| `crates/shamir-wal/src/segment_set.rs:64` | `SegmentSet::inner: Mutex<Inner>` | дважды за КАЖДЫЙ `append_batch` (`:225`, `:234`) — это путь group-commit'а WAL |
| `crates/shamir-connect/src/server/session.rs:184` | `Session::post_auth_bucket: std::sync::Mutex<PostAuthBucket>` | на КАЖДЫЙ post-auth запрос (token bucket) |
| `crates/shamir-types/src/core/interner/interner.rs:78` | `Interner::reverse_write_lock: std::sync::Mutex<()>` | на каждый first-touch нового имени поля (write-путь) |
| `crates/shamir-engine/src/table/in_flight_create_guard.rs:79` | `InFlightCreateSet::ids: Arc<Mutex<BTreeMap<u64,u32>>>` | CREATE INDEX + чтение из `degraded_index_count()` |
| `crates/shamir-index/src/base_index/index_manager.rs` (`dropping_regular`/`dropping_unique`) | guard-множества | DROP INDEX |

Плюс `parking_lot::Mutex` на runtime-структурах: `SessionStore::cap_lock` (`session.rs:350`), `server/admin.rs:298,393`, `server/audit_chain.rs:131`, `server/bootstrap.rs:49`, `server/durable_counters.rs:54`, `shamir-server/src/audit_appender.rs:190,193,211`, `server_meta.rs:239`, `user_directory.rs:257`, `tables_registry.rs`.

Каждое из этих мест несёт собственный комментарий вида «`std::sync::Mutex` is the sanctioned exception here (CLAUDE.md)» — то есть авторы считали, что ссылаются на существующее разрешение, тогда как нормативный документ разрешает ровно ОДНО другое поле. Это не косметика: сам смысл раздела в CLAUDE.md — чтобы список исключений был обозримым и каждое имело записанную модель контеншена. Сейчас список в документе неверен, а по-настоящему горячие из перечисленных (`SegmentSet::inner` на пути WAL-append, `post_auth_bucket` на каждый запрос) не отличимы от действительно холодных (`InFlightCreateSet`).

Перед релизом нужно либо привести CLAUDE.md к коду (перечислить все пять + parking_lot-семейство с моделью контеншена каждого), либо мигрировать горячие два. Оставлять расхождение нельзя: следующий разработчик, добавляя шестой мьютекс, будет опираться на прецедент, а не на разрешение.

### F-8 (LOW) — «strict-by-default» типизированные конструкторы не проверяют даже пустой список полей

Док-строки утверждают (`create_index.rs:61-63` и аналогично для остальных пяти):

> «This is a **strict-by-default** typed constructor: it produces a valid `BatchOp` directly with no need for `try_build()`.»

Но `IndexSpec::into_op` (`crates/shamir-query-builder/src/ddl/index_spec.rs:111-242`) — чистое разложение варианта в DTO, без единой проверки, а `TryFrom<&CreateIndex> for IndexSpec` (где живут все 12 проверок) на этом пути не вызывается. Поэтому

```rust
create_index("i", "t").hash(vec![]);              // fields: [] — прошло
create_index("i", "t").sorted_index(vec![]);      // fields: [[]] — путь нулевой длины
```

дают валидный с точки зрения типов `BatchOp`, который `try_build()` отверг бы как `CreateIndexBuildError::EmptyFields`. Утверждение «valid ... with no need for try_build()» верно только для комбинаций kind-полей, но не для содержимого. То же в TS (`crates/shamir-client-ts/src/core/builders/ddl.ts`, `hashIndex`/`uniqueIndex`/`sortedIndex` — ни одной проверки на `fields.length`).

### F-2 (LOW) — `PerIpLimiter::try_acquire` держит shard write-lock весь CAS-цикл вопреки собственной документации

`crates/shamir-server/src/conn_limiter.rs:146-189`:

```rust
/// The counter lives in an `AtomicUsize` inside the map value, so the
/// CAS retry loop here does NOT hold the DashMap shard lock — it only
/// holds it transiently inside `entry().or_insert_with()` ...
let entry = self.counts.entry(ip).or_insert_with(|| AtomicUsize::new(0));
let counter = entry.value();
// CAS loop ...
```

`DashMap::entry(...).or_insert_with(...)` возвращает `RefMut`, который **владеет** write-guard шарда; `counter` заимствован из него, поэтому guard живёт до конца функции — весь CAS-цикл и конструирование `PerIpGuard` идут под shard write-lock. Практический эффект мал (цикл короткий, коллизии по IP редки), но документированный инвариант («лок только на резолв слота») в коде не выполняется, и это единственное место, где заявленный lock-free характер accept-пути расходится с реальностью. Сравните с `CursorRegistry::register`, где `drop(slot)` сделан явно — здесь его нет.
Фикс — `let counter = Arc::clone(...)`/`drop(entry)` перед циклом (и тогда см. F-1: делать это осознанно, а не копируя баг).

### F-10 (LOW, доказательство некорректно, не код) — доказательство livelock-freedom в `ReaderDrainGate` неверно

`crates/shamir-index/src/reader_drain_gate.rs:48-52`:

> «Livelock-freedom … : once `dropping` is `true`, no reader can proceed past its own check without immediately backing off, so `in_flight` is **monotonically non-increasing** from that point — the drain wait is guaranteed to terminate.»

Это неверно по конструкции самого `enter()` (`:121-127`): читатель делает `in_flight.fetch_add(1)` **до** проверки флага и `fetch_sub` только после. Значит после подъёма флага `in_flight` НЕ монотонно невозрастающий — каждый новый читатель кратковременно поднимает его на 1. `wait_for_drain` (`:215`) сэмплирует `in_flight != 0` в цикле с `yield_now`, поэтому под непрерывным потоком читателей он может систематически ловить транзиентные единицы.

Практически это не livelock (окно между `fetch_add` и `fetch_sub` — единицы наносекунд против микросекунд между сэмплами), поэтому severity LOW и это скорее дефект доказательства, чем дефект кода. Но доказательство в этом файле — единственное обоснование безусловно неограниченного ожидания (`:200-208` «deliberately unbounded»), и оно опирается на ложное утверждение. Либо исправить формулировку (свойство даёт вероятностное завершение, а не гарантированное), либо разделить счётчики («вошедшие до флага» и «отскочившие»), и тогда утверждение станет истинным.


### F-11 (LOW) — `DropIndexOp::unique` мёртв для резолвинга, но всё ещё входит в HMAC-подпись

После #1025 семейство индекса резолвится из каталога, а не из клиентского флага. Док-строка билдера прямо это фиксирует (`crates/shamir-query-builder/src/ddl/drop_index.rs:29-35`):

> «This field is now informational-only and used only for HMAC canonical input generation … Setting this incorrectly does not affect which index is dropped, **only changes the bytes signed into the HMAC**.»

Итог: одна и та же логическая операция «DROP INDEX X ON T» имеет ДВЕ разные валидные HMAC-подписи в зависимости от флага, который больше ничего не значит. Сценарий отказа: оператор подписал каноническую строку с `unique=false`, клиентская обёртка (или TS-билдер, где значение по умолчанию другое) выставила `unique=true` — деструктивная операция отвергается с несовпадением HMAC, и причина не видна ни в одном сообщении об ошибке. Поле стоит убрать из канонического входа HMAC и пометить `#[deprecated]` до релиза, пока формат не заморожен.

Смежное: док-комментарий `DropIndexOp::if_exists` (`crates/shamir-query-types/src/admin/types/index_ops.rs:131-144`) сам признаёт, что удаление НЕсуществующего индекса на существующей таблице — «always a silent no-op … regardless of this flag». То есть для самого индекса `if_exists` не значит ничего: опечатка в имени индекса всегда «успешна». Для DDL-поверхности это асимметрия с `DROP TABLE` и потенциальный тихий no-op в миграционных скриптах.

### F-12 (LOW, производительность) — `SortedIndexManager::lookup_range` строит `BTreeSet` поэлементно на каждом range-скане, хотя в соседнем семействе эта же оптимизация уже сделана

`crates/shamir-index/src/base_index/sorted_index_manager.rs:1949-1958`:

```rust
let mut out: BTreeSet<RecordId> = BTreeSet::new();
while let Some(batch) = stream.next().await {
    for (k, _) in batch? {
        if let Some(id) = decode_record_id_suffix(k.as_ref()) {
            out.insert(id);
        }
    }
}
```

и сразу за этим потребитель делает ВТОРОЙ полный проход (`crates/shamir-engine/src/table/read_index_scan.rs:255`):

```rust
let id_vec: Vec<RecordId> = record_ids.iter().copied().collect();
```

То есть на диапазон в N постингов: N аллокаций узлов B-дерева + N ребалансировок + обход дерева + аллокация Vec. Ровно эту схему в hash-семействе уже устранили (см. док-комментарий `IndexManager::lookup_by_index`, `index_manager.rs:2283-2300`: «`BTreeSet::insert` на каждый элемент был чистыми накладными расходами (аллокация узла + ребалансировка)» → `Vec` + `Arc<[RecordId]>`), а на sorted-семейство перенести забыли.

Здесь `BTreeSet` всё же несёт смысл (сортировка по `RecordId` перед `get_many`, что даёт локальность в сторе — в отличие от hash-пути, где скан уже отсортирован по id). Но эквивалент — `Vec::push` + один `sort_unstable` + `dedup`: тот же выход, один непрерывный буфер, без per-node аллокаций. Это стандартно даёт 2–5× на построении и лучше ложится в кэш при последующем обходе. Измерять: `crates/shamir-engine/benches` — range-скан на 10k/100k строк.

### F-13 (nit) — 133 конструкции `QueryResult { .., ..Default::default() }` в `shamir-engine` отключают проверку исчерпывающей инициализации

`#1015` добавил в `QueryResult` поля `op_id` / `ddl_status` (`crates/shamir-query-types/src/read/query_result.rs:190,201`), и вместо явного `op_id: None` во все литералы был добавлен `..Default::default()` (напр. `crates/shamir-engine/src/table/read_index_scan.rs:113`+, `read_exec.rs:317,430,462,606,675,776,1303`, …). Сейчас это безобидно, но компилятор больше не заставит автора нового поля пройти по всем 133 местам. Следующее поле, которое ОБЯЗАНО заполняться на части путей, молча получит `Default` везде. Дешёвая профилактика — явные `op_id: None, ddl_status: None` (или хелпер-конструктор `QueryResult::rows(...)`), а `..Default::default()` оставить только там, где дефолт действительно семантичен.

### Пересечения с уже известным (не переоткрываю, отмечаю совпадение)

- `SelectItem::Function` без алиаса получает ключ `name` (`select_projection.rs:112`) — ровно тот же тихий last-write-wins, что Codex нашёл для `SelectItem::Expression`'s `"expr"` (`:131`): `SELECT upper(a), upper(b)` вернёт одну колонку `upper`. Один корень, одно исправление (обязательный алиас при неоднозначности либо суффиксация ключа).
- Механика `enter()`/`Ok(None)`-fallback для reader-drain gate проверена мной по всем 12 боевым call-site'ам (`fk_restrict.rs:333`, `fk_on_update.rs:899,1109`, `fk_actions.rs:1300`, `validator_db.rs:235,364`, `write_helpers.rs:397,463`, `read_exec.rs:382,729,817,850,881,911,1766`, `read_index_scan.rs:113`, `read_asof_seek.rs:235`, `read_planner.rs:52,83,110`) — везде корректный fall-back, нигде `None` не сворачивается в «нет строк». Это единственная часть волны, где закрытие проведено действительно полно; дефектов не нашёл.

## Ответы на 6 вопросов

### 1. Всё ли сделано верно?

В основном — да, и качество проработки P0-3a (reader-drain gate, слайсы #1011/#1037/#1038) заметно выше среднего: гейт заведён во ВСЕ боевые read-пути всех четырёх семейств, и все потребители честно откатываются на полный скан, а не трактуют «занято» как «пусто». Проверил поимённо — фолс-негативов там нет.

Реальные дефекты корректности, найденные в этом проходе:

* **F-3 (CRITICAL)** — `doctor` не видит wire-созданные таблицы и рапортует «здоров» (fail-open в инструменте проверки целостности).
* **F-1 (MEDIUM)** — гонка учёта курсоров на сессию: превышение cap и, в трёхстороннем сценарии, вечный `CursorLimitExceeded` из-за underflow `usize`.
* **F-6 (MEDIUM)** — #1039 отклоняет легальные транзакции (guard-лист не знает про освобождение unique-ключа внутри той же tx).
* **F-7 (MEDIUM)** — типизированный `CreateIndex` молча теряет `.unique()`: приложение думает, что у него есть ограничение целостности, а его нет.

Тавтологичные тесты: **F-4** — весь `doctor_e2e.rs` (4 теста, каждый ассертит только `is_ok()`; один из них к тому же фильтрует по несуществующему репозиторию `default` вместо `main`). Отдельно отмечу, что четыре новых теста #1039 (`intra_tx_*`) проверяют только положительный случай коллизии и не покрывают освобождение ключа — из-за чего F-6 и проехал.

Fail-open там, где нужен fail-closed: F-3 (doctor), и мягче — `extract_tls_exporter(&tls).unwrap_or([0u8; 32])` в `server_launcher.rs:1073,1159`. Последнее **не** находка: клиент падает жёстко (`client.rs:391` `.ok_or(Handshake)`), а на TLS 1.3 exporter всегда доступен после успешного `accept()`, поэтому нулевое значение приводит к отказу аутентификации, а не к её обходу. Но как стиль это подмена «невозможного» на «константу известную атакующему» — я бы заменил на явный `return`.

Fail-closed там, где не нужен: F-6.

### 2. Что ещё нужно сделать перед релизом (блокеры)

1. **F-3 + F-4** — починить `doctor` (реплей `TablesRegistry`) и переписать его e2e так, чтобы они падали на «слепом» doctor: ассертить `total_tables`, имена индексов, парсить JSON, проверять ненулевой exit на заведомо повреждённой таблице. Сейчас релиз-блокер P1-1 закрыт формально, а не фактически.
2. **F-1** — гонка per-session cap: инкремент под тем же shard-lock + насыщающий декремент. Вечный лок-аут сессии — это доступность, а не удобство.
3. **F-7** — либо `Result`-возврат из типизированных конструкторов, либо отдельный стартовый тип. Тихая потеря `UNIQUE` — единственный из найденных дефектов, который портит данные, а не отказывает.
4. **F-6** — вернуть легальные транзакции; в тесте зафиксировать сценарий «insert → update-off-key → insert» и «insert → delete → insert».

**Заявлено в документации, но не соответствует коду:**

* `CLAUDE.md` («ровно одно санкционированное `std::sync::Mutex`») — **F-9**, в коде их минимум пять плюс семейство `parking_lot`.
* `crates/shamir-server/src/doctor.rs:8` («Exits with non-zero if any table is unhealthy») — не выполняется для реальных таблиц (**F-3**).
* «strict-by-default … produces a valid `BatchOp` … with no need for `try_build()`» (`create_index.rs:61-63` и ×5) — **F-8**, пустые `fields` не проверяются.
* Док-комментарий `CursorRegistry::free_session_slot` («a session can never exceed `max_per_session`») — **F-1**, может.
* Док-комментарий `PerIpLimiter::try_acquire` («CAS retry loop … does NOT hold the DashMap shard lock») — **F-2**, держит.
* Доказательство livelock-freedom в `reader_drain_gate.rs:48-52` — **F-10**, неверно.
* `docs/dev-artifacts/research/completeness-ddl.md:35` («Drops have **no** `if_exists`») — устарело (в §2 того же файла G2 помечен закрытым, но вводная таблица не поправлена); при этом для DROP INDEX флаг фактически ничего не меняет (**F-11**).
* HNSW recall 0.90/10K в `docs/guide-docs/guide/06-search.md` против floor 0.60 и N=3000 в тесте — известно (список B), не переоткрываю.

Не блокеры, но перед релизом стоит закрыть: F-5 (`process::exit` из середины async), F-11 (`unique` в HMAC — формат ещё не заморожен, потом будет ломающим).

### 3. Где код остался неоптимальным

* **F-12** — `BTreeSet` поэлементно на sorted-range-скане (в hash-семействе то же уже убрали). Самый конкретный из найденных: N аллокаций узлов + лишний полный проход на каждый range-запрос.
* **`IndexRegistry::lease_by_field_and_kind` (`registry.rs:608-647`) — O(N) по всем index2-бэкендам на каждый резолв.** Планировщик зовёт его до трёх раз на запрос (`read_planner.rs:51,82,107` — fts/vector/functional) плюс `read_exec.rs:1760`, и каждый вызов делает полный `iter_async` по `by_id` со сравнением `desc.paths[0] == field_path` (сравнение `Vec<u64>`). N — число index2-индексов таблицы, так что абсолютная величина мала; но это ровно тот «скрытый O(N) в хелпере», который идеология запрещает, и лечится вторичной мапой `(first_field, kind) → id`, поддерживаемой в `insert`/`remove_by_id` (обе уже под `ddl_admission`, так что дополнительной синхронизации не нужно).
* **`AuditChainWriter::append` (`crates/shamir-connect/src/server/audit_chain.rs:427-433`) — синхронный `write_all` + `sync_all` каждые 1000 записей прямо в async-задаче соединения.** В batched-режиме (который и включён в проде, `server_launcher.rs:154`) обычная запись дешёвая, но `checkpoint()` вызывает `flush_buffer()` → `write_all` + `sync_all` + запись в fjall, всё под `parking_lot::Mutex<File>`, без `spawn_blocking`. Это блокировка воркера tokio на длительность fsync раз в 1000 аудит-событий — хвостовая латентность p99.9 на пути аутентификации. Проверил специально: `open_strict` в проде НЕ используется (иначе fsync был бы на каждое событие) — поэтому severity низкая, но пункт настоящий.
* **F-2** — shard write-lock DashMap удерживается на весь CAS-цикл в per-IP лимитере (accept-путь).
* **F-13** — не производительность, а эрозия проверок компилятора, но лечится в том же классе работ.

### 4. Как можно развить DDL и OQL

Мёртвые/заглушечные поверхности контракта, найденные лично:

* `DropIndexOp::unique` — резолвинг из каталога сделан (#1025), поле осталось и влияет ТОЛЬКО на HMAC (**F-11**). Кандидат №1 на удаление до заморозки формата.
* `DropIndexOp::if_exists` — для самого индекса семантики не имеет (всегда silent no-op). Либо сделать `if_exists: false` строгим («индекс не найден» → ошибка), либо убрать флаг и задокументировать безусловную идемпотентность. Сейчас контракт обещает выбор, которого нет.
* Асимметрия семейств, видимая в билдере: `include` (covering) существует только для sorted; для index2-btree — нет. `unique` — только для hash. `if_not_exists` есть у CREATE INDEX всех семейств, а вот путь DROP различает семейства только внутри сервера. Естественный следующий шаг — единый typed `IndexKind` на проводе вместо связки `unique: bool` + `sorted: bool` + `index_type: Option<String>` (сейчас три поля кодируют одно перечисление, и именно из этого выросли #1025, #990, #1029, #1017 — четыре задачи вокруг одной и той же переусложнённой репрезентации).
* DDL op-status: `GetDdlOpStatus` есть, но (список B) `InProgress` в проде не пишется, а `DDL_OP_LOG_CAP` мёртв. Пока это так, статус-контракт — двухзначный (`Succeeded`/`Failed`), а объявлен трёхзначным. Развитие — довести до реального асинхронного DDL (online CREATE INDEX, RFC #1018), где `InProgress` + прогресс-процент имеют смысл; сейчас поверхность опережает движок.
* OQL: по `completeness-oql.md` живые кандидаты — M3 (FTS ranking/score/highlight), M6 (conditional MERGE), остаток M2 (generated columns). Добавлю от себя, из прочитанного: `SelectItem::Expression` (#1024) реализован через переиспользование `FilterValue`, но без обязательного алиаса — значит, выражения нельзя надёжно проецировать более одного на запрос (**совпадение с B**). Прежде чем расширять выражения (CASE, арифметика по нескольким полям), нужно закрыть именование результата, иначе каждая новая возможность будет множить тихие коллизии ключей.

### 5. Нужно ли расширять Query Builders

Да, но не «вширь» — прежде всего нужно закрыть три дыры в уже существующем:

1. **F-7** — типизированный `CreateIndex` принимает невыразимые комбинации и молча их теряет. Пока это так, правило «всегда билдер» даёт ложное чувство безопасности: билдер собрал запрос, но не тот.
2. **F-8** — «strict-by-default» конструкторы не валидируют содержимое (пустые поля). Валидация должна быть на ЕДИНОМ пути (`TryFrom<&CreateIndex> for IndexSpec`), а типизированные конструкторы — вызывать его, а не обходить.
3. Паритет Rust ↔ TS: в TS типизированные конструкторы — свободные функции (`hashIndex`, `uniqueIndex`, …), в Rust — методы на билдере, из-за чего смешивание возможно только в Rust. Форма API должна совпадать, иначе кросс-язычные фикстуры (`create_index_matrix.json`) проверяют байты, но не эргономику.

Где билдер реально не покрывает нужды (из прочитанного): нет билдера для `GetDdlOpStatus`-опроса (клиент собирает `DbRequest` напрямую — `crates/shamir-db/src/shamir_db/shamir_db/core.rs:752`), и нет билдер-поверхности для курсоров (`CreateCursor`/`FetchNext`/`CancelCursor` — они живут на уровне `Client`, а не батча). Это не нарушение правила (там не «строится запрос»), но операторские сценарии («открыть курсор по построенному билдером `ReadQuery`») сейчас сшиваются вручную.

### 6. Где нужно улучшить производительность

По убыванию ожидаемого эффекта, с измеримой формулировкой:

1. **F-12, sorted range-скан.** Метрика: время `read_sorted_index_scan` на диапазон 10k/100k строк. Ожидание: −30…−60 % на построении набора id (устранение N аллокаций узлов) плюс −1 полный проход. Прецедент с числами уже есть в репозитории (audit 3.2/#499 для hash-семейства).
2. **`lease_by_field_and_kind` O(N).** Метрика: `read_planner` на таблице с 8–16 index2-бэкендами, запрос с fts+vector-фильтрами (3 резолва на запрос). Ожидание: O(N) → O(1) на резолв; абсолютный выигрыш мал при N ≈ 1–3, поэтому это работа «на вырост» (таблицы с десятками index2), а не срочная.
3. **Аудит-checkpoint без `spawn_blocking`.** Метрика: p99.9 латентности handshake под нагрузкой аутентификации. Ожидание: убрать периодический выброс длиной в fsync с воркера.
4. **`SegmentSet::inner` (F-9) на пути WAL-append.** Метрика: `crates/shamir-server/benches/db_handler_rps.rs` / `tx_pipeline` при высокой конкуренции писателей. Здесь я НЕ утверждаю, что мьютекс — узкое место (модель single-writer-leader делает контеншен маловероятным); утверждаю только, что он не соответствует заявленному в CLAUDE.md инварианту и что «CAPSTONE replaces this with a single-writer task» из док-комментария до релиза не сделан.

Отдельно честно: **измеримых оснований называть что-либо ещё «медленным» я не нашёл.** Perf gate удалён из релизного DAG (#1020, коммит `7a0e9b64`), baseline'а на текущей волне нет, поэтому любые дальнейшие утверждения о производительности были бы «выглядит медленно» — а это ровно то, чего просили не делать. Восстановление baseline-замера (хотя бы разовый прогон `bench_scale_tool` по `tx_pipeline` + `db_handler_rps` + range-скан на текущем HEAD) — это и есть предрелизная работа по вопросу 6.

## Приложение: прочитанные файлы (аудит-след)

Полностью прочитанные файлы:

- `crates/shamir-index/src/reader_drain_gate.rs`
- `crates/shamir-index/src/registry.rs`
- `crates/shamir-server/src/conn_limiter.rs`
- `crates/shamir-server/src/cursor_registry.rs`
- `crates/shamir-server/src/tls.rs`
- `crates/shamir-server/src/doctor.rs`
- `crates/shamir-server/tests/doctor_e2e.rs`
- `crates/shamir-transport-tcp/src/tls.rs`
- `crates/shamir-engine/src/table/in_flight_create_guard.rs`
- `crates/shamir-query-builder/src/ddl/create_index.rs`
- `crates/shamir-query-builder/src/ddl/index_spec.rs`

Прочитанные фрагменты (с указанием диапазонов):

- `crates/shamir-index/src/base_index/index_manager.rs` — перечень методов; `2280-2400` (`lookup_by_index` + gate); `1743-1890` (drop-путь, по grep-контексту)
- `crates/shamir-index/src/base_index/index_manager_unique.rs` — `1-140`
- `crates/shamir-index/src/base_index/sorted_index_manager.rs` — `1925-2075`, `2170-2200`, `2290-2310`, `2505-2530`
- `crates/shamir-engine/src/tx/pre_commit.rs` — `583-680` (Phase 2.6), `700-760`
- `crates/shamir-engine/src/table/table_manager_tx_ops.rs` — `990-1075` (запись `UniqueGuard` на update)
- `crates/shamir-engine/src/table/read_index_scan.rs` — полный diff волны; `236-260`, `430-470`, `565-600`
- `crates/shamir-engine/src/table/read_exec.rs` — `795-930` (диспетчер sorted-путей + обработка `IndexDrainInProgress`)
- `crates/shamir-engine/src/table/write_helpers.rs` — `380-480`
- `crates/shamir-engine/src/query/read/select_projection.rs` — `1-180`
- `crates/shamir-engine/src/query/batch/fk_restrict.rs` — `320-360`
- `crates/shamir-engine/src/query/batch/fk_actions.rs` — `1290-1320`
- `crates/shamir-engine/src/query/batch/fk_on_update.rs` — `890-915`, `1100-1125`
- `crates/shamir-engine/src/validator/validator_db.rs` — `225-265`, `355-395`
- `crates/shamir-engine/src/tx/tests/base_index_tx_tests.rs` — новые `intra_tx_*` тесты (через `git show c9e0626b`)
- `crates/shamir-server/src/server/server_launcher.rs` — `140-175` (аудит-аппендер), `345-500` (init + реплей `TablesRegistry`), `1040-1180` (accept-циклы TCP/WSS, exporter)
- `crates/shamir-server/src/tables_registry.rs` — `1-80`
- `crates/shamir-server/src/main.rs` — блок подкоманды `doctor` (`~310-326`)
- `crates/shamir-server/src/audit_appender.rs` — `180-290`, `655-700`
- `crates/shamir-connect/src/server/resume.rs` — `230-470` (`process_resume`, `issue_initial_ticket`)
- `crates/shamir-connect/src/server/session.rs` — `175-200`, `340-360`
- `crates/shamir-connect/src/server/audit_chain.rs` — `395-435`
- `crates/shamir-client/src/client.rs` — `380-400` (connect + exporter), `595-615` (resume + exporter)
- `crates/shamir-wal/src/segment_set.rs` — `40-70`, `120-200` (`open`/self-heal/сайдкары), `330-520` (`seal_and_rotate`, `replay`, `truncate_below`)
- `crates/shamir-types/src/core/interner/interner.rs` — `60-110`
- `crates/shamir-query-types/src/read/query_result.rs` — `120-210`
- `crates/shamir-query-types/src/admin/types/index_ops.rs` — `122-145` (`DropIndexOp`)
- `crates/shamir-query-builder/src/ddl/drop_index.rs` — `1-60`
- `crates/shamir-client-ts/src/core/builders/ddl.ts` — новый блок типизированных конструкторов (через diff волны)
- `crates/shamir-wasm-host/src/` — обзорно (перечень модулей, grep по fuel/limits; глубокого чтения не проводил)

Документы:

- `CLAUDE.md` (нормативные разделы про идеологию и мьютексы)
- `docs/dev-artifacts/research/completeness-ddl.md` — вводная часть + список gap'ов
- `docs/dev-artifacts/research/completeness-oql.md` — вводная часть + статус актуализации

Git-инспекция (только чтение): `git log` последних 120 коммитов,
`git diff --stat 237feda4..HEAD` (полный, по крейтам),
`git diff 237feda4..HEAD -- crates/shamir-engine/src/table/read_index_scan.rs`,
`git diff 237feda4..HEAD -- crates/shamir-client-ts/src/core/builders/ddl.ts`,
`git show c9e0626b` (#1039).

Сборка, тесты и бенчи не запускались — все утверждения получены чтением
исходников и истории. Там, где для подтверждения нужен прогон (пункт 6,
восстановление baseline), это явно названо работой, а не выводом.
