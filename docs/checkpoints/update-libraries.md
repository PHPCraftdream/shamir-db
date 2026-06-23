בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-23 [update-libbraries]

## Session summary

Реализуем декларативные схема-валидаторы (третий вид валидаторов, задаётся данными) по
дизайну в `docs/design/declarative-schema-validators/`. Сделано и закоммичено: **Phase 0**
(узкая роль `RecordValidator` + by-name `RecordFields`, миграция native/wasm — `5b1955b`,
review-notes `fdab32a`) и **Phase A движок** (declarative schema vertical: SchemaValidator,
TypeTag/Constraints/FieldRule, rule_builder, builtin-checks, хранение/boot/DDL-скелет,
TS-клиент — `44b352c`). Запускал автономный o46l-workflow на A→B→C, но **Anthropic API
ушёл в устойчивый 500-outage** (все субагенты падают мгновенно, 0 токенов; resume только
реплеит кэш с провалами). Переключился на **crush** (CLI, свой провайдер zai/glm — не
затронут 500). crush сделал **Phase B движок** (scalar-bridge/format/cross-field +
`ValidatorCtx.scalars` + резолвер на write-path; новые `format.rs`/`cross_field.rs`;
23 unit + 6 e2e — мой независимый прогон зелёный).

**КЛЮЧЕВАЯ НАХОДКА моего zero-trust ревью:** declarative-schema **серверное исполнение
DDL застаблено** — `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`: все 4 хендлера
(set_table_schema/add/remove/get) были authz-only заглушками (`{ok:true}`, TODO, ничего
не персистят). Из-за этого схему нельзя создать через API, она не пишется в каталог,
`boot_compile_schemas` (вызван из core.rs:403) её не восстановит. Инфра (parse_schema,
compile_table_schema, boot, drop) в `schema_management.rs` — **реальна, но мертва** без
write-стороны. Все e2e (A и B) обходили это через `register_and_bind_schema*` (native-
регистрация). Phase A была закоммичена с этим застабленным слоем (disclosed TODO, но уехало
в «Phase A done»). Юзер выбрал «сначала аудит» → я составил карту слоёв (движок реален,
DDL/persist застаблен) → юзер дал go → **делегировал crush раскрыть persist-слой**: реализовать
4 хендлера (persist schema+schema_validator_id+schema_version под per-table lock, intern путей,
expected_version, вызов compile_table_schema), провести Phase B поля через ConstraintsDto/
query-builder/ddl.ts, + честный round-trip reopen e2e (НЕ через register-helper).

**В ПОЛЁТЕ СЕЙЧАС:** crush run `b035i5ee4` (session `declarative-phaseB`, alive) — раскрывает
persist-слой. Первый persist-прогон (`bje4ou3nz`) оборвался на ошибке внутреннего crush
`agent`-tool (он спавнит субагентов в Anthropic → 500); перезапустил с директивой «не
использовать agent-tool, исследуй сам view/grep» — сейчас реально правит admin_schema.rs,
schema_ops.rs, query-builder. Жду нотификацию; потом МОЙ ключевой тест: сам создам схему
через DDL → перезапишу → **переоткрою БД** → проверю что схема применяется (доказательство,
что заглушки раскрыты), затем gate + единый коммит.

**Параллельный запрос юзера — Dependabot-бампы** (7 PR на github PHPCraftdream/shamir-db).
Дал триаж (ниже в Decisions). Главное: **bincode 1.3.3→3.0.0 — ОПАСНО** (переписанный API +
смена on-disk формата → durability-риск; рекомендую отклонить). Ничего НЕ бампаю без явного
go (правило) и не сейчас (crush мид-флайт правит дерево).

Бранч master. НЕТ активных /loop или /babysit кронов (снял o46l-retry-крон `a637c890` и
babysit `cdf9acf4`, чтобы не было параллельных workflow в гонке с crush). origin/master =
6dd135f (старые docs/parity/checkpoints уже на origin); непушены 3 коммита Phase 0/A.

## Active goal

None (`/goal` не установлен). Прогресс ведётся через crush-run + TaskList, без Stop-hook.

## TaskList
### in_progress
- #197 Phase B: scalar-bridge + format + cross-field checks (движок готов в дереве, uncommitted)
- #202 Schema server-execution: раскрыть застабленные DDL-хендлеры + persist + round-trip reopen e2e (crush b035i5ee4)
### pending
- #195 Phase 0 follow-up: function record-navigation by-name (doc 08 item 6) (blockedBy #197)
- #201 Phase C (АСПИРАЦИОННА): relational foreign_key + unique (blockedBy #195)
### recently completed
- #191 Phase 0 core validator-plane refactor
- #192 Phase 0 gate verify
- #193 Phase 0 adversarial review
- #194 Phase 0 atomic commit
- #196 Phase A declarative schema vertical (NB: серверное исполнение DDL было застаблено — закрывается в #202)
- #198 Review-note 3: сузить ValidatorCtx interner
- #199 Review-note 2: doc 08 async+Option
- #200 Review-notes: gate + commit

## Decisions
- **Phase C — последней, feasibility-gated** (нужен tx-scoped read-only validator db-handle, НЕ
  autocommit DbGateway). Если упрётся — A/B/func-nav уже зафиксированы.
- **Anthropic 500 → перешли на crush** (zai/glm-провайдер). o46l-workflow и retry-крон сняты.
  resume бесполезен (реплеит кэш с провалами) — для recovery нужен свежий прогон.
- **crush agent-tool падает** (спавнит субагентов в Anthropic → 500) → директива crush «не
  использовать agent-tool, исследовать самому».
- **Phase A «done» было неполным** — серверное исполнение DDL застаблено; заведена #202 чтобы
  раскрыть. Движок реален, persist — нет.
- **Dependabot триаж:** взять #5 quote / #4 futures-util / #2 checkout (🟢); проверить #1
  rust-toolchain 1.93→1.100 (gate на новом); отдельными задачами #6 rand 0.8→0.9 (breaking, 5
  крейтов/7 файлов) и #7 tungstenite 0.24→0.29 (breaking + конфликт: PR бампает только
  tungstenite, а tokio-tungstenite 0.24 пинит 0.24 → не соберётся, нужен локстеп); **отклонить
  #3 bincode 1.3.3→3.0.0** (durability-риск: смена on-disk формата + 34 call-site).

## Open questions
- **Какие Dependabot-бампы брать?** Жду решения юзера по каждому (правило: версии — только с
  явного go). Рекомендация — выше. Делать ПОСЛЕ схемной работы (crush мид-флайт).
- **Пуш?** Непушены 3 коммита Phase 0/A (5b1955b/fdab32a/44b352c) — origin/master=6dd135f.
  Жду явного «пуш».
- **func-nav (#195) + Phase C (#201)** — после закрытия #202 (persist) и коммита A+B.

## Repo state
```
 M crates/shamir-db/src/shamir_db/execute/admin_schema.rs        (crush раскрывает заглушки)
 M crates/shamir-db/src/shamir_db/shamir_db/mod.rs
 M crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs
 M crates/shamir-db/tests/declarative_schema_e2e.rs
 M crates/shamir-engine/src/table/table_manager_validators.rs
 M crates/shamir-engine/src/validator/record_validator.rs
 M crates/shamir-engine/src/validator/schema/{constraints,field_rule,mod,rule_builder,schema_validator}.rs
 M crates/shamir-engine/src/validator/schema/tests/mod.rs
 M crates/shamir-query-builder/src/batch/batch.rs
 M crates/shamir-query-builder/src/ddl/schema.rs
 M crates/shamir-query-types/src/admin/{mod,types/mod,types/schema_ops}.rs
?? crates/shamir-engine/src/validator/schema/cross_field.rs
?? crates/shamir-engine/src/validator/schema/format.rs
?? crates/shamir-engine/src/validator/schema/tests/phase_b_tests.rs
?? docs/checkpoints/  (this + 2026-06-23-declarative-schema-design.md)
```
(Дерево грязное — Phase B движок + crush-персистентность в полёте, uncommitted. crush b035i5ee4 alive.)

```
44b352c feat(schema): Phase A — declarative per-table schema vertical
fdab32a chore(validator): address Phase 0 review notes — narrow ValidatorCtx + doc sync
5b1955b refactor(validator): узкая роль RecordValidator + by-name RecordFields
6dd135f docs(design): declarative schema validators — per-table schema vertical
bacab51 docs(checkpoints): perf-hunt roadmap + parity campaign session checkpoints
```
