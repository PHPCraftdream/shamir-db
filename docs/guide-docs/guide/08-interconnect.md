בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 8 — Interconnect: P2P / Chat (the "I")

**Когда подниматься:** нужна децентрализация.

Буква **"I"** в S.H.A.M.I.R. — Interconnected. Это самый высокий этаж
спирали. Часть его (changefeed, network pull, leader-follower
репликация, live subscriptions) уже реализована и покрыта тестами;
верхний слой — децентрализованный P2P/gossip/chat — ещё нет. Здесь
честно описано, что существует, что планируется и почему.

> ⚠ **Этот этаж — частично реализация, частично роадмап.**
> Leader-follower репликация, network changefeed (pull API) и live
> subscriptions (server-push) реализованы и покрыты тестами — см.
> §1. Но P2P-протокол, gossip, peer discovery и chat **не имеют
> рабочего кода** — это по-прежнему чистый роадмап. Не пытайся
> использовать примеры Шага 4 ниже как API — их нет.

## 1. Что существует сегодня

### Changefeed — the substrate under replication

Базовый строительный блок для "I" — локальный changefeed
(`shamir-tx/src/changefeed.rs`):

* **Hybrid live-push** (`tokio::sync::broadcast`) + **durable journal**
  (append-log, keyed by `commit_version`).
* `ChangelogEvent` = `{ repo, commit_version, tx_id, actor, timestamp_ns,
  changes: Vec<RecordChange> }`.
* `read_from(version, limit)` — resumable pull API.
* События упорядочены монотонным `commit_version` — **по дизайну
  P2P-ready** (issue #179: changefeed version == MVCC version).
* Gap-маркеры (`gap_at`) для dropped events, watermark для persistence
  tracking.
* E2E-тесты: `shamir-db/tests/changefeed_e2e.rs`.

Начиналось как **локальный** механизм (внутри одного процесса). Сейчас
поверх него уже построен network-транспорт — pull API по wire-протоколу
(`ReplRequest::Pull`) и push-доставка подписчикам (live subscriptions),
см. ниже.

### Транспорты

`shamir-transport-tcp` и `shamir-transport-ws` — клиент↔серверные
транспорты (TLS 1.3, framing). Они не содержат peer-discovery, gossip
или mesh-логики.

### Network changefeed (pull API over wire) — реализовано

Это уже не роадмап, а рабочий wire-op: `ReplRequest::Pull`
(`crates/shamir-query-types/src/wire/repl.rs`). Авторизованный follower
запрашивает батч changelog-событий для одного репо начиная с
`from_version`, с опциональным long-poll (`wait_ms`). Ответ
(`ReplResponse::Pull`) несёт `leader_epoch` (VR-style fencing),
msgpack-encoded `events`, опциональный `gap_at` (если запрошенный
`from_version` уже за `journal_floor` — нужен reseed) и
`current_version` для расчёта лага. Диспетчер —
`crates/shamir-server/src/db_handler/repl_handler.rs`.

### Leader-follower репликация — реализовано

Follower-цикл, супервизор и источники (`InProcess`/`Wire`) живут в
`crates/shamir-server/src/replication/`: `follower_loop.rs`,
`supervisor.rs`, `source.rs`, `wire_source.rs`, `in_process.rs`,
`prod_factory.rs`. Follower подключается к leader'у через
`ReplRequest::Hello` (handshake — версия протокола, epoch, список
реплицируемых репо) и затем циклически тянет `Pull`, применяя события
локально. Тесты:
`replication::tests::{follower_loop_tests, supervisor_tests}` (lib),
плюс e2e-сьюты `crates/shamir-server/tests/repl_pull_e2e.rs`,
`repl_convergence_e2e.rs`, `repl_supervisor_boot.rs`,
`set_replicator_wire.rs`.

### Live subscriptions (server-push) — реализовано

`crates/shamir-server/src/subscriptions/` (`bridge.rs`, `registry.rs`,
`target_match.rs`, `filter_eval.rs`, `reactive.rs`, `push.rs`,
`decode_cache.rs`, `deliver_cache.rs`, `payload.rs`) — сервер держит
живое соединение и отправляет unsolicited push-фреймы с изменениями,
матчащими подписку клиента. На wire-уровне это `BatchOp::Subscribe`
(`SubscribeOp`) / `BatchOp::Unsubscribe` (`UnsubscribeOp`)
(`crates/shamir-query-types/src/batch/batch_op.rs`), плюс полноценная
publication/subscription DDL-модель (`CreateSubscription` /
`DropSubscription` / `AlterSubscription` / `ListSubscriptions`,
см. `docs/dev-artifacts/roadmap/REPLICATION.md §5.5`). Тесты:
`subscriptions::tests::{bridge_tests, registry_tests,
target_match_tests, filter_eval_tests, filter_lens_parity_tests,
cache_eviction_tests, cache_depth_probe_tests, reactive_limits_tests}`
— широкая, реальная сьюта.

### Что не существует

|| Компонент | Статус |
||---|---|
|| P2P-протокол / gossip | ❌ Нет кода |
|| Peer discovery | ❌ Нет кода |
|| Chat-протокол | ❌ Нет кода |
|| `shamir-interconnect` crate | ❌ Не существует |

## 2. Планируемая архитектура

Из роадмапа (`docs/dev-artifacts/roadmap/PLAN.md`, Movement C — "parked by request").
Шаги 1–3 ниже уже реализованы (см. §1); Шаг 4 остаётся чистым роадмапом:

```
                ┌─────────────────────┐
                │  Этап F: Interconnect  │
                └─────────┬───────────┘
                          │
          ┌───────────────┼───────────────┐
          ▼               ▼               ▼
   Network changefeed  Live subs     Leader-follower
   (pull API over      (server-push   replication
    existing journal)   to clients)   (follower subscribes)
     ✅ реализовано     ✅ реализовано   ✅ реализовано
          │               │               │
          └───────────────┼───────────────┘
                          ▼
                    P2P / gossip → chat
                    ❌ роадмап (Шаг 4)
```

### Шаг 1: Network changefeed — реализовано

Wire-запрос поверх существующего durable journal:
`ReplRequest::Pull` (`crates/shamir-query-types/src/wire/repl.rs`).
Реальные поля (не иллюстративный набросок) —

```
REQUEST  → ReplRequest::Pull { db, repo, from_version: 42, limit: 100, wait_ms: Some(5000) }
RESPONSE ← ReplResponse::Pull { leader_epoch, events: <msgpack Vec<ChangelogEvent>>, gap_at: None, current_version }
```

`from_version` — это `commit_version`. Resumable: follower запоминает
последний применённый `commit_version`, при переподключении
продолжает с него. `wait_ms` даёт long-poll вместо busy-pull. `gap_at`
сигнализирует, что запрошенный `from_version` уже выпал за
`journal_floor` — follower должен reseed.

### Шаг 2: Live subscriptions (server-push) — реализовано

Транспортный механизм для server→client push:
`crates/shamir-server/src/subscriptions/`. Design doc (история задачи):
`docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md`.

* Wire-операции: `BatchOp::Subscribe(SubscribeOp)` /
  `BatchOp::Unsubscribe(UnsubscribeOp)`
  (`crates/shamir-query-types/src/batch/batch_op.rs`).
* Сервер отправляет unsolicited push-фреймы с change-событиями,
  матчащими зарегистрированную подписку (`registry.rs`,
  `target_match.rs`, `filter_eval.rs`, `reactive.rs`, `push.rs`).
* Публикации/подписки управляются DDL-операциями
  (`CreateSubscription`/`DropSubscription`/`AlterSubscription`/
  `ListSubscriptions`).

### Шаг 3: Leader-follower replication — реализовано

Follower подключается к leader'у и тянет changefeed:

```
Follower  ──ReplRequest::Hello──►  Leader   (handshake: proto_ver, node_id)
Follower  ◄──ReplResponse::Hello──  Leader   (leader_epoch, реплицируемые репо)
Follower  ──ReplRequest::Pull────►  Leader   (from_version, limit, wait_ms)
Follower  ◄──ReplResponse::Pull──  Leader   (events, gap_at?, current_version)
Follower  ──apply changes───────►  Local store
```

Результат: read-replica. Пишет только leader; follower тянет изменения
через `crates/shamir-server/src/replication/follower_loop.rs` +
`supervisor.rs` и применяет локально. VR-style epoch fencing защищает
от применения событий от устаревшего leader'а.

### Шаг 4: P2P / gossip → chat

Децентрализованное состояние. Несколько ShamirDB-инстансов образуют mesh:

* **Gossip** — распространение метаданных (кто жив, какие данные у кого).
* **Anti-entropy** — фоновая синхронизация: Merkle-tree → diff → merge.
* **Chat** — прикладной слой поверх mesh: messaging, collaboration.

Это конечная цель "I" — полностью децентрализованная СУБД без
единой точки отказа.

## 3. Где это в роадмапе

Из `docs/dev-artifacts/roadmap/STAGES.md`:

```
A (эксплуатация) → C (доки) → B (бизнес-доступ) → D (поиск) →
E (перф) → F (P2P)
```

**Этап F — последний.** Перед ним: эксплуатация, документация,
бизнес-доступ, поиск, производительность. Это осознанный выбор:
P2P строится на стабильном фундаменте, а фундамент ещё дорабатывается.

## 4. Сквозной принцип и "I"

Из README:

> Истина живёт в **одном** месте — версионированном MVCC-сторе. Всё
> производное (кеши, индексы, проекции) — overlay над ней,
> восстановимый из WAL.

"I" не ломает этот принцип. Каждый peer владеет **своей** истиной
(свой MVCC store). Репликация — это **broadcast изменений** (через
changefeed), а не distributed lock. Конфликты — на уровне application
logic (CRDT, last-write-wins, merge-функции).

## Что важно знать уже сейчас (дозированно)

* **Changefeed, network pull, leader-follower репликация и live
  subscriptions — реальны, реализованы и покрыты тестами.** Монотонный
  `commit_version`, resumable journal (`from_version`/`wait_ms`), gap
  detection (`gap_at`), VR-style epoch fencing — всё это уже работающий
  код, а не набросок API. Если нужна репликация или server-push
  сегодня — это доступно через `ReplRequest`/`ReplResponse` и
  `BatchOp::Subscribe`/`Unsubscribe`.
* **P2P, gossip, peer discovery и chat — по-прежнему роадмап.** Не жди
  децентрализованного mesh в ближайших релизах.
* **Смотри роадмап.** `docs/dev-artifacts/roadmap/STAGES.md` и `docs/dev-artifacts/roadmap/PLAN.md`
  — актуальное состояние планов.
