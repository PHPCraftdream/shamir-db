# S.H.A.M.I.R. Database — readonly review новой волны перед первым релизом

Дата: 2026-08-05  
Режим: только чтение Git и файлов; сборка, тесты, форматирование и изменение исходников не выполнялись.  
Ревизия: `37cc59a3c38f6130ec764fa977ff0c0fc117e59f` (`master`, совпадает с `origin/master` на момент снимка).  
Предыдущая точка ревью: `72106a908fd817433170496bab376d2fba27f7ab`.  
Диапазон: 99 коммитов, 215 файлов, примерно `+25 766 / -1 148` строк.

## Итог

Вердикт для публичного `0.1.0-alpha.1`: **NO-GO**.

Новая волна заметно улучшила crash recovery индексов, DDL-тестирование, бинарные сравнения, наблюдаемость и release workflow. Работа не была напрасной: несколько прежних P0 действительно закрыты или существенно сужены.

Однако поверх исправлений обнаружились новые композиционные ошибки. По отдельности CREATE, DROP, RENAME и транзакционный commit выглядят покрытыми, но их сочетания всё ещё могут:

- пропустить запись в только что созданном индексе;
- записать posting в уже удалённый индекс;
- испортить переименованный sorted index;
- оставить дублирующий index2 backend, недоступный по имени;
- сделать Ready-индекс неполным после неудачного восстановления;
- удалить физические postings, пока старый reader всё ещё использует backend.

До публичного релиза необходимо закрыть P0 ниже и получить зелёный CI на замороженном SHA. После этого нужен отдельный release-candidate прогон, а не выпуск с текущего moving `master`.

## Что сделано правильно

Подтверждённые улучшения новой волны:

1. Для DDL, проходящего через write barrier, добавлен admission mutex и устранена multi-owner гонка снятия барьера.
2. Повреждённый unique index теперь ведёт себя fail-closed, а не пропускает потенциальный дубль.
3. Добавлена транзакционная rederive-логика для жизненного цикла индексов; отдельный regression fix исправил порядок rederive и unique guards.
4. DROP получил durable tombstones для base, sorted и index2 семейств; существенно улучшено восстановление после падения.
5. Исправлена persistence index2 RENAME, добавлена resumable recovery для sorted и hash rename.
6. Проверка авторизации перенесена до existence probe в большем числе административных путей.
7. Исправлены binary value roundtrip и неоднозначность binary/string в фильтрах.
8. Добавлена метрика degraded indexes с учётом identity и исправлены её ложные срабатывания.
9. Существенно расширены E2E DDL/OQL тесты и матрица Rust/TypeScript CreateIndex builder.
10. Добавлены fallible batch adapters и внутренний typed `IndexSpec`.
11. Ограничения продукта теперь описаны честнее; README больше не обещает drop-in замену PostgreSQL/MySQL/MongoDB/Redis/Memcached.
12. Release workflow проверяет больше платформ, интеграционные тесты, SBOM, подписи, Docker smoke и версии.

Это хорошая база для alpha, но перечисленные механизмы пока не образуют единый линейризуемый lifecycle.

## P0 — блокеры корректности

### P0-1. `IndexRegistry` публикует неверное поколение

Файл: `crates/shamir-index/src/registry.rs:121-199`, удаление — `:259-275`; тесты — `crates/shamir-index/src/tests/registry_tests.rs:317`.

В реестре используются два независимых счётчика:

- `insert_ticket` выдаёт номер новой вставке;
- `generation` увеличивается при remove и обновляется через `fetch_max(my_gen)` при insert.

`max(reservation ticket)` не является watermark завершённой публикации. Есть два воспроизводимых класса ошибки.

Последовательный сценарий:

1. CREATE A получает ticket/generation 1.
2. DROP A увеличивает `generation` до 2, но не двигает `insert_ticket`.
3. Транзакция планируется в поколении 2 без A.
4. CREATE B получает ticket 2; `generation.fetch_max(2)` не меняет поколение.
5. Commit видит то же поколение 2 и не rederive-ит операции для B.

В результате строка становится видимой в таблице, но отсутствует в B.

Параллельный сценарий:

1. Insert A резервирует ticket 1 и задерживается до публикации.
2. Insert B резервирует ticket 2 и публикуется, `generation = 2`.
3. Транзакция планируется с B при поколении 2.
4. A публикуется позже, но `fetch_max(1)` оставляет поколение 2.
5. Commit не замечает появление A.

Текущий тест проверяет уникальность тегов и итоговое увеличение поколения после завершения всех задач. Он не проверяет, что каждое изменение снимка обязательно меняет наблюдаемое поколение и что watermark не обгоняет публикацию.

Что требуется:

- один сериализованный publication sequence для insert/remove/rename/state transition; либо
- reservation tickets плюс отдельный contiguous-published watermark, который продвигается только после публикации всех предыдущих tickets;
- тесты с deterministic pause до и после публикации;
- тест `CREATE A → DROP A → stage tx → CREATE B → commit`;
- инвариант: одинаковое наблюдаемое поколение означает идентичный planner-visible набор backend instances.

### P0-2. Sorted RENAME ломает транзакции, спланированные до переименования

Файлы: `crates/shamir-index/src/base_index/sorted_index_manager.rs:1024-1055`, `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1658-1661`.

`rename_definition` меняет `old_id` на `new_id`, переносит postings и сохраняет metadata, но не меняет lifecycle generation. Внешний путь sorted RENAME также не удерживает общий table write barrier на всём переходе.

Сценарий:

1. Транзакция строит sorted posting ops с `old_id`.
2. RENAME переносит существующие postings и завершает tombstone.
3. Транзакция commit-ится после переноса и пишет операции под `old_id`.
4. Поколение не изменилось, rederive не выполняется.

Итог: новый индекс пропускает строку, а в старом namespace появляются orphan postings.

Исправление должно не просто добавить barrier. Нужен новый instance identity/epoch, чтобы commit заменял старый план новым, либо RENAME должен сохранять стабильный физический id, меняя только логическое имя.

### P0-3. Reconcile добавляет новые index ops, но не удаляет все устаревшие

Файл: `crates/shamir-engine/src/tx/pre_commit.rs:797-1368`.

Текущий lifecycle reconcile асимметричен:

- base index пытается `retain`-ить живые планы и добавляет актуальные;
- index2 добавляет backend-и новее staged generation;
- sorted повторно добавляет актуальные definitions.

Для sorted и index2 нет полноценного удаления операций, подготовленных для уже удалённого instance. После DROP поздний commit может снова создать postings под удалённым id.

У base index фильтрация опирается на `(family, name_interned)`. Это не защищает ABA:

1. транзакция планируется для индекса `x` по полю `a`;
2. `x` удаляется;
3. создаётся новый `x` по полю `b`;
4. commit сохраняет старые и добавляет новые операции, потому что имя снова живо.

Для unique это способно породить ложные конфликты. Для sorted повторное имя означает прямое загрязнение нового индекса posting-ами старого определения.

Нужен не эвристический retain по байтам или имени, а provenance каждой `IndexWriteOp`: семейство, immutable instance id/catalog epoch и definition version. Reconcile должен заменять план instance целиком.

Минимальная тестовая матрица:

- stage → DROP → commit для base/sorted/index2;
- stage → DROP → CREATE same name/different field → commit;
- stage → RENAME → commit;
- несколько lifecycle переходов между stage и commit;
- rollback после частично применённых rederived ops.

### P0-4. `IndexRegistry::insert` оставляет частично вставленный backend

Файлы: `crates/shamir-index/src/registry.rs:178-198`, `crates/shamir-engine/src/table/table_manager.rs:466-520`, `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:352-357`.

Insert сначала добавляет backend в `by_id`, затем в `by_name`. Если имя уже занято, функция возвращает ошибку без rollback `by_id`.

CREATE index2 до этого уже может:

- сохранить Building descriptor;
- выполнить backfill;
- создать физические postings.

При коллизии имени получается backend, живой по id и видимый через обход всех backend-ов, но недоступный по имени. На startup ошибка `insert` игнорируется через `let _ = ...`, после чего Building descriptor может быть переведён в Ready и снова сохранён.

Это не только утечка: planner, который выбирает backend по полям, способен использовать orphan backend, а DROP по имени удалит другой instance.

Требуется:

- атомарная вставка в обе проекции либо обязательный rollback;
- preflight глобальной уникальности имени до persistence/backfill;
- startup должен fail-closed на duplicate metadata, а не игнорировать ошибку;
- consistency check `by_id ↔ by_name ↔ persisted descriptors`.

### P0-5. Имя индекса не уникально между четырьмя семействами

Файл: `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:352-357,593-618`; RENAME — `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1437-1690`.

Сейчас возможно создать regular, unique, sorted и index2 с одинаковым логическим именем. CREATE проверяет не единый namespace, а отдельные ветви. Последствия:

- DROP short-circuit-ит на первом найденном семействе и оставляет остальные;
- RENAME может последовательно переименовать несколько совпавших семейств;
- ошибка в середине RENAME оставляет частично изменённый каталог;
- DESCRIBE, recovery и operator tooling получают неоднозначную identity.

Перед release нужен единый per-table index namespace. Если совместимость уже важна, startup doctor должен детектировать старые коллизии и требовать явного repair/rename; молча выбирать семейство нельзя.

### P0-6. DROP не защищает уже начавшихся readers

Файлы: `crates/shamir-index/src/base_index/index_manager.rs:1616-1630`, `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:863-872`.

Удаление metadata и registry entry защищает только новых planners. Старый query уже может держать `Arc<backend>`, но backend использует общий `info_store`. `drop_all` физически удаляет postings из этого store, поэтому `Arc` сам по себе не сохраняет снимок данных.

В base index это ограничение прямо признано комментарием. В index2 комментарий утверждает более сильную гарантию, чем даёт реализация. Sorted имеет тот же класс риска.

Возможные решения:

- retire metadata сразу, а physical GC выполнять после read-side grace period/epoch;
- immutable generation namespace и удаление старого namespace после исчезновения pinned readers;
- snapshot-capable backend store.

До реализации безопаснее оставлять физические postings и выполнять GC на controlled startup/offline doctor, чем удалять их во время активных запросов.

### P0-7. DDL admission покрывает не весь DDL

Файлы: `crates/shamir-engine/src/table/table_manager_sorted_index.rs:297-304`, `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:863-970,1437-1690`.

Admission mutex берётся через `begin_write_barrier`, но не все операции проходят через него:

- sorted DROP вызывает manager напрямую;
- index2 DROP выполняет tombstone/remove/sweep напрямую;
- sorted/index2 RENAME не удерживают общий barrier/admission;
- regular rename не удерживает одну атомарную секцию на всём drop+create переходе.

Следовательно, формулировка «per-table DDL admission» сейчас сильнее фактической гарантии. Несколько DDL разных семейств могут пересекаться, а writer может иметь подготовленные operations в момент sweep.

Нужен один TableManager DDL coordinator для CREATE/DROP/RENAME/ALTER всех семейств. Внутренние manager API должны либо требовать доказуемый guard token, либо быть private, чтобы новый вызов нельзя было случайно пустить в обход координатора.

### P0-8. Startup recovery продолжает работу после ошибок, оставляя Ready неполным

Файл: `crates/shamir-engine/src/table/table_manager.rs:483-585`; контракт — `crates/shamir-index/src/backend.rs:250-276`; vector restore — `crates/shamir-index/src/vector/vector_backend.rs:637`.

Для persisted Building index2 ошибка `drop_all` только логируется. Затем выполняется backfill, индекс переводится в Ready и metadata сохраняется. Комментарий признаёт, что partial postings могут сохраниться. Для FTS это особенно опасно: stats обновляются не как чистый overwrite, поэтому неполная очистка и rebuild могут удвоить статистику.

В общем restore loop ошибка `restore_on_open` также только логируется. Backend остаётся Ready. Свежесозданный vector adapter при неудаче snapshot/rebuild может остаться пустым, но planner продолжит считать индекс пригодным и возвращать неполные результаты.

Для базы данных silent wrong result хуже отказа открыть таблицу. Корректная политика:

- failed restore переводит backend в `Failed`/`Building` и делает planner-invisible;
- либо открытие таблицы завершается ошибкой;
- Ready выставляется только после доказанного полного восстановления;
- причина и recovery action доступны через operator API.

## P1 — необходимо до публичной alpha

### P1-1. Doctor/repair не является операторской функцией

Код и метрика рекомендуют выполнить `TableManager::verify()/repair()`, но в просмотренном query/admin/server/CLI контуре нет команды, которой владелец установленного binary мог бы это сделать.

Нужен хотя бы один поддерживаемый интерфейс:

- offline `shamir-server doctor --data-dir ...` и `repair --dry-run/--apply`; либо
- аутентифицированный admin endpoint с progress/status.

Он должен показывать Building/Failed, metadata/registry расхождения, orphan namespaces, duplicate names и recovery tombstones. Пока такого пути нет, текст «run doctor repair» неоперационален.

### P1-2. DDL result contract пока существует только как RFC

RFC `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md` правильно описывает проблему, но wire/API всё ещё возвращает обычный success/error. Ошибка может прийти после live mutation или durable metadata mutation, и клиент не знает итоговое состояние.

Рекомендуется реализовать единый результат до расширения DDL:

- operation id/idempotency key;
- `Accepted / Building / Ready / Retired / Failed`;
- текущая phase;
- `retryable`;
- `state_changed`;
- recovery action;
- status lookup после disconnect/restart.

Без этого online build, resumable DDL и ALTER только увеличат неоднозначность.

### P1-3. Query Builder всё ещё допускает panic в обычном API

Файлы: `crates/shamir-query-builder/src/batch/batch.rs`, `batch/into_batch_op.rs:49-84`, `ddl/schema.rs:504-515`, `ddl/replication.rs:286-297`.

Новые `TryIntoBatchOp` и `try_update/try_upsert/try_delete/try_op` полезны, но additive API не устраняет старую ловушку:

- основные `Batch::update/upsert/delete/op` принимают `IntoBatchOp`;
- некоторые конверсии всё ещё вызывают `.expect(...)`;
- panic происходит при добавлении операции, поэтому последующий `Batch::try_build()` его не перехватит.

Для alpha лучше сделать fallible путь основным сейчас, пока breaking changes дёшевы. Panicking adapters следует удалить или явно назвать `*_unchecked` и deprecated.

### P1-4. CreateIndex остаётся stringly typed и местами fail-open

Файлы: `crates/shamir-query-builder/src/ddl/create_index.rs`, `ddl/index_spec.rs`, server validation в `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`.

Внутренний `IndexSpec` — правильный шаг, но публичный builder всё ещё принимает строки для index type, metric, quantization и tokenizer. `build()` остаётся lenient, а строгий `try_build()` нужно выбрать явно.

Риски:

- неизвестный index type может сначала интерпретироваться как Hash, а затем упасть уже на server;
- неизвестная vector quantization тихо становится `None`;
- неизвестный tokenizer может тихо стать Whitespace;
- для non-btree можно передать несколько fields, хотя backend фактически использует только первый.

Нужен typed public API:

- `.hash(field)`, `.unique(field)`, `.sorted(fields)`;
- `.fts(field, Tokenizer)`;
- `.functional(field, FunctionRef)`;
- `.vector(field, NonZeroU32, Metric, Quantization)`;
- raw DTO оставить отдельным escape hatch;
- для FTS/vector/functional строго требовать ровно одно поле.

Rust и TypeScript должны генерироваться или проверяться одной schema/fixture matrix, включая все invalid rows.

### P1-5. CREATE INDEX всё ещё останавливает writes на минуты

Документ: `docs/guide-docs/KNOWN_LIMITATIONS.md:351-377` соответствующего раздела; benchmark `crates/shamir-engine/benches/f78_writer_latency.rs`.

Текущий backfill удерживает write barrier на полном сканировании. В документации приведён порядок 140–160 секунд для 100k records и потенциально часы для 1M. Новая волна добавила честную документацию, progress log и alpha bar, но не исправила архитектуру.

Для заявленного small/indie/medium сегмента это существенно. Допустимы два релизных варианта:

1. Alpha-ограничение: maintenance mode, жёсткий max rows/estimated cost, явное подтверждение и отказ запуска сверх лимита.
2. Правильное решение: snapshot scan → durable delta capture → catch-up → короткий publish barrier.

Progress должен быть queryable, а не только находиться в log.

### P1-6. Текущий CI не зелёный

Согласно уже существовавшему незакоммиченному checkpoint `docs/checkpoints/2026-08-05-2010.md`, run `31032501528` имеет красный Windows integration job: четыре batch E2E теста дважды превысили 30-секундный client budget. Остальные jobs указаны зелёными.

Это локальная запись состояния, а не повторная онлайн-проверка. В рамках readonly review CI не перезапускался.

Нужно отличить две причины:

- интеграционный тест объективно требует больше 30 секунд — тогда он должен задавать явный test-specific budget;
- Windows path действительно деградировал — тогда повышение timeout скроет regression.

Релиз нельзя основывать на «rerun eventually green». Требуется причина, исправление и чистый прогон того же SHA.

## DDL: куда развивать

Приоритет DDL сейчас — не новые синтаксические операции, а единый lifecycle.

Рекомендуемый порядок:

1. Единый каталог и immutable `IndexInstanceId` для всех семейств.
2. Один DDL coordinator и обязательный guard для CREATE/DROP/RENAME/ALTER.
3. Durable state machine с operation id: Prepared → Building → CatchingUp → Ready → Retiring → Retired/Failed.
4. Идемпотентное recovery каждого перехода.
5. Read-side epochs и отложенный physical GC.
6. Online CREATE INDEX.
7. После этого — `ALTER INDEX`, `REINDEX`, `VALIDATE INDEX`, `CANCEL DDL`, `SHOW DDL STATUS`.

Практичные alpha-команды:

- `CREATE INDEX ... IF NOT EXISTS` с проверкой полного definition equality, а не только имени;
- `DROP INDEX ... IF EXISTS` с однозначным global namespace;
- `DESCRIBE INDEX` с family, instance id, state, progress, persisted generation и last error;
- `REINDEX` как отдельная durable operation;
- `EXPLAIN DDL`/cost estimate до тяжёлого backfill.

Не следует добавлять ещё одно семейство индексов, пока существующие четыре не подчинены одному contract.

## OQL: чего не хватает

### Непосредственные пробелы

1. `SelectItem::Expression` существует в wire/parser, но executor явно отклоняет его (`crates/shamir-engine/src/query/select_projection.rs:80-120`). Это dead surface: либо реализовать, либо убрать из публичного contract до готовности.
2. Нужны вычисляемые projections: arithmetic, comparison, boolean, coalesce/null handling, scalar functions и aliases с едиными типовыми правилами.
3. Нужен `EXPLAIN ANALYZE`: фактические rows, chosen index instance, scan/post-filter count, memory, spill, elapsed и reason отказа от индекса.
4. Нужны semi-join/`EXISTS` и alias-based lookup как более безопасный следующий шаг, чем полный general-purpose JOIN.
5. Нужен настоящий keyset cursor на уровне engine, а не повторный полный pinned scan каждой страницы.

### Что пока не стоит обещать

Полные JOIN, window functions и сложные subqueries резко увеличат требования к memory accounting, spill, cancellation и optimizer. Сначала следует закрыть streaming/resource model и composite index planner. Для первого alpha честный ограниченный OQL лучше широкого, но непредсказуемого SQL-подобного языка.

Предлагаемый порядок OQL:

1. expression evaluator и строгая типизация;
2. `EXPLAIN ANALYZE`;
3. engine cursors/keyset continuation;
4. composite/covering planner и статистика selectivity;
5. bounded aggregate/distinct с spill;
6. semi-joins/EXISTS;
7. только затем joins/windows.

## Query Builders: рекомендуемое расширение

Кроме устранения panic и stringly CreateIndex:

- typestate builders для обязательных полей;
- `NonZeroU32/NonZeroUsize` для page size, vector dimensions и лимитов;
- typed field paths вместо произвольных строк там, где это возможно;
- единый `BuilderError` с stable code/path/value, одинаковый в Rust и TS;
- `build()` сделать строгим, `build_unchecked()` — явным escape hatch;
- capability/version handshake, чтобы client не генерировал unsupported wire surface;
- DDL builders должны возвращать operation handle/status type;
- compile-time или fixture-driven parity test для каждой операции и каждого invalid state;
- убрать неоднозначные `Into`-конверсии, способные panic-нуть внутри batch.

## Производительность и ресурсы

### Главный bottleneck — backfill под глобальным write barrier

Это важнее мелких аллокаций в query path. Сначала online build и bounded backpressure, затем микрооптимизация.

### Настоящего streaming результата нет

`QueryResult` остаётся `Vec`, а cursor page в общем случае повторяет полный scan. `ORDER BY`, `GROUP BY`, `DISTINCT` материализуют состояние. Это ограничивает средние проекты по RAM и latency даже при небольшом binary.

Нужны:

- storage iterator, живущий между cursor pages;
- bounded channel до transport;
- cancellation propagation;
- per-query memory budget;
- external spill для sort/group/distinct;
- лимит pinned MVCC age и понятная ошибка истёкшего cursor.

### Reconcile index ops не должен быть byte-length heuristic

Помимо корректности, повторное планирование и сравнение по длинам/префиксам создаёт лишнюю работу на commit. Typed per-instance plans позволят быстро заменить только затронутые индексы.

### Startup rebuild

Последовательное восстановление всех backend-ов может дать большой cold-start. После fail-closed semantics можно:

- параллелить CPU-bound rebuild через ограниченный scheduler/spawn_blocking;
- ограничивать I/O concurrency;
- публиковать таблицу только с Ready backend-ами;
- показывать progress и estimated completion.

### Что измерить перед оптимизацией

Минимальная матрица baseline:

- point get/insert/update/delete;
- batch 1/10/100/1000;
- hash/unique/sorted/FTS/vector query;
- write latency во время CREATE INDEX;
- transaction commit при 0/1/10 индексах;
- 1/8/32/128 concurrent clients;
- cold/warm restart с индексами;
- cursor memory и latency на 100k/1M rows;
- Windows отдельно, поскольку текущий CI сигнал именно там.

Для каждого теста нужны throughput, p50/p95/p99, max RSS, bytes written, WAL amplification и reproducible hardware profile.

## Release engineering

### Performance gate фактически не запускается

Файлы: `.github/workflows/perf-gate.yml:15-21,65`, `.github/workflows/release.yml:422-463`.

Workflow требует `[self-hosted, shamir-bench]`, но комментарии прямо говорят, что подходящей машины сейчас нет и job будет стоять в очереди бесконечно. В корне также отсутствует `bench-baseline.json`.

Все release artifacts зависят от этого job. То есть workflow формально строгий, но практически релиз невозможен.

До тега:

1. зарегистрировать закреплённый runner;
2. зафиксировать CPU governor/power plan, toolchain и фоновые процессы;
3. снять baseline с замороженного RC;
4. commit-нуть baseline;
5. прогнать perf gate на том же SHA;
6. задокументировать процедуру обновления baseline и допустимые variance bands.

### Версия и CHANGELOG конфликтуют

`CHANGELOG.md` содержит:

- `[Unreleased]` с новой волной (`:17`);
- уже существующий `[0.1.0-alpha.1] - 2026-07-26` (`:92`).

Версии Rust и TS всё ещё `0.1.0-alpha.1`. Если сейчас поставить tag `v0.1.0-alpha.1`, workflow найдёт старый heading и извлечёт июльские notes, а 99 новых коммитов останутся в Unreleased.

Нужно выбрать один вариант:

- если alpha.1 ещё никогда не публиковался — перенести Unreleased в alpha.1 и обновить дату;
- безопаснее — bump всех компонентов до `0.1.0-alpha.2` и создать точный heading alpha.2.

Node package всё ещё имеет `0.1.0`, что расходится с Rust/TS alpha.1.

### Не определён состав продукта

Все Cargo crates имеют `publish = false`, TypeScript package — `private: true`, а release workflow публикует главным образом server binaries. Это допустимо, если первый релиз сознательно server-only, но тогда документация должна прямо объяснять, как пользователи получают совместимый SDK.

Нужно решить:

- server-only release с vendored/examples clients;
- либо публикация Rust/TS/Node SDK;
- version compatibility matrix server ↔ protocol ↔ clients;
- что считается поддерживаемым публичным API.

Архивы намеренно не включают `THIRD_PARTY_LICENSES`; SBOM отдельно полезен, но third-party notices лучше положить внутрь каждого distributable archive.

### Security и операционный drill

После первого тега обновить `SECURITY.md`: supported versions, канал private disclosure, сроки реакции и политика alpha.

На замороженном SHA обязательно выполнить ручной release drill:

1. старт clean server и smoke CRUD/auth;
2. создать каждый тип индекса и проверить restart;
3. backup с manifest verification;
4. restore в пустой каталог;
5. старт после restore и контрольные queries;
6. проверка invalidation старых auth tickets;
7. симуляция прерывания restore/DDL в каждой durable phase;
8. upgrade/no-upgrade statement для alpha;
9. распаковка каждого release archive на чистой машине;
10. проверка checksum, signature, SBOM и license notices.

## Обязательный план до релиза

### Этап A — correctness freeze

1. Запретить новые возможности DDL/OQL до закрытия P0.
2. Починить registry publication watermark.
3. Ввести immutable index instance identity и полную замену stale tx plans.
4. Сделать единый global namespace имён.
5. Сделать registry mutation атомарной и startup metadata validation fail-closed.
6. Провести все DDL через один coordinator.
7. Добавить read-side grace period перед physical index GC.
8. Переводить не восстановившийся backend в Failed/Building, не Ready.

### Этап B — release usability

1. Вывести doctor/repair/status в CLI или admin API.
2. Реализовать typed DDL result/status contract.
3. Убрать panic-by-default из builders.
4. Сделать CreateIndex typed и strict.
5. Закрыть Windows integration failure.
6. Принять явное решение по CREATE INDEX stall: online build либо enforced alpha limits.

### Этап C — release candidate

1. Freeze SHA.
2. Полные `fmt`, `clippy -D warnings`, workspace tests и E2E на всех ОС.
3. Failure-injection tests lifecycle/recovery.
4. Зарегистрированный perf runner и committed baseline.
5. Backup/restore/upgrade drill.
6. Версии, CHANGELOG, release notes и package matrix.
7. Сборка, подпись и smoke всех артефактов с того же SHA.

## Минимальные новые regression tests

До снятия `NO-GO` нужны deterministic tests как минимум для следующих сценариев:

1. `CREATE A → DROP A → stage tx → CREATE B → commit` и проверка B.
2. Две out-of-order registry publications с tx snapshot между ними.
3. stage sorted op → RENAME → commit.
4. stage op → DROP → commit для всех четырёх families.
5. stage op → DROP → CREATE same name/new definition → commit.
6. duplicate name между regular/unique/sorted/index2.
7. duplicate index2 insert: отсутствие backend только-в-`by_id`.
8. startup с duplicate descriptors должен отказать или открыть их Failed, но не Ready.
9. `drop_all` failure при Building recovery не должен завершиться Ready.
10. `restore_on_open` failure vector/FTS не должен делать индекс planner-visible.
11. reader pause после backend selection → concurrent DROP → reader получает корректный snapshot.
12. параллельные CREATE/DROP/RENAME разных families сериализуются одним admission guard.

## Release bar

Считать alpha готовой можно, когда одновременно выполнены условия:

- все P0 закрыты кодом и deterministic regression tests;
- нет известных путей silent wrong result;
- текущий SHA зелёный во всех обязательных CI jobs без ручного rerun;
- perf gate реально выполнен на закреплённом runner;
- doctor/status доступны пользователю binary;
- DDL partial outcomes имеют машинно-читаемый contract;
- CREATE INDEX outage либо устранён, либо жёстко ограничен и явно подтверждается;
- version/CHANGELOG/release notes совпадают;
- документирован точный состав server/SDK release;
- backup/restore и crash-recovery drill пройдены на том же SHA.

## Методика и ограничения ревью

Просмотрены Git history новой волны, diff относительно предыдущего snapshot, реализации registry/lifecycle/recovery, транзакционный pre-commit, DDL handlers, builders, документация ограничений, checkpoints и release/performance workflows.

По требованию review был readonly относительно исходников и Git. Не выполнялись `cargo fmt`, `cargo clippy`, tests, benchmarks, server startup, fault injection или online CI inspection. Поэтому этот документ выявляет дефекты по статическому анализу и не заменяет обязательный release-candidate прогон.

На момент начала review в рабочем дереве уже находился незакоммиченный файл `docs/checkpoints/2026-08-05-2010.md`; он не изменялся.

