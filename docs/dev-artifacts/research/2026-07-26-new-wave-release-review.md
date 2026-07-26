# S.H.A.M.I.R. — readonly review новой волны перед `0.1.0-alpha.1`

Дата: 2026-07-26  
Проверенный HEAD: `5270aa92d4a3b3c663186883cb8acb00283cdd5a`  
База сравнения: `4d5436a0dbdb4c0ce0884457576aab093b01c337`  
Дельта: 30 коммитов, 127 файлов, `+8531/-708`

## 1. Вердикт

Новая волна существенно улучшила состояние проекта: исправления F-2, F-6,
F-7, F-8 и основная часть F-4/F-10/F-12 выглядят содержательно правильными,
покрытие тестами в изменённых областях заметно расширено, release workflow уже
намного серьёзнее обычного alpha-релиза.

Однако **тег `0.1.0-alpha.1` на текущий HEAD ставить пока рано**. Статический
аудит нашёл пять проблем, которые затрагивают correctness/security-контракт:

1. keyset-cursor считает колонку безопасной по текущей схеме, хотя установка
   схемы не валидирует уже существующие строки;
2. истёкший bootstrap-пароль остаётся пригодным для успешного входа, если
   ротация credential завершилась ошибкой;
3. schema DDL всё ещё может оставить catalogue и live validator в разных
   состояниях;
4. reverse-FK actions имеют документированное TOCTOU-окно и могут оставить
   нарушенную ссылочную целостность;
5. `SelectItem::Expression` принимается wire/parser-слоем, но молча
   игнорируется исполнителем.

Для первого **публичного alpha** не обязательно реализовывать JOIN, sharding,
durable subscriptions или production-ready online migration. Но нельзя
оставлять принимаемые API, которые молча дают неверный результат или обещают
ограничение, не обеспечиваемое при допустимом конкурентном сценарии.

Позиционирование в README сейчас выбрано верно: это не замена PostgreSQL,
MySQL, MongoDB, Redis или Memcached. Реалистичная ниша alpha — greenfield
embedded/self-hosted document DB с транзакциями, WASM, FTS/vector search и
защищённым сетевым доступом. Это позиционирование не следует расширять до
исправления перечисленных ниже проблем и накопления эксплуатационных данных.

## 2. Метод и ограничения ревью

Ревью выполнено строго в readonly-режиме:

- прочитаны Git history/diff от `4d5436a0` до HEAD;
- проверены конечные ветки исполнения, а не только commit messages и briefs;
- просмотрены DDL/OQL DTO, engine paths, Rust/TS builders, cursor/runtime
  limits, backup/restore, release/CI и публичные документы;
- сборка, `cargo fmt`, clippy, тесты и benchmark не запускались по прямому
  требованию пользователя;
- единственное изменение рабочего дерева — этот отчёт.

Следовательно, отчёт подтверждает статическую корректность/некорректность
контуров, но **не заменяет зелёный release gate на точном SHA**.

## 3. Сводка находок

| ID | Приоритет | Область | Вывод |
|---|---:|---|---|
| R1 | P0 | cursors/schema | F-1 опирается на schema type, но старые строки после `SET SCHEMA` могут ему не соответствовать |
| R2 | P0 | auth/bootstrap | F-3 сохраняет retryability, но остаётся fail-open: истёкший token может успешно войти при ошибке rotation |
| R3 | P0 | schema DDL | persist/interner/activate не образуют атомарную state transition; rollback покрывает не все этапы и только catalogue |
| R4 | P0 | FK correctness | `RESTRICT`/`CASCADE`/`SET NULL` планируются до tx и допускают concurrent orphan |
| R5 | P0 | OQL | `SelectItem::Expression` парсится, но silently ignored |
| R6 | P1 | cursor lifecycle | F-9 оставляет check-then-remove race между `expired_ids` и reaper removal |
| R7 | P1 | corruption | F-10 покрывает не все read paths; часть строк всё ещё исчезает или превращается в `Null` без diagnostic |
| R8 | P1 | memory limits | finite budget добавлен в small/medium profiles, но default и основной example остаются unbounded |
| R9 | P1 | backup durability | fsync файлов не гарантирует durability новых directory entries; ошибки dir fsync при restore не влияют на success |
| R10 | P1 | release process | changelog gate проверяет наличие любого heading, а не heading текущего tag |
| R11 | P2 | docs/public repo | `KNOWN_LIMITATIONS`, README, AGENTS и tracked dev artifacts местами рассинхронизированы с кодом |
| R12 | P2/perf | cursors | cursor повторяет полный pinned scan на каждой странице; null probe тоже scan-heavy |

## 4. P0 — исправить до тега

### R1. Schema-based keyset gate не доказывает однородность существующих данных

F-1 разрешает keyset только для schema-typed `Int/Bool/String/Bin`:

- `crates/shamir-server/src/db_handler/cursor_handlers.rs:475`
- accepted type set: `cursor_handlers.rs:508`
- применение gate: `cursor_handlers.rs:1227-1249`

Проблема в доказательстве: `set_table_schema` и `add_schema_rule` валидируют
DTO/index requirements, компилируют validator и привязывают его к **будущим**
writes, но не сканируют и не валидируют уже существующие строки:

- `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:298-447`
- live binding: `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:477-519`

Контрпример:

1. schemaless table уже содержит `score: Int` и `score: Str`;
2. оператор устанавливает schema `score: Int`;
3. старый `Str` не переписывается и не отклоняет DDL;
4. cursor видит schema `Int`, включает keyset;
5. boundary comparison не сопоставляет старый `Str` с `Int` seek key;
6. строка может исчезнуть из последующих страниц.

То есть F-1 закрывает mixed-type только для таблицы, где доказано, что **вся
видимая snapshot-история** прошла этот validator. Текущий metadata bit этого
не доказывает.

Что сделать:

- самый безопасный alpha-fix: отключить keyset и использовать offset до
  engine-level cursor;
- либо добавить `validate_existing: true`/mandatory validation scan при
  активации schema и хранить durable marker вроде
  `schema_validated_through_version`;
- keyset допустим только если pinned snapshot version не новее доказанного
  validated-through version и нет legacy rows вне этого контура;
- добавить e2e: mixed pre-schema rows → set homogeneous schema → полный drain
  cursor без потерь/дубликатов.

Дополнительно `Bin` лучше убрать из accepted keyset types: downstream
`safe_seek_key` всё равно деградирует Bin в offset, поэтому сейчас Bin лишь
оплачивает лишний null probe.

### R2. Bootstrap TTL остаётся fail-open при ошибке ротации

F-3 правильно изменил порядок на rotate → consume → delete и оставляет
metadata для retry:

- boot sweep: `crates/shamir-server/src/server/server_launcher.rs:186-216`;
- login path: `crates/shamir-server/src/connection/handshake.rs:401-456`.

Но proof проверяется раньше (`handshake.rs:306-396`), а при rotation error
код прямо требует не прерывать успешный login (`handshake.rs:405-408`).
Проверки `bootstrap_token_expired(now)` до принятия proof в login path нет.

Следствие: если token истёк, а boot-time rotation не сработала из-за storage
ошибки, сервер стартует; владелец истёкшего token может пройти SCRAM. Если
post-login rotation снова падает, session всё равно создаётся. Metadata
retryable, но TTL security guarantee в этот момент не действует.

Что сделать:

- до proof acceptance определить, относится ли credential к outstanding
  bootstrap username и истёк ли token;
- expired bootstrap credential должен fail-closed с generic auth error;
- rotation можно ретраить при boot/login, но не выдавать session на
  credential, срок которого истёк;
- отдельно определить поведение при недоступности metadata: для auth boundary
  безопаснее fail-closed;
- тест: expired token + forced rotation failure + реальный login = reject;
  после успешной rotation старый token также reject.

### R3. Schema DDL всё ещё не атомарен

F-4 правильно добавил strict parsing перед catalogue write и compensating
catalogue rollback после activation error. Но pipeline остаётся:

1. `save_table_meta(&rec)` — `admin_schema.rs:413`;
2. `interner_mgr.persist()` — `admin_schema.rs:420-423`;
3. `compile_table_schema()` — `admin_schema.rs:433-446`.

Две незакрытые развилки:

- если `interner.persist()` падает, метод возвращает error без rollback:
  catalogue уже содержит новую schema, live validator ещё старый;
- `compile_table_schema` сначала заменяет/registers validator
  (`schema_management.rs:488-495`), затем добавляет binding
  (`schema_management.rs:498-512`). Ошибка binding означает частично
  изменённый live registry. Внешний rollback возвращает старый catalogue, но
  не возвращает старый validator artifact.

Аналогичный порядок есть в add/remove rule:

- `admin_schema.rs:565-592`;
- `admin_schema.rs:688-715`.

Что сделать:

- превратить activation в prepare/commit:
  `parse + compile detached validator + verify table/binding writes` до
  публикации live artifact;
- persist interner до catalogue commit либо включить оба состояния в durable
  transaction/journal;
- live registry менять одним final swap после всех fallible steps;
- rollback должен восстанавливать и catalogue, и live artifact/binding;
- при невозможности общей транзакции хранить durable schema state
  `pending/active/failed` и на boot детерминированно завершать recovery;
- fault-injection tests на каждом шаге: catalogue write, interner persist,
  registry register/replace, table binding persistence.

### R4. FK constraint допускает concurrent orphan

Код сам документирует, что reverse-FK discovery/check выполняется до atomic
tx scope:

- RESTRICT: `crates/shamir-engine/src/query/batch/fk_restrict.rs:9-18`;
- CASCADE/SET NULL: `crates/shamir-engine/src/query/batch/fk_actions.rs:17-34`;
- ON UPDATE: `crates/shamir-engine/src/query/batch/fk_on_update.rs:52-59`.

Сценарий:

1. parent delete/update plan проверяет детей — их нет;
2. конкурентная tx вставляет child, ссылающийся на ещё существующего parent;
3. parent tx коммитит delete/rekey;
4. child остаётся orphan либо пропускает declared action.

Это не просто “не Serializable”: это нарушение заявленного FK invariant.
Размер race window не делает constraint корректным.

Что сделать:

- reverse lookup и action planning выполнять внутри той же tx;
- включить read/predicate dependency на child FK range либо сериализовать
  parent action с child FK insertion на общем keyed gate;
- для index-backed FK использовать range/version validation на posting set;
- добавить deterministic concurrency tests с barrier между plan и commit;
- до исправления явно пометить FK actions как experimental или убрать
  обещание строгой referential integrity из public docs.

### R5. `SelectItem::Expression` молча игнорируется

Surface существует и принимается:

- DTO: `crates/shamir-query-types/src/read/select_expr.rs:7`;
- parser: `crates/shamir-engine/src/query/read/parser.rs:190`;
- TS public type: `crates/shamir-client-ts/src/core/types/query.ts:66`.

Но обычная projection обрабатывает только Field/Function и проглатывает
остальное `_ => {}`:

- `crates/shamir-engine/src/query/read/select_projection.rs:87-108`.

Aggregate path также явно игнорирует expression:

- `crates/shamir-engine/src/query/read/aggregate.rs:1001`.

Это silent wrong result: валидный по wire shape запрос не получает ни
вычисленного поля, ни typed error.

Что сделать перед релизом:

- минимальный безопасный вариант: parser/planner rejects expression с
  `select_expression_not_supported`;
- убрать/пометить internal соответствующий TS public union arm;
- затем отдельно реализовать expression evaluator и builders;
- tests должны проверять либо правильное значение Add/Sub/Mul/Div, либо
  стабильный explicit error — никогда `{}`/пропуск.

## 5. P1 — желательно закрыть перед alpha.1, минимум честно задокументировать

### R6. Cursor reaper fix неполный

F-9 проверяет `state().try_lock()` при сборе expired IDs, но удаление идёт
отдельным проходом:

- residual прямо описан: `crates/shamir-server/src/cursor_registry.rs:483-492`;
- collect: `cursor_registry.rs:494-499`;
- later remove: `cursor_registry.rs:572-578`.

Новый fetch может взять lock после collect и до remove. Reaper удалит registry
entry и snapshot pin во время активного fetch. Следует объединить
expired-check, lock acquisition и conditional removal в одну операцию либо
ввести atomic `in_flight` lease checked при removal.

### R7. Corrupt-record reporting остаётся неполным и неоднородным

F-10 — полезное улучшение, но текущие остатки:

- `try_project_page_only_bytes` silently skips:
  `crates/shamir-engine/src/table/read_exec.rs:2561-2564`;
- `apply_select_value_bytes` превращает decode error в `QueryValue::Null`:
  `read_exec.rs:2603-2608`;
- streaming filter исключает malformed row без diagnostic:
  `crates/shamir-engine/src/table/table_manager_streaming.rs:319-324`;
- `read_index_scan.rs`/`read_temporal.rs` вызывают helper, у которого нет
  канала для `corrupt_records`.

Это особенно опасно для aggregate/count и pagination metadata: разные plans
могут по-разному учитывать одну и ту же corruption.

Нужен общий `DecodedRow::{Valid, Corrupt{id}}`/diagnostic accumulator для всех
plans. Для API стоит определить policy: partial success + diagnostics либо
hard error в strict mode.

### R8. Global in-flight budget всё ещё unbounded по умолчанию

Small/medium examples теперь задают 128/256 MiB:

- `deploy/server.small.example.ktav:91-95`;
- `deploy/server.medium.example.ktav:91-95`.

Но config default остаётся `None`:

- `crates/shamir-server/src/config.rs:370-385`.

Основной `deploy/server.example.ktav` параметр не содержит. Поэтому самый
очевидный путь установки сохраняет прежнюю worst-case модель
`connections × max_result_size`.

Рекомендация:

- finite safe default, зависящий от выбранного profile;
- добавить параметр в основной example;
- startup warning при `None`;
- метрики current/peak reserved bytes, wait count/time и rejects;
- разделить execution-memory budget и response-byte budget: текущая
  pessimistic reservation ограничивает concurrency, но не все промежуточные
  allocations sort/group/vector paths.

### R9. Backup/restore durability улучшена, но обещание шире реализации

F-12 вызывает `sync_all()` на скопированных файлах и manifest. Это защищает
file contents, но новая directory entry тоже должна быть durable. Backup path
создаёт directories/files и не fsync-ит содержащие directories; manifest
комментарий сознательно не делает directory fsync.

Restore fsync-ит parent после rename только на Unix, но failure лишь логируется,
и method всё равно возвращает success:

- `docs/guide-docs/KNOWN_LIMITATIONS.md:415-445`;
- `crates/shamir-server/src/restore.rs:338-399`.

Для alpha допустимо при точной формулировке “best effort against power loss”,
но не “crash-durable success”. Лучше:

- fsync destination directories bottom-up после copy;
- fsync snapshot root после manifest creation;
- дать strict mode, где dir fsync error возвращается caller;
- power-cut testing делать отдельным VM/filesystem harness, не unit test.

### R10. Release workflow допускает tag без соответствующей changelog section

`version-consistency` правильно сравнивает tag со всеми crate versions, но
CHANGELOG check принимает `[Unreleased]` **или любой version heading**:

- `.github/workflows/release.yml:301-365`.

Теперь реальная section `0.1.0-alpha.1` уже есть, поэтому loose backward-
compatibility больше не нужна. Проверять надо точный
`## [${TAG_VERSION}]`. Иначе будущий `v0.1.0-alpha.2` может пройти с notes
только от alpha.1 и получить fallback release notes.

Также комментарии workflow всё ещё говорят, что changelog содержит только
`[Unreleased]`; это уже неверно.

## 6. Что новая волна сделала правильно

### F-2 — единый numeric comparator

Хорошее исправление. Exact Int/F64 comparison вынесено в единый
`numeric_cmp`, подключено к bytes path, filter node, resolve, ORDER BY и
MIN/MAX aggregate. Это закрывает прежнее расхождение fast/slow plans.
Отдельный большой cross-path test module — правильный способ удержать parity.

Остаток `Big ↔ F64` остаётся approximation по дизайну; это допустимо только
при сохранении явной numeric-semantics документации.

### F-4 — strict schema parsing

Unknown/malformed `array_of`, `format`, `compare`, FK fields/actions больше не
должны silently collapse в `None`/default. Precompile до catalogue write —
правильное направление. Не хватает именно полной state-machine atomicity,
описанной в R3.

### F-6 — `count_total` и top-K

Fast path теперь исключается при `count_total=true`, поэтому total больше не
теряется. Это корректный узкий fix.

### F-7 — `with_version`

Collapse paths отклоняются, а plain ORDER BY протаскивает RecordId/version.
Лучше explicit rejection, чем records без соответствующих versions.

### F-8 — `u64`

`serialize_u64` переводит значения выше `i64::MAX` в `Big`, а не wrap. Это
соответствует заявленному wire numeric contract.

### F-10 — corruption diagnostics

Добавление `QueryResult.corrupt_records` обратно совместимо по serde и полезно
операционно. Нужно только довести policy до всех plans (R7).

### F-11 — deploy profiles

Small/medium profiles теперь не оставляют RI-15 полностью выключенным. Это
хороший operational default для этих двух файлов, хотя общий default ещё
нужно закрыть (R8).

### F-12 — backup/restore

File content fsync и parent fsync после rename заметно уменьшают power-loss
окно. Ошибки swap теперь лучше диагностируются, temp cleanup стал аккуратнее.
Остаток сформулирован в R9.

### F-13 — builder safety

Rust write/DDL builders больше не используют `expect`/`Null` как missing-field
sentinel; typed `BuilderError` — правильный API. `Query::try_build` закрывает
having/page parity без массовой поломки call sites.

Для следующего breaking alpha лучше поменять приоритет: safe validation должна
быть обычным `build()`, а permissive path — явно `build_unchecked()`.

### F-15 — live-server migration opt-in

Config flag теперь реально включает тот же `Arc<ShamirDb>`, которым пользуется
wire handler. Default false и WARN при включении — правильно.

Но `KNOWN_LIMITATIONS.md:100-104` всё ещё утверждает, что live server этот
toggle никогда не вызывает. Документ надо обновить. Саму migration нельзя
называть production-ready: write interception, durable coordinator,
byte-level verification и durable destination backend всё ещё отсутствуют.

## 7. DDL: что развивать

### Обязательно/ближайшее

1. **Schema activation state machine** — R3.
2. **Validate existing data / dry-run**:
   `SET SCHEMA ... VALIDATE`, async status, список violating record IDs,
   `NOT VALID` + later `VALIDATE CONSTRAINT`.
3. **Safe schema evolution**:
   add/rename/drop field с declarative transform/backfill, resumable progress,
   rollback marker и versioned schema.
4. **FK concurrency correctness** — R4.
5. **DDL idempotency и CAS везде**:
   `expected_schema_version`, operation id/idempotency key, typed conflict.
6. **Explicit unsupported rejection**:
   ни один persisted-but-not-enforced option не должен приниматься молча.

### После alpha.1

- composite unique/index/FK;
- partial indexes;
- TTL indexes/real record expiration (не путать с buffer `ttl_ms`);
- rename table вместе с declarative validator identity;
- transactional DDL или durable DDL job model;
- schema/index introspection с state, progress, last error;
- online index build с snapshot + write catch-up;
- storage migration только после shadow write interception, durable recovery и
  content hash/byte verification.

## 8. OQL: что развивать

### Сначала исправить контракт

- reject или реализовать `SelectExpr` (R5);
- server-side semantic validation для `page=0`, `page_size=0`,
  `HAVING` shape, keyset tuple/order compatibility — builders не являются
  trust boundary;
- единые typed errors для unsupported combinations;
- parity tests, выполняющие один logical query через full scan, legacy index,
  index2, temporal и cursor paths.

### Наиболее полезное расширение

1. computed SELECT expressions на базе уже существующего universal
   `FilterValue`/function registry, а не второго expression language;
2. `EXPLAIN ANALYZE` с actual rows/time/bytes, не только plan preview;
3. explicit semi-join/lookup primitive для common relational cases;
4. subquery exists/in через typed query refs;
5. aggregate/window roadmap: global HAVING semantics, window rank/partition;
6. query cancellation/deadline propagation в storage и CPU-bound tasks;
7. strict mode: corruption/precision/unsupported feature policies.

Полный SQL parity до первого alpha не нужен. JOIN/UNION/window functions
следует добавлять после стабилизации planner contracts и memory budgets.

## 9. Query builders: что расширить

Текущая Rust/TS parity стала значительно лучше; старые coverage documents в
`docs/dev-artifacts/research/coverage-*.md` уже частично устарели: TS теперь
имеет typed `Handle`, `tryBuild` validation, interner DDL, computed writes и
subscription call delivery.

Приоритеты:

1. сделать validation default в Rust на следующем breaking alpha;
2. общий cross-language fixture для **всех** wire operations, а не отдельных
   replication/vector shapes;
3. builder для computed SELECT только после engine implementation;
4. validation:
   empty aliases/paths, duplicate aliases, zero limits, invalid vector dims,
   keyset key arity, incompatible cursor temporal/version flags;
5. typed `BuildError` с machine-readable codes и field path вместо plain TS
   `Error`;
6. generated capability matrix из DTO/builders/tests, чтобы coverage reports
   не устаревали вручную;
7. deprecation path для permissive methods с release-note migration examples.

Не стоит расширять fluent API десятками синонимов до стабилизации semantics:
лучше меньше методов, но одинаковый wire output и одинаковые validation errors.

## 10. Производительность

### Самый большой архитектурный долг

Cursor не является engine cursor: каждая страница повторно выполняет полный
`AsOf` scan и заново сортирует/агрегирует материализованный page result.
Это честно описано в `KNOWN_LIMITATIONS.md:185-195` и CHANGELOG.

Следующий значимый perf milestone:

- engine-level snapshot iterator/token;
- держать не borrowed Rust stream, а owned plan state + storage iterator
  bookmark;
- для ORDER BY использовать sorted-index seek `(key, RecordId)`;
- offset mode не должен повторно проходить уже пропущенный prefix;
- отдельные limits на cursor CPU/scan bytes/lifetime pinned bytes.

### Ближайшие оптимизации

- исключить Bin из schema-keyset eligibility;
- null existence probe должен short-circuit на первой строке на scan layer,
  а не материализовать match set и применять LIMIT позднее;
- добавить spill/bounded algorithms для GROUP BY, DISTINCT и large sort;
- учитывать intermediate memory, не только serialized response bytes;
- `count_total` отделять от page read и по возможности брать из index/count
  plan;
- corruption diagnostics собирать без повторного decode;
- benchmark matrix должна включать selectivity, skew/tie runs, wide rows,
  cold/warm cache, concurrent writers и cursor drain, а не только single-op
  throughput;
- перед заявлениями “high-performance” опубликовать воспроизводимые команды,
  hardware, dataset, p50/p95/p99 и сравнение только в корректно совпадающих
  use cases.

## 11. Public release/repository hygiene

### Документация

- README верно отказывается от заявления “replacement for everything”.
- README всё ещё говорит `Create/Drop User / Role`, хотя role objects были
  удалены; остались role labels + grant/revoke.
- README “published binaries are not available yet” надо изменить в том же
  commit, который готовит tag/release.
- `KNOWN_LIMITATIONS` надо обновить для F-15, R1, R4, R5, R6, R7 и R9.
- AGENTS.md заявляет 10 default crates, README — 23; фактический workspace
  сейчас 23 crates (24 Cargo manifests минус excluded `shamir-client-node`).
  Инструкции для contributors устарели.

### Репозиторий

- tracked `bench-iters.txt` — локальный benchmark artifact, его лучше удалить
  из release branch или перенести в подписанный benchmark report;
- tracked `docs/dev-artifacts` содержит около 716 файлов, включая около 393
  prompt briefs и множество checkpoints. Для публичной репы это ухудшает
  signal/noise и раскрывает внутренний процесс сильнее, чем помогает
  пользователю. Разумнее оставить curated ADR/research/roadmap, а prompts и
  in-flight checkpoints вынести в archive branch/отдельную private history;
- локальный tag `backup/pre-history-rewrite-2026-07-14` не соответствует
  release naming `v*`; убедиться, что он не публикуется вместе с public refs;
- текущие untracked `docs/checkpoints/2026-07-25-2152.md`,
  `docs/checkpoints/2026-07-26-0230.md` и `vitest.log` не включать в release.

### Release artifacts

Сильные стороны workflow:

- fmt/clippy/tests на Linux/Windows/macOS;
- TS unit + live server e2e;
- version consistency;
- Linux x86_64, macOS aarch64, Windows x86_64;
- checksums, CycloneDX SBOM, keyless cosign;
- prerelease GitHub release.

До тега:

- сделать exact changelog heading gate (R10);
- добавить `README`, licenses, default config и upgrade/known-limitations note
  внутрь archive — сейчас archive содержит только binary;
- проверить clean-machine smoke распакованного archive;
- проверить размер `<50 MB` как machine gate, если это public promise;
- желательно добавить Linux aarch64; x86_64 macOS — по спросу;
- зафиксировать supported OS/libc/minimum CPU features;
- release должен зависеть от supply-chain deny/audit policy либо иметь
  documented waiver.

## 12. Рекомендуемый порядок работ

### Gate A — correctness/security

1. R2 bootstrap expiry fail-closed.
2. R1 disable/repair schema-keyset proof.
3. R3 schema activation transaction/state machine.
4. R4 FK TOCTOU либо explicit experimental downgrade.
5. R5 reject unsupported SelectExpr.

### Gate B — operational safety

6. R6 atomic cursor reaper lease.
7. R7 uniform corruption policy.
8. R8 finite default in-flight budget.
9. R9 accurate/strict backup durability semantics.

### Gate C — public release polish

10. update `KNOWN_LIMITATIONS`, README, AGENTS and builder coverage docs;
11. clean tracked/untracked artifacts;
12. tighten release changelog gate and archive contents;
13. run full gates on exact release SHA;
14. clean-machine install/start/auth/CRUD/backup/restore smoke;
15. only затем создать signed `v0.1.0-alpha.1` tag.

## 13. Release checklist на точном SHA

Обязательные repo gates:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
./scripts/test.sh --full --locked
```

Дополнительно:

- TS typecheck/unit/e2e с реально собранным release server;
- Rust client live e2e;
- backup → destructive change → restore → checksum/query verification;
- restart/crash-recovery matrix для Fjall/WAL;
- concurrency repro tests для R1/R2/R3/R4/R6;
- release archive smoke на всех трёх OS;
- `cargo deny`, `cargo audit`, SBOM generation;
- secret scan по **всей публикуемой Git history**, не только HEAD;
- verify tag version, exact changelog heading, signatures и checksums;
- сохранить CI run URLs и benchmark baseline в release evidence.

## 14. Итог

Новая волна — не “сделали зря”: большинство исправлений точные и полезные,
а release infrastructure уже сильная. Но несколько fixes закрыли исходный
симптом, оставив более узкое окно того же класса:

- F-1 доказал schema shape, но не legacy data;
- F-3 сделал rotation retryable, но не сделал expiry fail-closed;
- F-4 защитил parse/activation error, но не все промежуточные failures и не
  live rollback;
- F-9 защитил long-running fetch в основном окне, но не атомаризировал reap;
- F-10 добавил diagnostics, но не дал общей policy всем plans;
- F-12 fsync-ит contents, но не все directory entries;
- F-15 подключил opt-in, но docs остались в прошлом состоянии.

После Gate A проект можно выпускать как честный, интересный
`0.1.0-alpha.1` с явно ограниченной областью применения. До этого текущий
HEAD лучше считать release candidate для ещё одной короткой correctness-wave,
а не финальным release commit.
