# Pre-Transactional Preparation

Эта папка — план подготовки инфраструктуры **до** реализации
полноценных изолированных транзакций. Сами транзакции спроектированы
в [`../roadmap/TRANSACTIONS.md`](../roadmap/TRANSACTIONS.md) +
[`../roadmap/TRANSACTIONS_IMPL.md`](../roadmap/TRANSACTIONS_IMPL.md);
здесь — что нужно сделать **в существующей кодовой базе**, чтобы тот
план можно было реализовать без хаоса и компромиссов.

## Зачем отдельный фронт работ

Прямая реализация MVCC поверх текущего кода упрётся в семь конкретных
архитектурных препятствий:

1. **HNSW необратим** — `hnsw_rs::Hnsw::insert` без `remove`. Abort
   транзакции оставит точки в графе → утечки + деградация recall.
2. **Index writes напрямую** — все `on_insert/update/delete` в
   index2 пишут в storage немедленно. Откатить нечего.
3. **WAL без inline body** — recovery читает запись из data_store.
   Для MVCC tx-uncommitted writes ещё нет в data_store → atomicity
   ломается на crash mid-commit.
4. **Keyspace разрозненный** — `b"__index__"` / `b"__meta__"` /
   `b"__counter__"` литералами по коду. Добавить `::<version>` суффикс
   в одном месте невозможно.
5. **Read pipeline не знает про tx** — `TableManager::get/iter_stream`
   проброс контекста транзакции отсутствует. Большая поверхность для
   рефакторинга.
6. **Background tasks tx-naive** — MemBuffer flusher, auto-verify,
   future GC. Будут "чинить" недопубликованные транзакции если их не
   развести.
7. **Test infrastructure** — один сервер на orchestrator, нет
   multi-connection harness. Изоляцию проверить нечем.

Этот документ снимает каждое препятствие отдельной задачей, **не
ломая существующего поведения** на non-tx путях (zero overhead).

## Структура

- **[00-overview.md](./00-overview.md)** — карта нынешнего состояния
  кодовой базы, обзор всех семи этапов, итоговый estimate.
- **[01-foundations.md](./01-foundations.md)** — keyspace consolidation,
  `Store::transact`, CAS, WAL inline body.
- **[02-write-isolation.md](./02-write-isolation.md)** —
  `IndexWriteOp` planner, HNSW staging, `StagingStore`.
- **[03-repo-coordinator.md](./03-repo-coordinator.md)** —
  `RepoTxGate`, `TxContext`, `LayeredInterner`, repo-level WAL.
- **[04-mvcc-store.md](./04-mvcc-store.md)** — `MvccStore`
  (current+history layout), read pipeline через `Option<&TxContext>`.
- **[05-executor-isolation.md](./05-executor-isolation.md)** —
  executor integration, SI / SSI, cross-repo guard.
- **[06-reconciliation.md](./06-reconciliation.md)** — MemBuffer,
  migration coordinator, audit log, auto-verify watchdog.
- **[07-gc-telemetry.md](./07-gc-telemetry.md)** — GC worker,
  метрики, max-tx-lifetime cap.
- **[08-tests-landing.md](./08-tests-landing.md)** — multi-connection
  e2e harness, 10 concurrent scenarios, wire format extension, docs.
- **[architectural-decisions.md](./architectural-decisions.md)** —
  decision log: пять архитектурных решений, которые надо
  зафиксировать **до** старта.
- **[crate-organization.md](./crate-organization.md)** — что из
  transactional preparation выносится в отдельные крейты
  (`shamir-wal`, `shamir-tx`) и почему остальное остаётся в engine.

## Принципы каждого этапа

- **Самодостаточный.** Этап можно land'ить отдельно, мерж не зависит
  от следующего.
- **Zero overhead для non-tx.** Существующий код не замедляется.
- **`Option<&TxContext>` параметр**, не generic dispatch. Compile-time
  бинарное раздувание не оправдано.
- **Перед каждым этапом — pre-commit gate** (fmt + clippy -D warnings
  + tests). Это уже наш standard (см. `../../AGENTS.md`).
- **Каждое архитектурное решение покрыто тестами И бенчмарками.** Это
  не «опционально если затронуло hot path» — это **обязательно** для
  каждого из пяти решений в
  [`architectural-decisions.md`](./architectural-decisions.md).
  Тесты доказывают **корректность** (поведение соответствует spec),
  бенчмарки доказывают **отсутствие регрессии** (non-tx путь не
  замедлен, tx-overhead в обещанных границах). Без обеих частей
  решение не считается «реализованным» — даже если код компилируется
  и существующие тесты зелёные.

## Estimate

~6-7 недель сфокусированной работы для всех семи этапов. После них
сам MVCC (по плану `TRANSACTIONS.md`) занимает ~2 недели чистого
кода — потому что вся подготовительная грязь уже разгребена.
