בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-02 [replication-design]

## Session summary

Продолжение от чекпоинта `2026-06-30-sefer-alloc-rollout.md`. Сессия
целиком посвящена букве **I** (Interconnected) — исследование, дизайн и
предподготовка кода под leader→follower репликацию (Movement C step 3 из
`docs/roadmap/PLAN.md`). Итог — четыре коммита за сессию, ноль
in-flight, две активные pending-таски (#365, #366).

**Основной результат — два дизайн-документа + один pre-refactor-план:**

- `docs/roadmap/REPLICATION.md` (~595 строк) — тот самый док, который
  PLAN.md резервировал «to be written when replication starts».
  Академический фундамент (Alsberg 1976 / Schneider 1990 / VR / PacificA /
  Kafka / DDIA), инвентаризация готового (changefeed уже replication-ready
  благодаря #177/#179), выбор модели (logical row-based repl, lazy master
  pull-first, epoch-fencing с первого дня), протокол 5 ops поверх
  существующего shamir-connect, QOL, фазы R0–R4. Итерации по обратной
  связи user'а породили §5.4 (модель доверия — «подключился→автоматически
  всё» explicitly отвергнуто), таблицу rw/ro × cluster/replica/peer в
  §3.2, и §5.5 — publication/subscription-модель PostgreSQL-стиля со
  streams (scope+direction+mode) и именованными ReplicationProfile-
  шаблонами на node accounts. Ключевое открытие §5.5: SystemStore — это
  обычный repo `system` с обычными таблицами, значит «реплицировать
  аккаунты» = включить `system/users` в scope, не отдельный механизм;
  push-потоки → лидерство per (db, repo), не per узел (edge-collect).

- `docs/research/REPLICATION-PRE-REFACTOR-2026-06-30.md` (~199 строк) —
  Fowler preparatory-refactoring. Разведка по коду закрыла все 5
  verify-пунктов §7.1 REPLICATION.md (таска #364, теперь удалена):
  V1 журнал append-only, retention нет ⇒ `journal_floor ≡ 0`; V1a
  SystemStore пишет через `execute_set_tx`/`run_implicit_batch_tx` ⇒
  changefeed на repo `system` должен работать из коробки, репликация
  аккаунтов = обычный data-поток в R1 (95% уверенности, остаток — PR0
  смоук); V2 interner уже ведёт `persisted_high_water` под дельта-синк;
  V4 идемпотентность apply = O(1) по repo-watermark'у; V5 dispatch/
  config/метрики/e2e готовы as-is. План PR0–PR5:
  - PR0 RED→GREEN смоук «create_user → событие в system changelog»
  - PR1 `BatchOp::is_write()` симметрично `is_admin()` (для read-only гейта)
  - PR2 `SessionPermissions::has_role()` хелпер
  - PR3 (единственный /opti-цикл) — выделить `finalize_commit` ядро из
    4 дублирующих commit-путей (`commit_tx_inner_legacy_async`,
    `commit_tx_lockfree`, `run_single_tx`, group_commit); блокирует R1,
    НЕ блокирует R0
  - PR4 `NodeMode {ReadWrite, ReadOnly}` гейт в единой точке
    `ShamirDbHandler::execute()`, default ReadWrite → нулевое изменение
    поведения до R1
  - PR5 wire-решение `DbRequest::Repl(ReplRequest)` как один вариант

**Прочие ходы сессии:**

- `deps: sefer-alloc local path → crates.io 0.2.1` (коммит `f22edd62`) —
  0.2.1 опубликован на crates.io (0.2.0 yanked), переключил оба места
  (shamir-server production + shamir-db optional bench-dep) с
  `path = "D:/dev/rust/sefer-alloc"` на registry. Cargo.lock подтверждает
  `source = registry`. Заодно снят устаревший perf-каваэт «mimalloc
  лидирует +22%» — измерение было на opt-0 (debug); на release (opt-1 и
  opt-3, engine_perf --test) sefer-alloc 17-22× БЫСТРЕЕ mimalloc.
  Gate: 235/235 lib PASS.

- TaskList прибран: удалено 9 completed (#287, #356–360, #362–364) и 2
  сделанных с прошлой сессии pending (#355 captrack PGO — user выполнил
  сам в отдельных коммитах; #361 ACL drift — user починил серией из
  4 fix(bench) коммитов после этой сессии, но всё равно закрыта).

**Ревью user-коммитов после сессии** — user за прошедшее время закоммитил
8 своих правок (не мои):
- `f9008096` + `5d4f3698` — captrack-pgo pre-size Vec::with_capacity на
  hot-path (13 + 10 сайтов, `--cap-from max`, полный workspace test-suite
  под инструментацией, 3975/3975 green);
- `b1473811` — declare missing `[workspace.dependencies] captrack` (root
  manifest fix);
- `f2b452be` + `05bbeeea` + `46dcb9e3` + `ac5ad603` — серия ACL fix'ов в
  5 бенчах (#361): `create_db_as`/`add_repo_as` вместо надежды на open
  0o777. Причина — Strategy A owned_enforced по умолчанию, System-owned
  ресурсы теперь 0o700, регулярный `alice` не мог `Execute`;
- `667fc5ad` — `batch_planner chain/20`: BatchLimits.max_dependency_depth
  по умолчанию 10, бенч до 20, поднят до 25 в variant'е.

**Inspected в этой сессии:** `docs/roadmap/PLAN.md`, `crates/shamir-tx/
src/changefeed.rs`, `crates/shamir-db/src/shamir_db/shamir_db/changelog.rs`,
`crates/shamir-connect/src/common/{envelope,push_envelope}.rs`, `crates/
shamir-server/src/db_handler/{handler,mod}.rs` (428 строк, читал целиком),
`crates/shamir-db/src/shamir_db/system_store.rs`, `crates/shamir-engine/
src/repo/changelog_store.rs`, `crates/shamir-engine/src/repo/repo_instance.rs`
(строки вокруг `emit_changefeed_event`), `crates/shamir-engine/src/tx/
{commit,group_commit}.rs` (для скоупа PR3), `crates/shamir-query-types/
src/batch/batch_op.rs`. Fetched: `crates.io/api/v1/crates/sefer-alloc`.

**Active timers:** нет.

**Push-статус:** мастер полностью синхронизирован с origin, оба
`docs/*` коммита этой сессии (927537bf, 5beea8f8) плюс sefer-alloc
registry-switch (f22edd62) и все 8 user-коммитов уже наверху HEAD =
`5d4f3698`.

## Active goal

none

## TaskList

### in_progress
(пусто)

### pending
- #365 Replication R0: network changefeed pull-API (ReplHello + ReplPull + роль replicator)
- #366 Replication pre-refactor PR0–PR4 (смоук system-changefeed, is_write, has_role, read-only точка, финализационное ядро)

### recently completed
(все completed в этой сессии удалены)

### deleted this session
11 (9 completed из прошлых сессий + 2 pending, ушедших в user-коммиты:
#355 captrack PGO run, #361 bench ACL drift fix).

## Decisions

- **Логическая репликация по существующему changelog** (row-based, полные
  байты записи) — не physical WAL-shipping (привязка к backend), не
  op-replay (недетерминизм WASM). Кreps/Alsberg/Schneider.
- **Lazy master, pull-first** (Kafka-модель): follower ведёт bookmark,
  лидер stateless по консьюмерам, `ReplStream` — только оптимизация,
  деградирующая обратно в pull. Reject: multi-master в v1 (Gray et al.,
  SIGMOD'96 — конфликты ~N³).
- **`leader_epoch` в каждом сообщении с R0** (Viewstamped Replication) —
  fencing от split-brain дешевле заложить сразу, чем ретрофитить.
- **Node accounts = существующий Shomer**: серверные аккаунты — обычные
  SCRAM-users с ролью `replicator` + `authorize_access` на каждый
  ReplPull; deny-by-default. Reject: параллельная система прав.
- **Publication/subscription-модель с ReplicationProfile-шаблонами** (§5.5):
  streams `(scope, direction, mode)` + именованные профили на node
  accounts. Реплика аккаунтов и настроек — обычный data-поток
  (`system/*` в scope), не отдельный механизм. Push-потоки означают
  лидерство per `(db, repo)`, не per узел.
- **sefer-alloc с crates.io 0.2.1**, не локальный path — 0.2.1 наконец
  опубликован. Reject: держать локальный path-dep — тормозит любую
  публикацию проекта.

## Open questions

- **Untracked `crates/*/target/`** — 16 директорий в working tree
  (кто-то запускал cargo изнутри крейтов; корневой `.gitignore`
  `/target` их не покрывает). Предложено добавить `crates/*/target/` в
  `.gitignore` — user не ответил.
- **PR3 (finalize_commit refactor)** — hot-path, /opti-цикл с
  `tx_pipeline` бенч-гейтом. Кто исполняет: sh-агент или прямо в сессии?
  Разово не решено.
- **PR0 смоук (V1a последние 5%)** — подтвердить смоук-тестом что
  `create_user` реально даёт событие в `read_changelog_from("system", …)`.
  Если RED — добавляется PR0b (подключить system repo к changefeed).

## Repo state

```
?? crates/shamir-client/target/
?? crates/shamir-db/target/
?? crates/shamir-engine/target/
?? crates/shamir-funclib/target/
?? crates/shamir-index/target/
?? crates/shamir-query-builder/target/
?? crates/shamir-query-types/target/
?? crates/shamir-sdk/target/
?? crates/shamir-server/target/
?? crates/shamir-storage/target/
?? crates/shamir-transport-tcp/target/
?? crates/shamir-transport-ws/target/
?? crates/shamir-tx/target/
?? crates/shamir-types/target/
?? crates/shamir-wal/target/
?? crates/shamir-wasm-host/target/
```

```
5d4f3698 perf: max-capacity re-apply from full captrack-pgo profile (tests+benches)
667fc5ad fix(bench): batch_planner chain/20 exceeded default max_dependency_depth
ac5ad603 fix(bench): access_denied panics in subscription_fanout/wire_pipelining
46dcb9e3 fix(bench): access_denied panic in subscription_delivery
05bbeeea fix(bench): access_denied panics in wire_latencies/subscription_throughput
```

Мастер синхронизирован с origin. Мои коммиты этой сессии (927537bf
`docs(roadmap): REPLICATION.md`, 5beea8f8 `docs(research):
пред-рефакторинг`, f22edd62 `deps: sefer-alloc local path → crates.io
0.2.1`) сидят между `26a810d0` и user-серией fix'ов сверху.
