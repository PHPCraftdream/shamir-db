בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Replication — the "I" step 3 (Movement C) · research + design

> **Дата:** 2026-06-30.
> **Статус:** research → design. Код не написан; этот документ — тот самый
> `REPLICATION.md`, который `PLAN.md` §2 резервировал «to be written when
> replication starts».
> **Цель:** leader→follower репликация поверх существующего changefeed —
> внутренний протокол, follower apply-engine, QOL-обвязка (конфиг, метрики,
> reconnect, promote) — как ступень к P2P/chat (буква **I** в S.H.A.M.I.R.).
> **Метод:** формальные источники → инвентаризация того, что уже есть в коде
> → дизайн протокола и apply-пути → фазы R0–R4 с verify-пунктами.

---

## 1. Академический фундамент

### 1.1 Модель: passive replication / log shipping

**Primary-Backup (passive) replication** — Alsberg & Day, *A Principle for
Resilient Sharing of Distributed Resources* (ICSE 1976); систематизация —
Budhiraja, Marzullo, Schneider, Toueg, «The Primary-Backup Approach»
(*Distributed Systems*, 2nd ed., Mullender, ch. 8, 1993). Один узел (leader)
принимает все записи; backups применяют поток изменений. Просто, предсказуемо,
минимум координации — правильная первая ступень.

**State Machine Replication** — Schneider, *Implementing Fault-Tolerant
Services Using the State Machine Approach* (ACM Computing Surveys 22(4),
1990). Реплики сходятся, если применяют детерминированные операции в одном
и том же порядке. Наш changelog — уже готовый SMR-лог с тотальным порядком
(`commit_version`, монотонный per-repo), причём **проще классического SMR**:
события несут *полные байты записи* (physical-logical, как row-based
binlog MySQL), а не операции → apply не требует детерминизма исполнения и
идемпотентен тривиально.

**Почему НЕ multi-master сразу** — Gray, Helland, O'Neil, Shasha, *The
Dangers of Replication and a Solution* (SIGMOD 1996): update-anywhere
репликация масштабирует конфликты нелинейно (deadlock rate ~ N³ в их
модели); lazy-master (наш выбор) — стабильная точка. Multi-writer возвращается
только на P2P-фазе через CRDT (см. §8).

### 1.2 Порядок, эпохи, failover

**Viewstamped Replication** — Oki & Liskov (PODC 1988); Liskov & Cowling,
*Viewstamped Replication Revisited* (MIT-CSAIL-TR-2012-021). Вводит
**view number** (у нас — `epoch`): монотонный номер конфигурации лидерства.
Любое сообщение репликации несёт epoch; узел отвергает события от лидера с
меньшим epoch. Это — минимальный механизм fencing'а от split-brain, и его
дёшево заложить в протокол с первого дня, даже пока failover ручной.

**Raft** — Ongaro & Ousterhout, *In Search of an Understandable Consensus
Algorithm* (USENIX ATC 2014). Не нужен для Фаз R0–R2 (async репликация не
требует консенсуса), но его term/лидер-элекция — референс для Фазы R3
(автоматический failover), если/когда он понадобится. До тех пор promote —
ручная админ-операция (как PostgreSQL `pg_promote`).

**PacificA** — Lin, Yang, Zhou (MSR-TR-2008-25): разделение
*configuration management* (кто лидер, membership) от *data replication*
(поток логов). Мы следуем этому разделению: протокол данных (§5) не знает
про элекции; epoch — единственная точка соприкосновения.

**Chain replication** — van Renesse & Schneider (OSDI 2004): реплики цепочкой,
каждая кормит следующую. У нас каскад **бесплатен по построению**: follower
ре-эмитит применённые события в свой локальный changefeed (§4.3), значит
follower-of-follower работает тем же протоколом без нового кода.

### 1.3 Практический prior art

| Система | Что берём | Источник |
|---|---|---|
| PostgreSQL streaming replication | LSN ↔ наш `(db, repo, commit_version)`; slot/bookmark на consumer'е; `pg_promote`; semi-sync `synchronous_commit` уровни | PG docs ch. 26 |
| MySQL row-based binlog + GTID | full-row events (= наш `RecordChange.value`); GTID-идемпотентность | MySQL 8.4 ref ch. 19 |
| Kafka ISR / follower fetch | **pull**-модель: follower ведёт свой прогресс, лидер stateless по консьюмерам; high-watermark | Kreps et al. (NetDB 2011), KIP-101 |
| Kleppmann, *DDIA* ch. 5 | таксономия sync/async/semi-sync; аномалии replication lag (read-your-writes, monotonic reads) и их клиентские лекарства | O'Reilly 2017 |
| CAP / PACELC | async-репликация = EL: жертвуем консистентностью хвоста ради latency; RPO > 0 при failover — документированный выбор | Gilbert & Lynch (SIGACT 2002), Abadi (IEEE Computer 2012) |
| CRDT (P2P-фаза) | конфликтно-свободные типы для chat / update-anywhere | Shapiro, Preguiça, Baquero, Zawirski (INRIA RR-7506; SSS 2011) |

### 1.4 Сводка принципов

1. **Lazy master, pull-first.** Follower тянет журнал по своему bookmark'у;
   лидер не хранит per-follower state (Kafka-модель). Push (live-stream) —
   оптимизация поверх pull, не замена: catchup всегда через pull.
2. **Тотальный порядок уже есть** — `commit_version`. Не изобретать вторую
   нумерацию; `(db, repo, commit_version)` — это наш GTID/LSN.
3. **Идемпотентность по версии.** `apply(event)` при
   `event.commit_version <= applied_version` — no-op. Повторная доставка
   безопасна ⇒ reconnect-логика тривиальна.
4. **Epoch с первого дня** — fencing от split-brain дешевле закладывать в
   wire-формат сразу, чем ретрофитить.
5. **Данные ≠ конфигурация.** Протокол данных не занимается элекциями;
   membership/promote — админ-плоскость.

---

## 2. Что уже есть в коде (инвентаризация)

Фундамент Movement C «fully laid» (PLAN.md §2) — конкретно:

### 2.1 Готовый репликационный лог

`shamir-tx/src/changefeed.rs` — hybrid live-push + durable journal:

- **`ChangelogEvent`** `{ repo, commit_version, tx_id, actor, timestamp_ns,
  changes: Vec<RecordChange> }`; `RecordChange` `{ table, key: Bytes
  (16-байтный RecordId), op: Put|Delete, value: Option<Bytes> }`. `value` —
  **полные новые байты записи** (те же, что WAL сериализует) ⇒ apply =
  прямой put/delete, идемпотентный, без op-replay.
- **Эмит после `gate.publish_committed`** из всех коммит-путей
  (`tx/commit.rs` ×2, `tx/group_commit.rs` ×2), включая non-tx writes
  (#177). `commit_version == MVCC version` (#179) — «a replica can apply
  by version» заложено сознательно.
- **Durable journal** per-repo, ключ — BE-u64 `commit_version`
  (лексикографический порядок = числовой); фоновый writer вне
  commit-critical-path; `last_persisted_version` watermark;
  **`gap_at`-сигнал** при drop'е из-за overflow (`JournalRead { events,
  gap_at }`).
- **Live broadcast** — `tokio::sync::broadcast` per-repo; отставший
  подписчик получает `Lagged` и достирает окно из журнала.

### 2.2 Готовые API поверх (facade)

`shamir-db/src/shamir_db/shamir_db/changelog.rs`:
`subscribe_changelog(db, repo)`, `read_changelog_from(db, repo,
from_version, limit)` (+ `_journal`-вариант с `gap_at`),
`current_commit_version(db, repo)`. Бенч `changelog_read` прямо в
docstring'е называет себя «pre-work for the leader→follower replication
pull loop» — baseline снят заранее.

### 2.3 Готовый транспорт и security

- TLS 1.3 + SCRAM-Argon2id + Ed25519 identity, resume-тикеты (reconnect
  ~7× дешевле полного handshake — измерено в
  `wire_latencies::handshake_paths`).
- Duplex + rid-demux; `PushEnvelope { push: Event|Gap|SlowConsumer|Ready|
  Closed, sub, seq, data, gap_at }` — сервер-инициированный push уже в
  протоколе (live subscriptions пользуются им сегодня).
- RBAC (Shomer) — место для роли `replicator`.
- Observability: axum `/healthz /readyz /metrics` + Prometheus exporter уже
  в `shamir-server`.

### 2.4 Дыры (то, чего нет — предмет этого дизайна)

| # | Дыра | Где всплывает |
|---|---|---|
| G1 | **Interner не в changefeed.** `RecordChange.value` — msgpack с interned u64-ключами; mapping u64↔имя персистится в meta-store (`interner_manager.rs::persist`) *мимо* журнала. Follower без mapping'а не может де-интернировать (читать) свои данные. | apply/read на follower |
| G2 | **DDL / meta-writes не в changefeed** (emit только из tx-commit путей). Create table/index, validators, access-изменения не едут потоком. | schema drift между узлами |
| G3 | **Нет raw-apply API** — весь write-path идёт через validators/CAS/SSI/новую версию. Follower'у нужен путь «применить событие с заданной версией». | ядро R1 |
| G4 | **Journal bounded?** Если журнал не truncate'ится — растёт диск; если truncate'ится — новый follower не сможет bootstrap'нуться с version 0 и потребует snapshot. Retention-политика журнала не определена. | bootstrap, R2 |
| G5 | **Нет read-only режима узла** и admin promote/demote. | R1/R3 |

---

## 3. Архитектура — выбор модели

### 3.1 Решение: логическая репликация по changelog (не физическая)

**Выбрано:** шиппинг `ChangelogEvent`'ов (row-based logical replication).

**Отвергнуто: физический шиппинг** (WAL-сегменты / fjall SST):
- привязывает реплику к байтовому формату backend'а (fjall-специфика,
  версии формата);
- исключает выборочную репликацию (per db/repo) и каскады;
- у нас WAL — короткоживущий (crash-recovery), не архивный поток, его
  пришлось бы переделывать в архивируемый (это второй большой проект);
- changelog уже спроектирован «replication-ready» (#179) — не использовать
  его значит выбросить готовую половину работы.

**Отвергнуто: op-shipping** (реплей BatchRequest'ов): требует детерминизма
исполнения (rand, время, WASM-функции!), ломается на любой недетерминированной
функции. Full-row events этого класса проблем не имеют (Schneider §1.1 —
мы shipping'уем *результат*, не *вычисление*).

### 3.2 Топология и роли

```
Фаза R1:            Фаза R2+ (каскад — бесплатно):      Фаза R4 (P2P):
leader ──► follower  leader ──► follower ──► follower    mesh + CRDT
   │                        └──► follower
   └──► follower
```

- **Роль узла** — конфиг: `leader` (default, принимает writes) или
  `follower { leader_addr }` (read-only, применяет поток).

  Роль (rw/ro) и класс доверия (§5.4 — что узел получает) — **две
  ортогональные оси**; их произведение и есть полная таксономия узлов:

  | | Класс `cluster` (полная копия) | Класс `replica` (scoped) |
  |---|---|---|
  | Суть | ro-узел, **промотируемый** в rw | ro-узел, **непромотируемый** |
  | Получает | всё, включая системный store | явные repos, без system store |
  | Назначение | DR / HA / failover-кандидат | read-scale, edge, аналитика |
  | При failover | `promote` → rw (epoch bump) | остаётся ro навсегда |

  rw-узел — ровно один на дерево (lazy master); ro-узлов — сколько угодно.
  ro отклоняет клиентские data-writes (`ReadOnlyReplica { leader_addr }`);
  локальные admin-ops (promote, status) ему разрешены. Смена ролей —
  только `promote`/`demote` с epoch-bump (§5.2).
- **Единица репликации** — `(db, repo)`: у каждого repo свой монотонный
  `commit_version` и свой журнал; никакого глобального порядка между repos
  не существует и не нужно (его нет и на одном узле).
- **Каскад**: follower ре-эмитит применённые события в свой локальный
  changefeed (§4.3) ⇒ его собственные подписчики (live subscriptions!) и
  его собственные followers работают без изменений протокола — chain
  replication по построению.

### 3.3 Консистентность (Фаза R1) — честная декларация

- **Async** (PACELC: EL). Лидер не ждёт follower'ов; RPO > 0 при потере
  лидера — хвост, не дошедший до реплики, теряется. Semi-sync — R2.
- **Монотонные чтения** на конкретном follower'е — да (лог применяется
  строго по возрастанию версии).
- **Read-your-writes** между узлами — нет; клиентское лекарство — barrier
  `min_version` (§6.4, QOL): клиент, получивший `commit_version=V` от
  лидера, читает с follower'а с барьером «подожди applied ≥ V».

---

## 4. Follower apply-engine (ядро R1, закрывает G3)

### 4.1 Раскладка

Новый модуль `shamir-engine/src/repo/replica_apply.rs` (или сосед):

```rust
/// Идемпотентно применить один реплицированный коммит.
/// Контракт:
///  - события применяются строго по возрастанию commit_version;
///  - event.commit_version <= applied_version(repo)  → Ok(Skipped);
///  - event.commit_version >  applied_version + 1    → Err(GapDetected)
///    (вызывающий обязан перечитать журнал лидера с applied+1);
///  - иначе: put/delete каждого RecordChange НАПРЯМУЮ в MVCC-лог с
///    версией = event.commit_version (не выделяя новую), обновить
///    индексы, продвинуть tx-gate watermark, ре-эмитить событие в
///    локальный changefeed, вернуть Ok(Applied).
pub async fn apply_replicated(repo: &RepoInstance, event: &ChangelogEvent)
    -> DbResult<ApplyOutcome>;
```

Ключевые свойства:

- **Мимо write-path'а лидера**: без validators / CAS / SSI-ledger / выдачи
  новой версии — всё это уже случилось на лидере; follower — исполнитель
  решённого. (Это же делает apply дешевле обычного write.)
- **Версия задана источником** — записать в MVCC single-log версию
  `event.commit_version` как committed. Temporal-история на реплике
  получается идентичной лидеру (тот же version-log) — AsOf/History работают.
- **Индексы** обновляются локально из full-row bytes (у нас есть и старое
  значение — читается по key перед put — и новое).
- **Ре-эмит в локальный changefeed** с тем же `commit_version` — каскад +
  локальные live-подписчики.
- **Idempotence-гарантия** позволяет at-least-once доставку по сети —
  никакого exactly-once в транспорте не требуется.

### 4.2 Bookmark (durable прогресс follower'а)

Follower персистит `applied_version` per `(db, repo)` в своём системном
store **атомарно с применением батча** (или консервативно: bookmark
обновляется ПОСЛЕ durable-применения; при рестарте повторное применение
хвоста — no-op по идемпотентности). Это slot-механика PostgreSQL, живущая
на стороне консьюмера (Kafka-стиль).

### 4.3 Read-only enforcement (закрывает G5)

Узел в роли `follower` отклоняет клиентские write-ops (`ReadOnlyReplica`
error code) — проверка в dispatch'е до executor'а. Admin-ops управления
самой репликой (promote, status) разрешены. Форвардинг writes лидеру —
сознательно НЕ в R1 (клиент получает адрес лидера в ошибке и идёт сам;
прозрачный прокси — QOL-кандидат R2+).

---

## 5. Внутренний протокол (поверх shamir-connect)

Никакого нового транспорта: те же TLS + SCRAM + envelope + rid-demux.
Репликация = выделенный пользователь с ролью **`replicator`** + новые
request-типы (как `SubscribeOp` для live subscriptions). Все ответы несут
`leader_epoch`.

### 5.1 Ops

```text
ReplHello    { proto_ver, node_id }
  → { leader_epoch, repos: [{db, repo, current_version, journal_floor}] }
     // journal_floor — минимальная версия, доступная в журнале (G4);
     // follower сверяет свой bookmark: bookmark+1 < floor → нужен reseed.

ReplPull     { db, repo, from_version, limit, wait_ms? }
  → { leader_epoch, events: bytes /* msgpack Vec<ChangelogEvent> */,
      gap_at?, current_version }
     // wait_ms — long-poll: если событий нет, подождать до wait_ms.
     // Базовый механизм R0/R1; работает и без hanging-состояния на лидере.

ReplStream   { db, repo, from_version }
  → поток PushEnvelope { push: Event|Gap, sub, seq, data: ChangelogEvent }
     // Оптимизация поверх Pull (переиспользует subscription-механику,
     // но privileged: сырые события, без де-интернирования и фильтров).
     // Catchup при Lagged/Gap — всегда через ReplPull.

ReplInternerSync { db, repo, table, from_id }
  → { entries: [(u64, name)], high_water }
     // Закрывает G1. Interner append-only ⇒ дельта = ключи с id > from_id.
     // Вызывается follower'ом при decode-miss и после connect.

ReplStatus   { node_id, applied: [{db, repo, version}] }
  → { leader_epoch }
     // Heartbeat + lag-репорт лидеру (метрики §6.2); в R2 — канал
     // semi-sync ack'ов.
```

### 5.2 Правила epoch (VR-style fencing)

- Лидер хранит `leader_epoch: u64` (персистентно; bump при promote).
- Каждый ответ/пуш несёт epoch. Follower запоминает максимум виденного;
  сообщение с epoch < виденного → соединение закрывается с
  `StaleLeaderEpoch` (защита от «воскресшего» старого лидера).
- Promote нового лидера (R3) обязателен с epoch-bump'ом; старый лидер при
  виде большего epoch самодемотируется в follower.

### 5.3 Wire-выбор: pull как база, stream как ускорение

Kafka-урок: pull-модель делает flow-control тривиальным (follower тянет
со своей скоростью, лидер не буферизует per-follower) и переживает любые
обрывы (bookmark + идемпотентность). `ReplStream` — чистая latency-оптимизация
(избегаем poll-паузы), деградирующая обратно в pull на любом сбое. R0
реализует только Pull; Stream — R1/R2.

### 5.4 Серверные аккаунты и модель доверия

**Принцип: «подключился сервер → всё автоматически» — анти-паттерн.**
Никакой автоматической полной репликации по факту подключения не
существует; всё — explicit grants, least privilege. Pull-модель делает
это естественным, потому что доверие в ней **двунаправленно и явно с
обеих сторон**:

- **Follower решает, кого слушать.** Он сам инициирует outbound-соединение
  на `leader_addr` из своего конфига (+ TLS server-cert validation +
  epoch-контроль §5.2). Поток, «принесённый» произвольным подключившимся
  сервером, невозможен по построению — follower не принимает входящие
  реплика-потоки вовсе.
- **Лидер решает, что отдавать.** Отдаёт только то, что разрешают grants
  серверного аккаунта, которым представился follower. От follower'а лидер
  не принимает **ничего**, кроме `ReplStatus`-heartbeat'а (данные вверх не
  текут; двусторонний обмен — только P2P-фаза R4, и там тоже под grants).

**Серверный аккаунт** (node account) — это Actor существующего Shomer'а,
созданный админом явно, с ролью `replicator` и repo-уровневыми grants
(`ResourcePath` = `db://<db>/<repo>`, `Action::Read` для pull). Прил
`ReplHello`/`ReplPull` лидер прогоняет запрошенный repo через тот же
`authorize_access`, что и обычные операции — никакой параллельной системы
прав.

**Три класса доверия** (фиксируются как grants, не как флаг в протоколе):

| Класс | Что получает | Назначение |
|---|---|---|
| `cluster` | ВСЁ, включая системный store (users + password-hash'и, access tree, functions, interners) | DR/HA-узел — кандидат на promote; после failover обязан быть эквивалентом лидера, иначе логины/права на нём мертвы |
| `replica` | явный список `(db, repo)` данных; системный store — нет | read-scale, edge, аналитика |
| `peer` (R4) | двусторонний CRDT-обмен по grants | P2P/chat |

Что **не реплицируется никогда**: приватные TLS/Ed25519 ключи узла
(у каждого узла свои), локальные resume-тикеты, локальный конфиг.

**Sparse-режим для scoped-реплик (R2).** Полная репликация (класс
`cluster`) даёт плотный поток версий — apply-контракт §4.1 требует
`applied+1`. Для scoped `replica`-класса события чужих repos отсутствуют
в потоке by design, а внутри разрешённого repo плотность сохраняется —
конфликт не возникает. Если же в R2+ появится **фильтрация внутри repo**
(row-level по правам, как у live subscriptions), включается sparse-режим:
bookmark продвигается по `last_seen`, гап внутри разрешённого потока —
норма, `GapDetected` остаётся только для журнальных потерь (`gap_at`).
Фильтрованная реплика **непромотируема** — это следствие, не баг:
promote-кандидат обязан быть класса `cluster`.

**Bootstrap доверия по фазам:** R0/R1 — SCRAM-пользователь с ролью
`replicator` + grants (работает на сегодняшнем стеке, нулевой новый
механизм). R3 — node registry: серверные аккаунты первого класса с
Ed25519-pubkey узла (подпись `ReplHello`), персистентным `node_id` и
epoch-журналом — реестр всё равно нужен для failover, identity-механика
Ed25519 уже в стеке.

### 5.5 Потоки и профили репликации (publication/subscription-модель)

Обобщение §5.4 (prior art: PostgreSQL publication/subscription; Syncthing
send-only / receive-only / send-receive per device; Active Directory
naming contexts — schema/config/domain партиции с разными правилами).

**Ключевой унифицирующий факт:** `SystemStore` — это обычный repo
(`system`) с обычными таблицами `users`, `roles`, `settings`, `groups`,
`functions`, `validators`, `databases`, `repositories`, `tables`,
`function_folders` — на тех же движковых примитивах, что и user-данные.
Поэтому «реплицировать аккаунты» или «реплицировать настройки» — это НЕ
отдельный механизм, а **включение путей системного repo в поток**, с
гранулярностью вплоть до таблицы (`system/users`, `system/settings`).
Локальное/доменное уже разделено архитектурой: ktav-конфиг файла (порты,
TLS-ключи, пути, allocator) не живёт в system store и не реплицируется
никогда; system store — доменное состояние, реплицируемое по политике.

**Три понятия:**

1. **Stream (поток)** — правило `(scope, direction, mode)`:
   - `scope` — путь с glob'ами: `app/*` (data-repos), `system/users`,
     `system/settings`, … (гранулярность: db → repo → таблица);
   - `direction` — относительно узла-владельца: `pull` (тянуть с
     апстрима), `push` (отдавать апстриму — edge-collect, R2+), `both`
     (R4, CRDT);
   - `mode` — плотный (default) / sparse (§5.4, R2+).
2. **ReplicationProfile (шаблон)** — именованный набор stream-правил.
   Хранится в system store лидера (таблица `repl_profiles` — сама
   реплицируемая категория, так что cluster-узлы получают и профили);
   версионируется как обычная запись.
3. **Node account** — серверный аккаунт с назначенным профилем:
   `ALTER NODE <name> SET PROFILE <profile>`. Изменение профиля
   применяется ко всем аккаунтам с этим профилем — в этом и есть QOL
   шаблонов. Per-account overrides сознательно НЕ вводим: нужна другая
   комбинация — создай другой профиль (меньше комбинаторики, яснее аудит).

**Прежние «классы доверия» §5.4 переопределяются как предопределённые
профили** (семантика не меняется):

```text
profile cluster        = { */*  pull }                      # всё, включая system/*
profile replica(REPOS) = { REPOS pull }                     # без system/*
profile edge-collect   = { system/users pull,               # локальная аутентификация
                           edge/<node_id>/* push }          # телеметрия вверх
profile peer (R4)      = { ... both + CRDT }
```

`edge-collect` — ответ на «что-то в обе стороны, а что-то только чтение»:
направление — свойство **потока**, не соединения и не аккаунта. Транспорт
не меняется (узел по-прежнему сам инициирует соединение к апстриму);
`push`-поток — это op `ReplPush { db, repo, events }` в том же
соединении, применяемый апстримом через тот же apply-engine §4.

**Следствие push-потоков: лидерство — per `(db, repo)`, не per узел.**
Push безопасен ровно потому, что у пушимого repo единственный писатель —
edge-узел; для этого repo edge и ЕСТЬ лидер, а центр — его follower.
Никакого multi-master внутри одного repo не возникает. R1 реализует
вырожденный случай (один узел — rw по всем repos); формулировка «rw/ro —
свойство пары (узел, repo)» закладывается в модель сразу, чтобы
edge-collect в R2+ не потребовал пересмотра инвариантов.

**Безопасность потоков:** выдача `system/users` (password-hash'и) в
профиль — явное, auditable решение админа; deny-by-default сохраняется —
пустой профиль не даёт ничего. Каждый поток проходит `authorize_access`
на стороне отдающего (§5.4).

---

## 6. QOL-пакет

### 6.1 Конфигурация (ktav, `shamir-server`)

```toml
[replication]
role        = "follower"          # "leader" (default) | "follower"
leader_addr = "tls://10.0.0.1:4747"
user        = "repl"              # SCRAM-пользователь с ролью replicator
repos       = ["*"]               # или явный список "db/repo"
pull_limit    = 1000              # событий за ReplPull
poll_wait_ms  = 5000              # long-poll окно
reconnect_backoff_ms = { min = 200, max = 30_000 }   # + jitter
```

Тумблеры — в `shamir-tunables` (рядом с `JOURNAL_BACKFILL_LIMIT = 10_000`).

### 6.2 Метрики (Prometheus — exporter уже стоит)

| Метрика | Смысл |
|---|---|
| `shamir_repl_lag_versions{db,repo}` | `leader_current − applied` (follower) |
| `shamir_repl_lag_seconds{db,repo}` | now − timestamp_ns последнего applied |
| `shamir_repl_connected` | 0/1 состояние линка |
| `shamir_repl_applied_total{db,repo}` | счётчик применённых событий |
| `shamir_repl_gaps_total` | сколько раз ловили gap (повод для алерта) |
| `shamir_repl_followers{db,repo}` | (лидер) сколько реплик репортят статус |

`/readyz` follower'а: not-ready, пока `lag_versions > threshold` (реплика,
отставшая на час, не должна получать читателей за load-balancer'ом).

### 6.3 Операционка

- `shamir-server replication status` — таблица (db, repo, role, applied,
  leader_current, lag, link-state).
- `shamir-server replication promote` — ручной promote: epoch-bump, снять
  read-only, начать принимать writes. (Fencing старого лидера — §5.2.)
- Reconnect: авто, exp backoff + jitter, resume-тикеты (уже дают ~7×
  дешевле полного SCRAM) — follower переживает рестарты лидера без
  оператора.
- Гап/reseed: при `bookmark+1 < journal_floor` или `gap_at` — понятная
  ошибка в лог + метрика + инструкция (`reseed` из бэкапа / R2-snapshot);
  никакого тихого расхождения.
- Аудит: репликационные сессии — в существующий audit-HMAC лог (кто,
  когда, какие repos тянул).

### 6.4 Клиентский QOL (replication-lag лекарства, DDIA ch. 5)

- **`min_version` read barrier**: опциональное поле read-запроса; follower
  ждёт `applied ≥ V` (с таймаутом) прежде чем исполнять. Даёт
  read-your-writes тем, кто его просит, не платя за него всюду.
- Ошибка `ReadOnlyReplica` несёт `leader_addr` — клиентские SDK могут
  прозрачно ретраить write на лидера.

---

## 7. Фазы

| Фаза | Содержание | Закрывает | Оценка |
|---|---|---|---|
| **R0 — network changefeed pull-API** | `ReplPull` (+ `ReplHello`, `journal_floor`) как privileged op; роль `replicator` + repo-grants через Shomer (§5.4); **без** apply. Сразу полезен как CDC-фид для внешних консьюмеров. Это «step 1» из PLAN.md as-is. | — | S |
| **R1 — async follower** | apply-engine (§4, G3) + bookmark + read-only (G5-half) + `ReplInternerSync` (G1) + reconnect/backoff + метрики §6.2 + e2e (leader+follower на loopback-TLS, 2 процесса). DDL при этом **заморожен** (документированное ограничение). | G1, G3, G5½ | M–L |
| **R2 — bootstrap + schema + semi-sync** | snapshot transfer для новых реплик (temporal `AsOf(V)` скан → стрим → журнал с V+1) — закрывает G4-bootstrap; журнальная retention-политика (G4-disk); DDL/meta-репликация (G2: либо meta-writes в журнал, либо DDL-форвардинг с версионным барьером); `min_version` barrier; опциональный semi-sync ack (k-of-n, деградирующий в async по таймауту — MySQL semi-sync urok). | G2, G4 | L |
| **R3 — failover** | promote-hardening: epoch-fencing e2e-тесты, старый лидер самодемотируется; авто-failover (lease или Raft для *конфигурации*, не данных — PacificA-разделение) — только если появится реальная эксплуатационная нужда. | G5½ | M (авто: L) |
| **R4 — P2P / chat** | gossip-топология, update-anywhere через CRDT (Shapiro et al.) поверх того же event-потока; отдельный research-док, когда доберёмся. | — | XL |

Дисциплина PLAN.md §4 сохраняется: каждый шаг — research → implement →
zero-trust verify → отдельный коммит; не строить R2 раньше, чем R1 запросит.

### 7.1 Verify-пункты перед стартом R0/R1 (проверить в коде, не гадать)

1. **G4:** живёт ли changelog-журнал вечно или truncate'ится (grep retention
   у journal-store); если вечно — R0 может обещать bootstrap-с-нуля, а
   `journal_floor ≡ 0`.
1a. **G2-система:** проходят ли записи `SystemStore` (repo `system`) через
   tx-commit path — т.е. есть ли у системного repo changefeed «из коробки».
   Если да — репликация аккаунтов/настроек (§5.5) = обычный data-поток
   уже в R1; если нет — включение system-writes в журнал уходит в R2
   вместе с остальным G2.
2. **G1:** точный формат `InternerManager::persist` — таблица meta-store и
   ключи, чтобы `ReplInternerSync` читал её же, без параллельной правды.
3. **G2:** перечень всего, что пишется вне tx-commit путей (DDL, access,
   validators, function-registry) — это точный скоуп «DDL заморожен» R1 и
   скоуп репликации R2.
4. Идемпотентность `apply` на уровне MVCC single-log: подтвердить, что
   запись версии V, уже присутствующей в логе, обнаружима дёшево
   (сравнение с текущей head-версией cell'а).
5. Bench-гигиена: `changelog_read` уже держит baseline pull-пути; добавить
   bench на `apply_replicated` при его появлении (обязателен
   `bu::tune`-вызов — QUICK-mode правило).

---

## 8. Куда это ведёт (P2P/chat — контур)

R1–R3 дают дерево (лидер → каскады). «Chat / P2P» из имени проекта — это
mesh: узлы равноправны, пишут локально, сходятся через обмен событиями.
Наши заделы под это: `commit_version == MVCC version` + full-row события +
идемпотентный apply — уже язык обмена; не хватает (а) векторных часов /
site-id в версии, (б) CRDT-семантики для конфликтующих Put'ов (LWW-register
на записи — простейший старт, timestamp_ns уже в событии), (в) gossip-слоя.
Это сознательно вынесено в отдельный research (R4): дерево нужно продать
в прод раньше, чем mesh — и дерево же станет транспортной основой mesh'а.

---

## 9. Тест-стратегия

- **Unit:** идемпотентность apply (повтор, скип, гап); epoch-fencing
  (стейл-лидер отвергнут); read-only enforcement.
- **E2E (2 процесса, loopback TLS):** прецедент `mvp_e2e` уже гоняет
  полный TLS+SCRAM стек — поднять leader+follower, писать в лидера,
  читать с follower'а после lag-барьера; kill -9 follower'а посреди потока
  → рестарт → сходимость (bookmark + идемпотентность).
- **Property:** случайные последовательности Put/Delete + случайные обрывы
  соединения → финальные состояния лидера и follower'а бинарно идентичны
  (сравнение MVCC-логов).
- **Chaos (R2+):** партиции, медленный follower (backpressure через pull —
  лидер не деградирует), clock skew (не влияет — порядок по версии).
- Всё через `./scripts/test.sh` (`@e2e` scope); никаких raw `cargo test`.

---

## 10. Решения (сводка)

| Решение | Выбор | Отвергнуто |
|---|---|---|
| Уровень | Логический changelog (row-based) | физический WAL/SST-шиппинг; op-replay |
| Модель | Lazy master (leader→followers), pull-first | multi-master в v1 (Gray et al.) |
| Порядок | Существующий `commit_version` per (db, repo) | новая глобальная нумерация |
| Доставка | at-least-once + идемпотентный apply | exactly-once транспорт |
| Fencing | `leader_epoch` в каждом сообщении с R0 | «добавим при failover» |
| Транспорт | shamir-connect (TLS+SCRAM+push envelope) как есть | отдельный порт/протокол |
| Консистентность R1 | async + монотонные чтения + opt-in `min_version` барьер | sync-по-умолчанию |
| Failover | ручной promote (R3 — авто по нужде) | Raft с первого дня |
| Доверие узлов | explicit node accounts + grants (cluster / replica / peer); follower сам выбирает лидера, лидер отдаёт по grants | «подключился сервер → автоматически всё в обе стороны» |
| Node identity | R0/R1: SCRAM + роль `replicator`; R3: node registry с Ed25519 pubkey | новый механизм аутентификации с нуля |
| Гранулярность потоков | streams `(scope, direction, mode)` + именованные ReplicationProfile-шаблоны на node accounts; аккаунты/настройки = таблицы repo `system` в scope | отдельный механизм для реплики метаданных; per-account overrides |
| Направление | per-stream (`pull` / `push` / `both`), лидерство per `(db, repo)` | направление как свойство узла целиком |

---

_Research 2026-06-30. Следующий шаг — verify-пункты §7.1, затем R0
(network changefeed pull-API) — «cheapest next step; no new subsystem»
(PLAN.md §2, Movement C step 1)._
