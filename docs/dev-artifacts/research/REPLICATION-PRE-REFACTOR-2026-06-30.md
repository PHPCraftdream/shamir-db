בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Пред-рефакторинг под репликацию (R0/R1) — разведка + план

> **Дата:** 2026-06-30.
> **Контекст:** [`../roadmap/REPLICATION.md`](../roadmap/REPLICATION.md)
> (дизайн Movement C step 3). Принцип — Fowler, *Preparatory Refactoring* /
> Beck: «make the change easy, then make the easy change».
> **Содержимое:** (А) результаты verify-пунктов §7.1 — проверено по коду,
> не по памяти; (Б) пять пре-рефакторинг шагов PR1–PR5, упорядоченных так,
> чтобы R0/R1 ложились рядом с существующим кодом, а не врезались в него.

---

## А. Verify-результаты (§7.1 REPLICATION.md, таска #364)

### V1 (G4) — журнал append-only, retention НЕТ ✓

`crates/shamir-engine/src/repo/changelog_store.rs` — `StoreChangelog`
реализует ровно два метода: `put` + `range_from`. Ни truncate, ни prune,
ни TTL. Журнал в namespace `"__changelog__"` per-repo растёт вечно.

**Следствия:** `journal_floor ≡ 0` — R0 может честно обещать
bootstrap-с-нуля любому follower'у; retention-политика журнала обязана
появиться в R2 (вместе со snapshot transfer, иначе после truncate новые
реплики не поднимутся). Диск: full-row события ⇒ журнал ≈ суммарный
объём всех записей × их версий — для write-heavy install это реальный
расход, отметить в R2.

### V1a (G2-system) — SystemStore пишет через tx-path ✓ (≈95%)

`system_store.rs` использует `execute_set_tx` / `execute_delete_tx` /
`run_implicit_batch_tx` (см. комментарий у строк 112–113 про V1-marker и
file-WAL path) — то есть системные записи проходят обычный tx-commit.
Changefeed emit живёт во всех tx-commit путях ⇒ **у repo `system`
changefeed уже должен работать из коробки** ⇒ репликация аккаунтов /
настроек / ролей (§5.5 REPLICATION.md) — обычный data-поток уже в R1,
никакого отдельного механизма.

Остаточные 5%: подтвердить смоук-тестом, что `subscribe_changelog` /
`read_changelog_from` на system-repo реально отдаёт события после
`create_user` (PR0 ниже). Возможная ловушка: system repo мог быть
сконструирован без changefeed-handle.

### V2 (G1) — interner готов к дельта-синку ✓

`interner_manager.rs`: `save_new_keys(&[(InternerKey, UserKey)])` +
`persist()` + `persisted_high_water()` — менеджер УЖЕ ведёт high-water
и персистит только новые ключи. `ReplInternerSync { from_id }` из
дизайна ложится прямо на эту механику: дельта = ключи с id >
`from_id`. Пре-рефакторинг не нужен — нужен только read-API «дай ключи
с id ≥ N» (маленький additive метод при реализации R1).

### V3 (G2-скоуп) — что НЕ проходит tx-path

Прямо в tx-path НЕ идут: ktav-конфиг (файл, локальный — и не должен),
`tables_registry` (boot-replay aid в `persist_table_lifecycle` —
дублирующая запись вне system-store tx; при репликации system-repo
follower получит `system/tables` через поток, а его локальный
tables_registry обновится при apply — проверить в R1), interner
meta-store writes (`save_new_keys` — идут в meta-таблицу; проходят ли
они через tx-commit — уточнить в PR0-смоуке; если нет, интернер-синк
остаётся отдельным op'ом, что дизайн §5.1 и так предусматривает).

### V4 — идемпотентность apply: дёшево ✓ (по конструкции)

MVCC single-log + RecordCell high-water mark (bump-first, Opt O) —
текущая head-версия записи читается без скана. `apply(event)` со
сравнением `event.commit_version <= applied_version(repo)` — O(1)
по watermark'у tx-gate (`current_commit_version` уже экспонирован).
Пер-запись сравнение не требуется: контракт §4.1 (строго
последовательное применение) делает repo-уровневый watermark
достаточным.

### V5 — инфраструктура готова ✓

- **Dispatch расширяем:** `DbRequest` enum в `shamir-query-types::wire`
  (общий для клиента и сервера), match в `handler.rs` (428 строк, чистый),
  sub-handlers по файлам (`tx_handlers.rs`, `subscribe_handler.rs`) —
  `repl_handler.rs` встаёт по готовому шаблону.
- **Config структурирован:** `Config { listeners, tls, security, audit,
  observability, ... }` — секция `replication: ReplicationConfig` добавляется
  тривиально (serde + ktav).
- **Метрики:** `observability.rs` + prometheus exporter — паттерн есть.
- **E2E-прецедент:** `tests/mvp_e2e.rs` гоняет полный TLS+SCRAM стек —
  шаблон для leader+follower теста.

---

## Б. Пре-рефакторинг план (PR0–PR5)

Порядок = порядок исполнения. Каждый шаг — отдельный коммит, зелёный gate,
полезен сам по себе (даже если репликация задержится).

### PR0 — смоук-тест «changefeed на system repo» (test-only)

RED→GREEN тест в `shamir-db`: `create_user` (или `create_role`) →
`read_changelog_from("system"-репо)` содержит событие с
`table == "users"`. Если GREEN сразу — V1a подтверждён на 100%, и тест
остаётся регрессионной страховкой репликации аккаунтов. Если RED —
узнаём ДО R1, что system repo нужно подключить к changefeed (и это
станет PR0b, маленьким и точечным).
*Размер: S. Риск: нулевой (test-only).*

### PR1 — `BatchOp::is_write()` (симметрия к `is_admin`)

В `batch_op.rs` есть только `is_admin()`. Read-only gate follower'а
(§4.3 дизайна) требует классификации «мутирует ли op данные». Добавить
`is_write()` рядом с `is_admin()` — исчерпывающий match по вариантам
(Insert/Update/Set/Delete/Call?/sub-Batch — рекурсивно), с тестом,
который ловит новые варианты enum'а (компилятор заставит: match без
wildcard).
*Размер: S. Польза вне репликации: аудит, метрики write-долей.*

### PR2 — ролевой хелпер в Session

`handler.rs` проверяет только `is_superuser` (permission gate v1);
`Session.permissions.roles: Vec<String>` существует, но нигде не
читается. Добавить `SessionPermissions::has_role(&self, r: &str) -> bool`
+ использовать в новом гейте `require_role(session, "replicator")`.
Ничего не менять в существующих проверках — только подготовить
инструмент.
*Размер: S.*

### PR3 — выделить финализационное ядро коммита (главный шаг)

Сейчас **четыре** commit-пути дублируют финализацию
(persist → publish_committed → emit_changefeed_event):
`commit_tx_inner_legacy_async`, `commit_tx_lockfree` (commit.rs),
`run_single_tx` + групповой путь (group_commit.rs). `apply_replicated`
(ядро R1) станет пятой копией той же последовательности — если не
выделить общую функцию.

Выделить `finalize_commit(repo, version, staged_writes, event) -> …`
(имя по месту): нижняя половина коммита, общая для всех путей —
запись версии в MVCC-log, обновление индексов, publish в tx-gate,
emit события. Верхние половины (SSI-валидация, WAL, group-batching,
локи) остаются в своих путях. `apply_replicated` в R1 = «пропустить
верхнюю половину, вызвать нижнюю с версией из события».

⚠️ Это hot-path (tx_pipeline бенч) — шаг делается в /opti-дисциплине:
baseline `tx_pipeline` ДО, рефакторинг, тесты `@oracle`, post-бенч —
регрессия недопустима (цель — ноль-разница: чистое выделение функции,
инлайнится обратно).
*Размер: M. Риск: средний (hot-path) — гейтится бенчем.*

### PR4 — точка read-only гейта в `execute()`

`ShamirDbHandler::execute()` — уже единственная точка входа всех
батчей. Добавить понятие «node mode» (enum `NodeMode { ReadWrite,
ReadOnly }`, default ReadWrite) в handler-конфиг рядом с
`QueryLimitsCap`. Гейт: `if mode == ReadOnly && batch содержит
is_write() → DbResponse::Error { code: "read_only_replica", … }`.
В R1 сюда добавится `leader_addr` в тело ошибки. До R1 mode всегда
ReadWrite — нулевое поведенческое изменение, но точка врезки готова
и покрыта тестом.
*Размер: S (после PR1).*

### PR5 — резервирование wire-пространства (design-only, вместе с R0)

Не пре-рефакторинг в строгом смысле, а решение при первом касании R0:
Repl-ops оформить как ОДИН вариант `DbRequest::Repl(ReplRequest)` с
вложенным enum'ом, а не пять новых верхнеуровневых вариантов — держит
privileged-протокол в одном месте, не раздувает общий клиентский enum,
и позволяет эволюционировать репликационный протокол независимо
(своя версия внутри `ReplHello.proto_ver`).
*Размер: 0 (решение, не работа).*

### Явно НЕ делаем заранее

- **Новый крейт `shamir-repl`** — R0 помещается в `repl_handler.rs` +
  wire-варианты; крейт заводить только если R1 покажет реальный объём
  (клиентский follower-loop может жить в `shamir-server`).
- **Retention журнала** — R2-работа (связана со snapshot transfer),
  сейчас не мешает.
- **Generic-переделка permission gate v1 → полный RBAC** — repl-ops
  хватает PR2-хелпера; большой RBAC-рефакторинг остаётся отдельным
  треком (упомянут в handler.rs docstring как follow-up).
- **Node registry / Ed25519-подписи** — R3 (§5.4 дизайна).

---

## Порядок и стыковка с R0/R1

```
PR0 (смоук) → PR1 (is_write) → PR2 (has_role) → PR4 (read-only точка)
                                   ↘ R0 (ReplHello/ReplPull + PR5-решение)
PR3 (финализационное ядро, /opti-дисциплина) ─────→ R1 (apply_replicated)
```

PR0–PR2/PR4 — мелкие, независимые, можно одной серией. PR3 — отдельный
/opti-цикл с бенч-гейтом; он единственный блокирует R1 (но НЕ блокирует
R0 — pull-API не трогает commit-пути).

_Разведка 2026-06-30: dispatch/config/метрики/e2e готовы as-is; журнал
append-only без retention (journal_floor ≡ 0); SystemStore на tx-path
(репликация аккаунтов = data-поток, подтвердить PR0-смоуком); interner
уже ведёт high-water под дельта-синк; идемпотентность apply — O(1) по
repo-watermark'у._
