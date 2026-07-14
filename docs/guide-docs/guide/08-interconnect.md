בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 8 — Interconnect: P2P / Chat (the "I")

**Когда подниматься:** нужна децентрализация.

Буква **"I"** в S.H.A.M.I.R. — Interconnected. Это самый высокий этаж
спирали и единственный, который **ещё не реализован**. Здесь честно
описано, что существует, что планируется и почему.

> ⚠ **Этот этаж — роадмап, не реализация.** Ни один из описанных
> ниже P2P/gossip/replication механизмов не имеет рабочего кода. Всё,
> что реально работает, — это фундамент (changefeed), на котором они
> будут построены. Не пытайся использовать примеры ниже как API — их
> нет.

## 1. Что существует сегодня

### Changefeed — foundation for replication

Единственный реальный строительный блок для "I" — changefeed
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

Это **локальный** changefeed — он работает внутри одного процесса.
Network-транспорт поверх него — **не реализован**.

### Транспорты

`shamir-transport-tcp` и `shamir-transport-ws` — клиент↔серверные
транспорты (TLS 1.3, framing). Они не содержат peer-discovery, gossip
или mesh-логики.

### Что не существует

|| Компонент | Статус |
||---|---|
|| P2P-протокол / gossip | ❌ Нет кода |
|| Peer discovery | ❌ Нет кода |
|| Leader-follower репликация | ❌ Нет кода |
|| Network changefeed (pull API over wire) | ❌ Нет кода |
|| Live subscriptions (server-push) | ❌ Design doc (`docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md`), status: PROPOSED |
|| Chat-протокол | ❌ Нет кода |
|| `shamir-interconnect` crate | ❌ Не существует |

## 2. Планируемая архитектура

Из роадмапа (`docs/dev-artifacts/roadmap/PLAN.md`, Movement C — "parked by request"):

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
          │               │               │
          └───────────────┼───────────────┘
                          ▼
                    P2P / gossip → chat
```

### Шаг 1: Network changefeed

Wire-запрос поверх существующего durable journal. Любой авторизованный
клиент может подписаться на поток изменений:

```
REQUEST  → { "op": "changes_since", "repo": "main", "cursor": 42, "limit": 100 }
RESPONSE ← { "events": [...], "next_cursor": 47, "gap_at": null }
```

Cursor — это `commit_version`. Resumable: клиент запоминает cursor,
при переподключении продолжает с последнего.

### Шаг 2: Live subscriptions (server-push)

Транспортный механизм для server→client push. Design doc:
`docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md` (status: PROPOSED).

* Новый `DbRequest` verb: `subscribe` / `unsubscribe`.
* Сервер отправляет unsolicited frames с change events.
* Client-side demux разделяет push-события от обычных ответов.

### Шаг 3: Leader-follower replication

Follower подключается к leader'у и подписывается на changefeed:

```
Follower  ──subscribe changefeed──►  Leader
Follower  ◄──push ChangelogEvents──  Leader
Follower  ──apply changes──────────►  Local store
```

Результат: read-replica. Пишет только leader; follower тянет изменения
и применяет локально.

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

* **Changefeed — реален и P2P-ready.** Монотонный `commit_version`,
  resumable journal, gap detection — всё спроектировано так, чтобы
  network-транспорт мог подключиться сверху.
* **Всё остальное — роадмап.** Не жди P2P в ближайших релизах. Если
  нужна репликация сегодня — external orchestrator (backup/restore,
  rsync data_dir) или application-level sync через changefeed pull.
* **Смотри роадмап.** `docs/dev-artifacts/roadmap/STAGES.md` и `docs/dev-artifacts/roadmap/PLAN.md`
  — актуальное состояние планов.
