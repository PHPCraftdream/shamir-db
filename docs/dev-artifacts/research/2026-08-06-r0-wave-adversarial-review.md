# Адверсариальное ревью волны R0 (коммиты `5935b346`, `125b7981`, `6602ea4e`, `a73c57c6`)

**Дата:** 2026-08-06
**Репозиторий:** `D:\dev\rust\shamir-db`, `HEAD = e2f034ee` (`master`)
**Предмет:** четыре correctness-фикса волны R0, закрывающие 8 P0 из двух ревью
`2026-08-05-new-wave-readonly-review.md` (@fh) и `...-codex.md`, по карте
`docs/dev-artifacts/roadmap/2026-08-05-release-blocker-execution-map.md`.
**Режим:** чтение кода + запуск `cargo fmt --check`, `cargo clippy --workspace
--all-targets -D warnings`, `./scripts/test.sh`, плюс **экспериментальные
локальные ревёрты** для проверки дискриминирующей силы тестов и для
воспроизведения найденного дефекта. Рабочее дерево восстановлено побайтово
(`git status` чист; ни один git-мутирующий вызов не выполнялся).

---

## Короткий вердикт

Три из четырёх коммитов (**R0-D**, **R0-A**, **R0-C**) действительно закрывают
заявленные сценарии — я проследил каждый по текущему коду и подтвердил
ревёртом, что их регрессионные тесты дискриминируют. **R0-B** закрывает
заявленные сценарии, но **вносит новую регрессию того же класса, который
чинит**: две батч-путевые точки staging'а index2-операций остались без
provenance-штампа и уходят в коммит с плейсхолдером `instance_epoch: 0`,
который новый ретрактор гарантированно снимает. Это **потеря postings на
живом пути** (`execute_insert_tx` → `insert_tx_many_bytes` — основной
транзакционный INSERT).

**Вердикт:** волна R0 **НЕ закрыта**. Требуется как минимум фикс F-1 (две
строки + дискриминирующий тест) перед тем, как считать R0-B выполненной.

| # | Severity | Коммит | Кратко |
|---|---|---|---|
| **F-1** | **CRITICAL** | `a73c57c6` (R0-B) | Батч-INSERT (`insert_tx_many`, `insert_tx_many_bytes`) не штампует index2-provenance → все index2-postings транзакции ретрактятся при любом DDL между stage и commit. **Воспроизведён тестом.** |
| **F-2** | HIGH | `5935b346` (R0-D) | #1023 починен только для unique-backfill; идентичный fail-open silent-skip остался в **regular**-backfill (`index_manager.rs:1142-1150`). |
| **F-3** | MEDIUM | `125b7981` (R0-A) | Инвариант «insert() всегда под `ddl_admission`», на котором держится однoсчётчиковая схема реестра, нарушается `DROP TABLE ... CASCADE` (и `doctor::repair()`), которые мутируют реестр и все три менеджера напрямую. |
| **F-4** | MEDIUM | `6602ea4e` (R0-C) | `any_index_exists`-preflight делает `CREATE SORTED INDEX` неидемпотентным (был last-write-wins → стал `KeyExists`), а `IF NOT EXISTS` для sorted/index2 теперь возвращает ошибку вместо `existed: true`. |
| **F-5** | LOW | `a73c57c6` (R0-B) | `IndexRegistry::rename_entry` не двигает `generation`, поэтому для index2 «bumped on RENAME» из doc-комментария `Provenance` **неверно**; работает только благодаря id-based ключам. |
| **F-6** | LOW | `a73c57c6` (R0-B) | Штамп и live-множество index2 читают **construction-time** `descriptor().name_interned`, а не авторитетный `BackendEntry.name_interned` — самосогласованно, но одностороннее «исправление» тихо сломает матчинг. |
| **F-7** | LOW | `a73c57c6` (R0-B) | Асимметрия: base/sorted-ретракция пропускается целиком при `tx.write_set.get(token) == None` (`continue`), index2 — нет (`Vec::new()`). Сегодня недостижимо, но контракт разный. |
| **F-8** | LOW | — | Мёртвые `rename_index_definition` / `rename_unique_index_definition` (`index_manager.rs:2211,2229`) не бампают `generation` — при оживлении это готовый NP-2 для base-семейства. |

Нарушений идеологии CLAUDE.md в волне **не найдено** (см. §6).

---

## 1. `5935b346` — R0-D, fail-closed open-path recovery (#1013, #1023)

### Что проверено

Оба сайта fail-open, названные в обоих ревью, действительно закрыты:

- `table_manager.rs:494-...` — неудачный `drop_all` при Building-self-heal больше
  не идёт в backfill: бэкенд регистрируется, помечается `set_failed(reason)`,
  id кладётся в `failed_recovery_ids` и **пропускается** в цикле
  `restore_on_open` ниже (`table_manager.rs:633`). Ready-флип не выполняется.
- `table_manager.rs:647-657` — неудачный `restore_on_open` → `set_failed(reason)`.

`IndexState::Failed` добавлен **в конец** enum (`state.rs`), что действительно
append-safe для bincode; `Failed` planner-невидим через уже существующие
`state != Ready` гейты (`registry.rs:534` в `find_by_field_and_kind`), и
попадает в `degraded_index_count()` без изменений. `doctor::verify()`
показывает причину (`doctor.rs:295-306` через `failure_reason_of`),
`set_state(!=Failed)` корректно очищает `failure_reason` (`registry.rs:287-289`).

**Проверка дискриминирующей силы (реальный ревёрт).** Убрал
`mgr.index2_registry.set_failed(id, reason).await` в `restore_on_open`-ветке →
`./scripts/test.sh -p shamir-engine -- r0d` → **1 из 2 упал**. Тест
дискриминирует, заявление коммита подтверждено.

#1023 (`index_manager_unique.rs:367-403`) — `continue` заменён на типизированный
`DbError::Codec` с внятным текстом. Корректно.

### F-2 (HIGH). #1023 закрыт наполовину: тот же fail-open остался в regular-backfill

`crates/shamir-index/src/base_index/index_manager.rs:1141-1150`:

```rust
for (key_bytes, value_bytes) in &batch {
    let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
        Ok(a) => a,
        Err(_) => continue,          // ← silent skip
    };
    let record_id = RecordId(arr);
    let value = match InnerValue::from_bytes(value_bytes) {
        Ok(v) => v,
        Err(_) => continue,          // ← silent skip
    };
```

Это байт-в-байт та же конструкция, которую R0-D удалил из
`create_unique_index`'s backfill двумя файлами ниже, в том же коммите, с
обоснованием «fail-closed policy». Ревью назвало только unique-сайт, и фикс
буквально закрыл только его.

**Конкретный сценарий отказа.** Таблица `t`, одна запись `r_bad`, чьи
storage-байты не декодируются `InnerValue::from_bytes` (повреждённый блок,
или запись, записанная другой версией кодека). `CREATE INDEX idx_name ON t(name)`
→ backfill молча пропускает `r_bad`, индекс помечается `Ready`.
Далее `SELECT * FROM t WHERE name = <значение r_bad>` планируется через
`try_plan_and_index_scan` → идёт по индексу → **строка не возвращается**, хотя
физически существует и вернулась бы full-scan'ом. Это silent wrong result —
ровно то, что R0-D объявляет недопустимым в собственном commit message
(«a silently-wrong result is worse than refusing to serve an index»), и по
severity не ниже unique-варианта: unique-дырка допускает лишний дубль,
regular-дырка **тихо теряет строки из ответов**.

Для сравнения — sorted-backfill (`table_manager_sorted_index.rs:201-210`)
уже fail-closed (`return Err(enrich_backfill(e))`), так что после фикса
асимметричным останется только regular.

**Исправление:** тот же `map_err(...DbError::Codec...)?` что в
`index_manager_unique.rs:379-401`, + тест-близнец к уже написанному
`unique_tests.rs`-аборт-тесту.

---

## 2. `125b7981` — R0-A, generation watermark + full DDL admission (#1006, #1012)

### Что проверено

Слияние двух счётчиков сделано корректно и в правильном порядке: `my_gen =
generation.load(Acquire) + 1` **до** публикации, `fetch_max(my_gen, Release)`
**после** публикации в обе проекции (`registry.rs:229-264`) — инвариант 2b
(«читатель, увидевший `generation() == N`, видит все записи с тегом `<= N`»)
сохранён. Ложные doc-утверждения `registry.rs:78-81` и `:92-94`, на которые
указывала карта, переписаны под фактическую семантику.

Admission расширен ровно на четыре названные точки:
`drop_sorted_index` (`table_manager_sorted_index.rs:330-332`),
`drop_index2` (`table_manager_index_mgmt.rs:927-929`),
sorted- и index2-ветки `rename_index` (`:1801-1803`, `:1822-1824`).
Deadlock, найденный при этом самими авторами (tombstone-precheck после
барьера), закрыт переносом проверки до `begin_write_barrier`
(`table_manager_index_mgmt.rs:44-95`); описание остаточной гонки
(«CREATE, выданный за мгновение до DROP, может пройти») честное — это
caller-ordering, не корректность. Вложенных `begin_write_barrier`
(re-entrant deadlock на не-реентрантном `tokio::sync::Mutex`) не найдено:
`rename_index`'s regular-ветка вызывает `create_index`/`drop_index` **не**
удерживая барьер, unique-ветка сознательно зовёт `create_unique_index_body`.

**Проверка дискриминирующей силы (два реальных ревёрта).**

1. Убрал барьер из `drop_sorted_index` → `-- r0a` → упал ровно
   `drop_sorted_index_acquires_write_barrier` (5/6 прошли). Тест на покрытие
   именно этого gap'а дискриминирует.
2. Восстановил pre-R0-A схему реестра (добавил обратно
   `insert_ticket: AtomicU64`, `my_gen = insert_ticket.fetch_add(1)+1`) →
   `-- create_a_drop_a` → **упал** `create_a_drop_a_stage_create_b_commit_indexes_b`.
   То есть центральный e2e-тест коммита действительно ловит исходный дефект
   #1006, а не просто «проходит после фикса».

### F-3 (MEDIUM). Инвариант «insert() только под `ddl_admission`» нарушается DROP TABLE CASCADE

`registry.rs:153-171` объявляет это **предусловием метода**, а commit message —
«at most one registry mutation is ever in flight per table». Это и есть
единственное обоснование, почему `generation.load()+1` (а не `fetch_add`)
безопасен. Но перечисление точек входа неполное:

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:204-245`
(`handle_drop_table`, ветка `op.cascade`) мутирует **все четыре семейства
напрямую, минуя `TableManager`-обёртки и, значит, минуя admission**:

```rust
for id in regular_ids { let _ = table.index_manager_ref().drop_index(id).await; }
for id in unique_ids  { let _ = table.index_manager_ref().drop_unique_index(id).await; }
for id in sorted_ids  { let _ = table.sorted_indexes().drop_index(id).await; }
for b in &backends    { let _ = table.index2_registry().remove_by_id(b.descriptor().id).await; }
```

То же делает `doctor::repair()` (`doctor.rs:501-537`): `drop_index` /
`drop_unique_index` / `sorted_indexes().drop_index` / `register` — всё без
барьера.

**Конкретный сценарий отказа (реестр).** Параллельно идут
`DROP TABLE t CASCADE` и `CREATE INDEX ... ON t` (functional). Пусть
`generation == N`. `create_index_v2` (под admission) читает
`my_gen = N + 1`, публикует запись; в это же окно `remove_by_id` из cascade
делает `generation.fetch_add(1)` → `N + 1`. Затем `fetch_max(N+1)` — no-op.
Итог: **два изменения снимка при одном приращении `generation`**. Транзакция,
застейдженная при `N+1`, на коммите увидит `generation() == stage_gen` и
пропустит rederive целиком — точно тот класс, который #1006 объявил закрытым.

Побочно: cascade'овые `drop_index`/`drop_unique_index`/sorted-`drop_index`
обходят и write-barrier, то есть возвращают окно P0-3b (writer со снимком
defs, снятым до retire, допишет posting после sweep) для пути DROP TABLE.

**Исправление (минимум):** провести cascade через `TableManager::drop_index` /
`drop_unique_index` / `drop_sorted_index` / `drop_index2` (обёртки, которые
уже берут барьер) — либо взять один `begin_write_barrier` на весь блок
cascade. Для `repair()` — аналогично. Пока это не сделано, комментарий
`registry.rs:153-171` следует смягчить: он утверждает больше, чем гарантирует
код.

Дополнительно к #1012 остаётся незакрытым пункт codex-ревью «regular rename
не удерживает одну атомарную секцию на всём drop+create переходе» —
`rename_index`'s regular-ветка берёт барьер дважды (внутри `create_index` и
внутри `drop_index`), между ними чужой DDL может вклиниться. На корректность
tx-плана это не влияет (оба сайта бампают base-generation), но заявление
«единая точка входа DDL» по-прежнему сильнее факта.

---

## 3. `6602ea4e` — R0-C, атомарность реестра + cross-family namespace (#1009, #1010)

### Что проверено

`IndexRegistry::insert` действительно переписан на check-before-mutate
(`registry.rs:192-196`): при занятом имени возвращается `Err` **до** любой
мутации `by_id`, поэтому полусостояние «жив по id, недоступен по имени»
структурно невозможно. Обоснование безопасности check-then-act ссылается на
R0-A (см. F-3 — ссылка не вполне надёжна, но для однопоточного open-path и
для admission-покрытых CREATE она верна).

Три `let _ = ...insert(...)` на open-path заменены на пробрасывание ошибки с
внятным текстом (`table_manager.rs:546,563,576` +
`duplicate_persisted_index2_name_error:350-374`). Выбор «падать всей таблицей,
а не помечать один бэкенд Failed» обоснован корректно: при коллизии имён на
диске нет одного «сломанного» бэкенда.

Cross-family preflight (`any_index_exists`, `table_manager_index_mgmt.rs:1457-1463`)
вызывается **из четырёх create-методов после `begin_write_barrier`** — то есть
внутри admission-окна, а не в handler-слое shamir-db (это правильный выбор:
handler-проверка осталась бы TOCTOU-уязвимой). DROP и RENAME теперь
**отказываются** при >1 совпавшем семействе вместо тихого разрешения
(`admin_table_index.rs:593-627`, `table_manager_index_mgmt.rs:1559-1583`);
short-circuit `||` ниже остаётся корректным ровно потому, что гард выше уже
отсёк множественное совпадение. Два новых чека в `doctor::verify()`
(`check_index2_registry_consistency`, `check_cross_family_name_collisions`)
читают уже загруженные структуры, используют `TFxMap`, лишних сканов не
добавляют.

### F-4 (MEDIUM). Preflight сделал `CREATE SORTED INDEX` неидемпотентным, а `IF NOT EXISTS` — ошибкой

Два независимых следствия одной правки.

**(a) Регресс идемпотентности sorted-create.**
`SortedIndexManager::register` документирован и реализован как
**last-write-wins** (`sorted_index_manager.rs:578-617`: «if a definition with
the same `name_interned` exists, it is replaced in-place»). До R0-C повторный
`create_sorted_index("x", ...)` пересоздавал определение и перестраивал индекс.
После R0-C `create_sorted_index_with_include` (`table_manager_sorted_index.rs:121-126`)
делает `if self.any_index_exists(index_name).await { return Err(KeyExists) }`,
а `any_index_exists` включает `sorted_index_exists` — то есть **собственное
семейство**. Повторный create теперь жёстко падает.

**(b) `IF NOT EXISTS` перестал быть no-op для sorted и index2.**
Handler `handle_create_index` (`admin_table_index.rs:352-374`) как был, так и
остался «своесемейным»:

```rust
let already_exists = if op.unique {
    table.unique_index_exists(&op.create_index).await
} else {
    table.index_exists(&op.create_index).await   // ← только REGULAR base-семейство
};
if already_exists { if op.if_not_exists { return Ok(... "existed": true ...)); } ... }
```

Для `index_type: "fts"|"vector"|"functional"` и для `sorted: true` эта проверка
всегда `false`. Значит поток идёт в `create_index_v2` /
`create_sorted_index_with_include`, где новый preflight возвращает
`DbError::KeyExists`.

**Конкретный сценарий.** `CREATE SORTED INDEX idx_age ON t(age)` выполнен.
Клиент (или idempotent-миграция, или ретрай после таймаута) повторяет
`CREATE SORTED INDEX idx_age ON t(age) IF NOT EXISTS` → **ошибка
`index 'idx_age' already exists on this table (possibly in a different index
family...)`** вместо `{"created": false, "existed": true}`. Для index2 это же
верно; до R0-C там тоже была ошибка (из `IndexRegistry::insert`), но —
существенно — **после полного backfill'а**, то есть R0-C заодно и улучшил этот
путь. Для sorted это чистая регрессия семантики.

Ни один тест волны это не покрывает (gate зелёный).

**Исправление:** заменить `already_exists` в handler'е на
`table.any_index_exists(&op.create_index)` (метод уже `pub`), и — если
идемпотентность sorted-re-create кому-то нужна — либо восстановить её явно,
либо задокументировать смену контракта в CHANGELOG/KNOWN_LIMITATIONS.

---

## 4. `a73c57c6` — R0-B, instance provenance + sorted rename generation (#1007, #1008)

### Что проверено и подтверждено

Заявление «WAL не затронут» верно: `shamir_tx::IndexWriteOp` выводит только
`Debug, Clone`, не `Serialize` (`crates/shamir-tx/src/index_write_op.rs:83`);
`Provenance` живёт только в памяти.

`SortedIndexManager::rename_definition` теперь бампает и `instance_epoch`
конкретного определения (`sorted_index_manager.rs:1042`), и manager-level
`generation` (`:1068`). `instance_epoch` — `#[serde(skip, default =
"next_instance_epoch")]`, то есть свежий на каждом deserialize-пути, включая
legacy-`From`-конверсии; `PartialEq` переписан вручную и эпоху исключает —
round-trip-тесты равенства сохранены осмысленно.

Единый ретрактор `retract_stale_provenance_ops` (`pre_commit.rs:1249-1269`)
фильтрует строго по `(family, table_token)` и оставляет op тогда и только
тогда, когда `(name_interned, instance_epoch)` есть в live-множестве. Все три
rederive-функции им пользуются; байтовая эвристика 41/25 действительно
удалена вместе с породившим её кодом. Проверил, что эпоха **не** меняется
паразитно на «нейтральных» мутациях: base-флип `Building → Ready` —
clone-then-mutate (`index_manager.rs:1358-1362`), sorted `mark_ready_at` —
RCU in-place (`sorted_index_manager.rs:476-487`), `doctor::repair()` —
`register(def.clone())`. Base-RENAME (regular/unique) реализован как
create-new + drop-old, оба сайта бампают `generation`, новое определение
получает свежую эпоху через `IndexDefinition::new` — то есть ABA закрыт и там.

**Проверка дискриминирующей силы (два реальных ревёрта).**

1. Убрал `self.generation.fetch_add(1)` из `rename_definition` →
   `-- p1007` упали оба unit-теста, `-- p1008` упал именно
   `sorted_rename_between_stage_and_commit_no_orphan_lands_under_new_name`
   (8/9 остальных прошли). То есть e2e-тест части 1 действительно
   дискриминирует, а не «проходит заодно».
2. Занейтралил `retract_stale_provenance_ops` (всегда `true`) → `-- p1008`
   упало **8 из 9**; прошёл ровно один — умышленный non-regression
   `rollback_after_multiple_rotations_leaves_no_partial_state`. Это в точности
   то, что заявляет commit message.

### F-1 (CRITICAL). Батч-INSERT не штампует index2-provenance → все index2-postings транзакции ретрактятся

Это и есть тот самый «плейсхолдер в живом пути», от которого предупреждает
собственная документация фикса.

`crates/shamir-index/src/write_ops.rs:12-37` определяет `index2_provenance()`
как **плейсхолдер** с `instance_epoch: 0` и явно перечисляет обязанных его
перезаписать: «`TableManager::plan_insert_ops`/`plan_update_ops`/
`plan_delete_ops` (stage time) и `rederive_index2_ops_post_stage` (commit
time) — MUST overwrite this placeholder». Перечисление **неполное**: два
батч-путевых staging-сайта планируют index2-операции **в обход** этих
хелперов (они специально инлайнят цикл, чтобы амортизировать
`all_backends()`-снимок) и штамп не делают:

- `crates/shamir-engine/src/table/table_manager_tx_ops.rs:595-605` —
  `insert_tx_many`;
- `crates/shamir-engine/src/table/table_manager_tx_ops.rs:776-786` —
  `insert_tx_many_bytes`.

```rust
for backend in &backends {
    let ops = backend.plan_insert_tx(*rid, view, tx_id).await...?;
    index_ops.extend(ops);          // ← нет stamp_index2_provenance
    ...
}
```

Для сравнения — четыре «обычных» сайта (`:67`, `:204`, `:230`, `:258`) штамп
делают.

`insert_tx_many_bytes` — **не редкий путь**: это то, во что упирается
`execute_insert_tx` (`table/write_exec.rs:292`, `:366`) и `execute_set_tx`
(`:1315`), то есть **любой транзакционный INSERT/UPSERT через исполнитель
запросов**, включая одиночную строку.

**Конкретный сценарий отказа.** Таблица `t` с FTS/functional-индексом
`fts_a` (эпоха = `BackendEntry.gen`, например 1).

1. Транзакция стейджит INSERT строки `r` → staged index2-op c
   `Provenance { family: Index2, name_interned: intern("fts_a"), instance_epoch: 0 }`.
2. Между stage и commit на этой же таблице происходит **любая** index2-DDL —
   например `CREATE INDEX fts_b` (совершенно другой индекс) или DROP третьего.
   `generation` → 2.
3. Commit: гейт `reg.generation() != stage_gen` открывается; rederive
   доплановывает ops только для `backends_newer_than(stage_gen)`, то есть
   для `fts_b`; затем строится
   `live_index2 = {(fts_a, 1), (fts_b, 2)}` и вызывается ретрактор.
4. Staged op `(fts_a, 0)` в live не входит (эпохи стартуют с 1 — это прямо
   написано в `write_ops.rs:18-22`), значит **ретрактится**.
5. Строка коммитится в таблицу, но **в `fts_a` её постингов нет — навсегда**.

Дополнительно `IndexWriteOp::BumpFtsStats` provenance не несёт и ретракции не
подлежит (`pre_commit.rs:1262`), поэтому для `FtsRankedBackend` BM25-статистика
(`doc_count`, `sum_doc_len`) окажется **инкрементирована для строки, постингов
которой нет** — рассинхрон, который переживёт до ближайшего `rebuild()`.

Направление ошибки — только «ложная ретракция» (эпоха 0 не совпадает ни с чем
живым), то есть строго потеря postings, не воскрешение.

**Воспроизведение (выполнено).** Временный тест в
`crates/shamir-engine/src/table/tests/` (после проверки удалён, дерево
восстановлено):

```rust
// index2 "lower_name" создан; батч-стейдж; затем неродственный CREATE "lower_other"; commit
let ids = tbl.insert_tx_many(&[record_with_two_str(name_key, "Alice", other_key, "X")], &mut tx).await.unwrap();
tbl.create_index_v2(&functional_lower_op("lower_other", "t", "other")).await.unwrap();
repo.commit_tx(tx).await.unwrap();
assert_eq!(functional_lookup(&tbl, idx_name, "alice").await.len(), 1);
```

Результат на `HEAD`:

```
FAIL  table::tests::…::batch_insert_index2_posting_survives_unrelated_index2_create
      assertion `left == right` failed: left: 0, right: 1
PASS  table::tests::…::single_insert_index2_posting_survives_unrelated_index2_create
```

То есть одиночный путь (`insert_tx`, штампованный) — цел, батч-путь — теряет
постинг. После добавления одной строки
`self.stamp_index2_provenance(backend, &mut ops).await;` в цикл
`insert_tx_many` оба теста проходят (2/2) — корень зафиксирован точно.

**Исправление:** добавить `stamp_index2_provenance` в оба батч-цикла
(`:595-605` и `:776-786`), обновить перечисление в `write_ops.rs:22-27`
(оно и есть ловушка: доктрина перечисляет функции, а не инвариант), и завести
дискриминирующий тест ровно на батч-путь. Более надёжный вариант — сделать
невозможным «незаштампованный» op: например, чтобы index2-op вообще нельзя
было положить в `tx.index_write_set` иначе как через хелпер, делающий штамп.

### F-5 (LOW). `rename_entry` не двигает generation — doc `Provenance` утверждает неверное для index2

`IndexRegistry::rename_entry` (`registry.rs:473-510`) меняет `by_name` и
авторитетные слоты `by_id`, но **не** трогает `generation` и **не** трогает
`entry.gen`. Между тем `Provenance`'s doc (`index_write_op.rs:37-40`) говорит:
«every definition mints a FRESH epoch on CREATE and bumps it on RENAME», после
чего в скобках отсылает index2 к `BackendEntry.gen`. Для index2 «bumps it on
RENAME» просто неверно.

Сегодня это **не** дефект корректности: физические ключи index2-постингов
содержат числовой `id`, а не `name_interned`, поэтому staged op после RENAME
всё равно попадает в правильный namespace, а гейт rederive не обязан
срабатывать. Но: (а) doc-утверждение ложно, и это ровно тот класс ложных
инвариантов в комментариях, который карта R0-A требовала вычистить в
`registry.rs`; (б) любая будущая замена id-based ключей на name-based
превратит это в тихую потерю postings без единого падающего теста.

### F-6 (LOW). Штамп и live-множество читают stale `descriptor().name_interned`

`table_manager_tx_ops.rs:95` (`stamp_index2_provenance`) и
`pre_commit.rs:1037-1039` (построение `live_index2`) оба берут
`backend.descriptor().name_interned` — **construction-time снимок**. При этом
`registry.rs:69-75` прямо объявляет авторитетным `BackendEntry.name_interned`,
а `all_descriptors()` его переопределяет именно потому, что снимок после
RENAME протухает.

Сегодня это самосогласовано (обе стороны читают одно и то же протухшее
значение, поэтому матчатся), но контракт хрупкий: «исправление» любой одной
из двух точек на авторитетное значение немедленно ломает матчинг после
index2-RENAME → массовая ложная ретракция. Стоит либо привести обе к
авторитетному значению (и тогда F-5 становится обязательным), либо явно
задокументировать, что здесь намеренно используется construction-time id.

### F-7 (LOW). Асимметричный ранний `continue` перед ретракцией

`pre_commit.rs:844` (index2) при отсутствии staged-ops для таблицы берёт
`Vec::new()` и **всё равно доходит до ретракции**; sorted (`:1083`) и
base_index (`:1328`) в том же положении делают `continue`, то есть **ретракцию
пропускают целиком**. Сегодня недостижимо (`stage_mutation` всегда вызывает
`ensure_table_staging`, так что запись в `write_set` есть), но три копии
одного алгоритма с разным поведением на пустом входе — именно та почва, на
которой волна уже дважды поскальзывалась. Комментарий у index2-версии
(`:838-841`) объясняет, почему `Vec::new()` правильнее; sorted/base его не
получили.

### F-8 (LOW). Мёртвые rename-хелперы без bump generation

`IndexManager::rename_index_definition` / `rename_unique_index_definition`
(`index_manager.rs:2211`, `:2229`) не имеют ни одного вызова в репозитории
(проверено grep'ом по всем крейтам, включая тесты). Они делают
`remove_index` + `add_index(IndexDefinition::new(...))`, но **не бампают
`generation`**. Если их когда-нибудь подключат как «дешёвый rename» вместо
нынешнего drop+create, это мгновенно даст NP-2 для base-семейства (гейт
rederive не сработает → staged ops уйдут под старый `name_interned`, строка
пропадёт из переименованного индекса). Либо удалить, либо добавить
`bump_generation()` и `#[allow(dead_code)]`-обоснование.

---

## 5. Воспроизводимость заявленных gate-результатов

| Проверка | Результат у меня |
|---|---|
| `cargo fmt --all -- --check` | чисто (exit 0) |
| `cargo clippy --workspace --all-targets -- -D warnings` | чисто (exit 0) |
| `./scripts/test.sh -p shamir-index -p shamir-engine -p shamir-tx -p shamir-db --full` | **3569/3569 passed**, exit 0, 208 s |

Заявленные в коммитах цифры воспроизводятся (наборы крейтов разные: 2478 в
R0-D, 2485 в R0-A, 3179 в R0-C, 2888 в R0-B — у меня объединённый набор из
четырёх крейтов даёт 3569).

`TIMEOUT` — ни одного. 14 `SLOW` — все pre-existing и не относятся к волне:
13 из них `shamir-db::functions_lifecycle::wasm_*` / secret-grant (WASM-компиляция,
у части есть собственные override'ы в `.config/nextest.toml`) и один
`shamir-engine::table::tests::filtered_ann_tests::vr5_cofilter_sees_staged_and_filters_residual`
(132 s, HNSW). Ни один из них не трогается коммитами R0. Точечные прогоны
(`-- r0a`, `-- r0d`, `-- p1007`, `-- p1008`) — доли секунды.

Отдельно отмечу, что заявленный в `a73c57c6` единственный workspace-fail
(`filtered_ann_low_selectivity_finds_rare`, HNSW-recall flakiness) —
действительно pre-existing и к волне отношения не имеет.

---

## 6. Соответствие идеологии CLAUDE.md

Диапазон `5935b346~1..a73c57c6` (51 файл, +4479/−420) проверен на:

- новые `std::sync::Mutex` / `std::sync::RwLock` / `parking_lot::*` — **нет
  ни одного** добавленного;
- сырые `HashMap::new()` / `HashSet::new()` / `RandomState` — **нет**; новые
  структуры это `TFxSet<(u64,u64)>` (live-множества в `pre_commit.rs`),
  `TFxMap` (`doctor.rs:398`, `:442`);
- `scc::*::len()` — новых вызовов нет; существующие аннотированы
  `#[allow(clippy::disallowed_methods)] // O(N) ack: …`;
- inline `#[cfg(test)] mod tests { … }` — **нет**; все шесть новых тестовых
  файлов лежат в `tests/`-директориях и подключены через `tests/mod.rs`
  (манифест только из `pub mod`);
- `use` внутри тел функций — новых нет (кроме `use super::*` в тестах и
  локального `use crate::index2::backend::{IndexQuery, IndexResult}` в
  тестовом хелпере, что подпадает под задокументированное исключение);
- новых `panic!`/`unwrap()` в библиотечном коде нет; все новые ошибки —
  типизированные `DbError::{Codec,Internal,KeyExists}`.

Стоимость на общем пути не выросла: ретракция и rederive платятся только
когда `generation` реально сдвинулся; `retract_stale_provenance_ops` — один
`retain` по `index_write_set`, вызываемый ровно в тех же ветках, где раньше
стоял 2c-retain.

**Стоило бы поправить (не блокеры):** ложное doc-утверждение F-5; неполное
перечисление обязанных штамповать в `write_ops.rs:22-27` (F-1 — прямое
следствие того, что доктрина зафиксирована перечислением, а не инвариантом);
`registry.rs:153-171` обещает предусловие, которого код не обеспечивает (F-3).

---

## 7. Достигнута ли цель волны R0 (8 P0)

| P0 | Коммит | Статус после моей проверки |
|---|---|---|
| #1013 fail-closed restore/drop_all | R0-D | **Закрыт.** Оба сайта, `Failed` planner-невидим, попадает в gauge и в `verify()` с причиной. |
| #1023 fail-open в unique-backfill | R0-D | **Закрыт частично** — идентичный сайт в regular-backfill остался (**F-2**). |
| #1006 generation watermark | R0-A | **Закрыт** для admission-покрытых путей; предусловие нарушается DROP TABLE CASCADE / repair() (**F-3**). |
| #1012 полное DDL admission | R0-A | **Закрыт для четырёх названных точек**; cascade и `repair()` по-прежнему в обход (**F-3**), regular-rename не одна секция. |
| #1009 атомарность `insert` | R0-C | **Закрыт.** check-before-mutate + fail-closed open path. |
| #1010 cross-family namespace | R0-C | **Закрыт** по существу (CREATE отвергает, DROP/RENAME отказываются, verify() показывает), но с побочной регрессией семантики (**F-4**). |
| #1007 sorted rename generation | R0-B | **Закрыт**, подтверждено ревёртом. |
| #1008 provenance-ретракция | R0-B | **НЕ закрыт**: механизм верный, но два живых staging-сайта его не используют и из-за этого **теряют postings** (**F-1**). |

**Итог.** Архитектурно волна сделана правильно: единая identity вместо трёх
эвристик, один источник монотонности, один fail-closed рефлекс на open-path,
единый namespace. Три из четырёх ходов доведены до конца, и их тесты честно
дискриминируют (проверено ревёртами, а не на слово). Но R0-B в текущем виде
**ухудшает** ситуацию относительно `37cc59a3` на самом ходовом пути записи:
до R0-B index2-ретракции не было вообще, поэтому staged ops батч-INSERT'а
доживали до применения; после R0-B они снимаются при любом конкурентном
index2-DDL. Пока F-1 не исправлен, выкатывать R0-B нельзя — это ровно тот
шаблон «фикс локально корректен, но соседний случай не покрыт», на котором
волна уже спотыкалась в #1003/#1005 и #1004/#1005.

**Минимальный порядок доработки:** F-1 (+ тест на батч-путь) → F-2 → F-4 →
F-3 → доп. правки комментариев F-5/F-6/F-7/F-8.

---

## Приложение. Методика и воздействие на дерево

Выполнялись только чтение и запуск проверок. Для проверки дискриминирующей
силы тестов и для воспроизведения F-1 делались временные локальные правки
(четыре ревёрта + один временный тестовый файл), каждая откатывалась сразу
после прогона. По завершении рабочее дерево восстановлено побайтово из
blob'ов `HEAD` (`git status --porcelain` пуст); ни одна мутирующая git-команда
(`reset`/`checkout`/`stash`/`clean`/`commit`) не выполнялась.
