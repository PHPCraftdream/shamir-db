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

## PR1: актуальный durability contract (дополняет исторический дизайн)

Ниже остаётся research/design 2026-06-30, не перечень shipped API.
PR1 добавляет серверный `durability` default: `wal_commit: buffered`,
retention 1 GiB, overflow `refuse_writes`, `replication_serve: durable`.
Файловый `buffered` ack — локальный WAL write; process crash и power loss
различаются. 250 мс — номинальный background fsync timer, не максимальное
окно потерь (storage errors/scheduler/I/O могут его растянуть). `synced`
ждёт coalesced WAL fsync. Mem/hybrid не persistent: tier игнорируется с
одним warn при open. Retention floor проверяет общий `resolve`:
`>= 2 × effective segment size`, включая env override
`SHAMIR_WAL_SEGMENT_MAX_BYTES`; PR2 применяет лимит и выбранное действие. `refuse_writes` отклоняет новые commits при превышении бюджета запечатанного WAL. `declare_gap` надёжно записывает GAP журнала и удаляет только WAL ниже безопасной границы history/interner; недоступный writer приводит к отказу в новых записях. После удаления объём запечатанного WAL измеряется заново. Runtime-checkpoint сначала сохраняет recovery floor не ниже J, затем продвигает J по settled-префиксу, покрытому событиями либо persisted GAP. Холодная сверка событий остаётся PR4: checkpoint не устраняет все GAP после сбоя. Для in-memory/hybrid без persistent WAL checkpoint и retention gate не применяются.

Durable Pull bound = `min(confirmed, W, R)`, R — **непрерывный префикс
recoverability**, не максимальная fsynced версия. Если R недостаточен,
pull создаёт coalesced demand sync и ограниченно ждёт продвижения R;
ошибка/таймаут оставляет старый safe bound. Это не ожидание очередного
250 мс tick и не ожидание follower на commit path. Локальный WAL ack
и разрешение безопасной выдачи follower — разные границы.
С PR8 `replication_serve: completed_reseed` разрешён: отдача
`min(confirmed, W)` без R-clamp (см. раздел S5c-X PR8 ниже);
fast serve, epoch bump и universal fencing реализованы этой настройкой.

Метрики, реально зарегистрированные сейчас: `shamir_wal_synced_lag_versions`,
`shamir_wal_sync_now_calls`, `shamir_wal_sync_errors`,
`shamir_replication_pull_held_back_ns` и `..._held_back_count`
(total-ns/count, не histogram). Series имеют labels db/repo и repo-owned
источники. Sync calls учитываются сразу; rounds после завершения, duration
включает snapshot replay и sync. Это не атомарный certificate. Таблица §6.2
ниже — дизайн follower telemetry, не свидетельство наличия всех series.

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
  **`gap_at`-сигнал** в read-API остаётся для легаси/retention-потерь; с
  PR5b переполнение канала (4096 слотов) больше **не** создаёт drop+gap:
  версия попадает в lock-free in-memory repair-pending set
  (`shamir_tx::changefeed::RepairPending`), фоновый дренаж заново ставит в канал
  событие, восстановленное из сохранённой WAL-записи, барьеры writer'а и
  checkpoint-ceiling зажимаются ниже старейшей repair-pending версии, а
  durable GAP объявляется только когда WAL-запись утрачена (retention) или
  writer закрыт.
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

### 4.4 Модель доверия payload'у apply-пути (threat model, #1199)

**`apply_replicated`** (`crates/shamir-engine/src/tx/apply_replicated.rs`,
уже реализовано в рамках R1-a) применяет `event.changes` как **TRUSTED raw
write**: НИКАКИХ validator/schema/DAC/integrity-проверок над содержимым
`RecordChange.value` не выполняется — байты идут напрямую в MVCC
version-log (`MvccStore::apply_committed_ops`) или в базовый store для
неприсоединённых таблиц. Это осознанное архитектурное решение (см. module
doc `apply_replicated.rs` строки 1–83): follower — исполнитель уже
решённого лидером результата, а не пересчитывающий его валидатор.

**Явная precondition, которую этот модуль НЕ проверяет сам и полагается,
что её обеспечивают сиблинг-крейты:** вся безопасность apply-пути держится
на том, что байты `ChangelogEvent` пришли по УЖЕ АУТЕНТИФИЦИРОВАННОМУ и
целостному транспортному соединению от легитимного leader/upstream-узла —
TLS 1.3 + SCRAM-Argon2id + Ed25519 identity (§2.3) и серверный аккаунт с
grants, прошедший `authorize_access` на стороне ОТДАЮЩЕГО (§5.4). Это
ответственность `shamir-connect` / `shamir-transport-tcp` /
`shamir-transport-ws` (установка и аутентификация соединения) и
`shamir-server`'s replication supervisor (какое соединение вообще
допускается до чтения changefeed) — НЕ `shamir-engine`, который в принципе
не видит транспортный уровень и не может независимо проверить, что событие
действительно пришло от заявленного отправителя.

**Failure scenario, если precondition нарушена:** неаутентифицированный
replication-эндпоинт, скомпрометированный upstream, либо
скомпрометированное промежуточное звено цепочки (события ре-эмитятся
дальше вниз по цепочке через `reproject_for_downstream` БЕЗ повторной
проверки) может протащить произвольные/испорченные байты записи; follower
применит их без вопросов и будет раздавать как валидные данные своим
собственным клиентам и дальше по цепочке репликации.

**Что реализовано / что отложено (#1199, P2):**
- **Реализовано:** явная документация precondition (этот подраздел) +
  doc-комментарий в `apply_replicated.rs`, указывающий сюда; и минимальный
  opt-in **`ApplyPolicy`** (`apply_replicated.rs`) с двумя вариантами:
  - `ApplyPolicy::Trusted` (default, byte-identical к пред-#1199 поведению)
    — не проверяет ничего, ровно как описано выше. В сервере это default
    при отсутствующем или false `replication.validate_replicated_payload`.
    Opt-in `replication: { validate_replicated_payload: true }` (#1216)
    передаётся через supervisor в `FollowerLoopConfig`; политика выбирается
    один раз при старте `run_follower_loop`. Проверка добавляет стоимость
    декодирования каждого Put; отказ сохраняет bookmark и retry cursor,
    останавливает batch и использует существующий backoff.
  - `ApplyPolicy::ValidatePayload` — ДО выделения версии (отклонение не
    расходует версию, идентично контракту `Skipped`) декодирует каждый
    `Put`'а `value` тем же двухшаговым декодером, которым
    `table/read_exec.rs` классифицирует `corrupt_records`
    (`RecordView::new`, затем fallback на `InnerValue::from_bytes` для
    legacy-кодированных строк); событие с payload'ом, не декодирующимся
    НИ ОДНИМ из двух способов, отклоняется целиком (`Err`, без частичного
    применения). Это НЕ re-run schema/validator/DAC — только "байты вообще
    похожи на запись, а не на мусор".
- **Отложено (не в скоупе #1199/#1216):**
  1. Полноценный re-run schema/validator/DAC на follower при приёме —
     НЕ реализован. Обоснование: DAC на follower бессмысленен (actor в
     событии — leader-asserted provenance для истории/аудита, НИКОГДА не
     вход авторизации на follower — у follower нет независимого способа
     проверить, что именно ЭТОТ actor реально инициировал операцию на
     лидере, кроме доверия самому транспорту, что и так уже требуется);
     integrity-подпись против скомпрометированного лидера бессмысленна
     (лидер, будучи скомпрометирован, подпишет что угодно — транзит уже
     защищён TLS §2.3); schema/WASM-validator re-run требует interner
     (дыра G1, ещё не закрыта) и validator-registry, которые живут в
     `shamir-db` — крейте ВЫШЕ `shamir-engine` по графу зависимостей,
     недоступном отсюда без структурной инверсии, аналогичной §1199's
     `AccessGate`. См.
     `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-engine/security-crypto.md`
     находка #4 для исходного описания.
  Deployments могут включить структурную проверку через
  `replication.validate_replicated_payload` или передать
  `ApplyPolicy::ValidatePayload` напрямую через `RepoInstance::apply_replicated`;
  полный нулевой trust к транспорту как таковому по-прежнему опирается
  исключительно на аутентификацию соединения (§5.4).

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

### 5.2a Универсальный identity-fence (S5c-X PR7)

Помимо epoch-fencing действует обязательный identity-fence на КАЖДОЙ
подписке, независимо от наличия seed-квитанции: при первом успешном
Hello/Pull follower записывает идентичность лидера `(repo lineage,
history epoch)` в один durable record со своим bookmark (один атомарный
`set`+`flush` того же значения bookmark в `__tx__`; каждый advance
сохраняет identity). Каждый последующий Hello и
каждый Pull сверяется с записанным ДО применения каких-либо событий;
расхождение lineage или epoch — терминальная остановка с типизированной
причиной (`seed_wrong_identity` / `seed_wrong_history_epoch`) и
структурированными `recorded_identity`/`observed_identity` в
per-repository `ReplicationStatus`;
счётчик `shamir_repl_identity_mismatch_total` (также для seeded).
Отсутствующая identity запрещает первый контакт; production disable-switch
нет. Seed schema-check остаётся строже, но решение об отказе одно. Подписки без seed-квитанции,
созданные до этого изменения, принимают первую наблюдаемую идентичность
ровно один раз (одна WARN-строка в логе). Лидеры на `in_memory`/`hybrid`
хранят `__tx__` только в RAM, поэтому каждый старт процесса порождает НОВЫЙ
lineage — перезапущенный лидер честно останавливает своих followers; durable
(`fjall`) лидеры сохраняют идентичность между рестартами.

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

### 5.6 Инвариант: репликация НИКОГДА не блокирует commit-ack

**Жёсткое правило.** Ни pull, ни push, ни доставка ресиверам не смеют
удлинять критический путь коммита и задерживать возврат результата
пользователю. На коммите событие **мгновенно (неблокирующе) кладётся в
очередь**, а фоновая задача уже отдаёт его ресиверам/follower'ам. Commit
подтверждается по локальной политике WAL (`buffered` — write, `synced` —
успешный coalesced fsync); доставка follower к этому моменту отношения не
имеет. Запуск durable-пути сам по себе не заменяет synced ack.

Это уже выполняется базовой механикой changefeed и не требует нового
кода на commit-пути:

- `RepoInstance::emit_changefeed_event` (в `finalize_sync_post_publish`,
  PR3) вызывает `feed.emit`, а тот — `tokio::broadcast::Sender::send`:
  отдаёт мгновенно, при отставании ресивера **дропает в его канале, а не
  тормозит отправителя**. Backpressure на коммит-путь отсутствует по
  построению.
- Долговечность гарантирует **журнал** (`emit_journal_only` пишется
  всегда, §2.1), не живой канал. Медленный/отвалившийся ресивер догоняет
  через `changes_since` (pull) — то есть «мгновенная очередь + фоновый
  дренаж» и «pull-догон» — две стороны одного инварианта.

**Требования к реализации R0/R1 (не нарушить инвариант):**

- `ReplStream`/`ReplPush`-доставка выполняется в **отдельной фоновой
  задаче** (свой `tokio::spawn`/канал per-соединение), НИКОГДА не в
  commit-タスке и не под `commit_lock`.
- Очередь доставки — **bounded, с политикой drop-oldest → деградация в
  pull** (follower подтянет пропуск через bookmark + `changes_since`).
  Заполнение очереди медленным ресивером НЕ создаёт backpressure на
  писателя: реплика отстаёт (async EL по PACELC, §3.3), коммит-latency
  не растёт.
- Ошибка/разрыв ресивера — best-effort log + продолжение (как текущий
  `Err`-branch `emit_changefeed_event`), НИКОГДА не проваливает коммит.

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

### 6.2 Метрики (план follower telemetry; exporter уже стоит)

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
- Гап/reseed: `gap_at` останавливает follower до применения payload и
  требует операторского восстановления, если residual gap пересекает его
  bookmark/следующую версию. Переполнение журнального канала (channel-full,
  PR5b) само по себе больше не создаёт gap: событие откладывается в
  repair-pending и восстанавливается из WAL, так что после overflow reseed
  не нужен. После S5c-X PR4 unclean restart сам по себе не
  означает GAP: cold open восстанавливает выводимые события v3 из WAL;
  неизвестный legacy WAL или отсутствующие evidence могут оставить
  минимальный консервативный gap. Offline seed остаётся ручным; автоматического
  или online snapshot/reseed нет.
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
| **R2 — bootstrap + schema + semi-sync** | **Статус: частично доставлен как OFFLINE R2-lite (SEED-1..5, не ONLINE snapshot).** Доставлено: identity-метаданные (repo lineage + history epoch в Hello/Pull), офлайн-экспорт логического состояния остановленного лидера (`replication-seed-export`), изолированный crash-safe инсталлер с durable-квитанцией (`replication-seed-install`), pause+validated Resume с типизированными отказами (`seed_*`), operator runbook (`guide/09-seed-recovery.md`) и CI-сертификация (`tests/seed_recovery_e2e/`). НЕ построено (остаётся этим пунктом роадмапа): онлайн snapshot transfer, автоматический reseed, DDL/meta-репликация (схема заморожена, расхождение = `seed_wrong_schema`), retention-политика журнала, `min_version` barrier, semi-sync ack. Офлайн-сид требует ОСТАНОВЛЕННОГО лидера, Unix, scope = один `(db, repo)`, и не восстанавливает потерянные лидером записи. | snapshot transfer для новых реплик (temporal `AsOf(V)` скан → стрим → журнал с V+1) — закрывает G4-bootstrap; журнальная retention-политика (G4-disk); DDL/meta-репликация (G2: либо meta-writes в журнал, либо DDL-форвардинг с версионным барьером); `min_version` barrier; опциональный semi-sync ack (k-of-n, деградирующий в async по таймауту — MySQL semi-sync urok). | G2, G4 | L |
| **R3 — failover** | promote-hardening: epoch-fencing e2e-тесты, старый лидер самодемотируется; авто-failover (lease или Raft для *конфигурации*, не данных — PacificA-разделение) — только если появится реальная эксплуатационная нужда. | G5½ | M (авто: L) |
| **R4 — P2P / chat** | gossip-топология, update-anywhere через CRDT (Shapiro et al.) поверх того же event-потока; отдельный research-док, когда доберёмся. | — | XL |

**S5c-X PR4 — cold reconciliation: реализовано; только cold path,
unbenchmarked.** До serving recovery идемпотентно восстанавливает
отсутствующие выводимые v3 journal events из surviving WAL; совпадающие
записи не меняются, несовпадающие persisted bytes и I/O/decode ошибки
закрывают open. Eventless v3 entry не создаёт GAP. Unknown legacy entries
или отсутствующие evidence могут оставить минимальный консервативный GAP;
pre-existing GAP не очищаются. Journal и recovery floor flush'ятся до записи
J=H, затем J сохраняется до serving. Unclean shutdown сам по себе не требует
reseed. PR5 (runtime orphan repair) и PR5b (channel-full repair: переполнение
канала на 4096 слотов откладывается в in-memory repair-pending set и
восстанавливается из WAL; durable GAP — только при утраченной WAL-записи или
закрытом writer'е) реализованы; с PR6c WAL-less follower
при холодном старте сертифицирует журнал через J из своей bookmark-записи,
оставляя минимальный uncertain GAP только для in-flight батча (retry-safe
receipts PR6b не реализованы). PR4 не
меняет commit hot path; benchmarks не запускались.

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
| Доставка vs commit | мгновенный неблокирующий enqueue + фоновый дренаж; репликация НИКОГДА не удлиняет commit-ack (§5.6) | синхронный push ресиверам на commit-пути / backpressure на писателя |

---

_Research 2026-06-30. Следующий шаг — verify-пункты §7.1, затем R0
(network changefeed pull-API) — «cheapest next step; no new subsystem»
(PLAN.md §2, Movement C step 1)._


### Per-repository durability overrides (S5c-X PR1b)

The server-level durability configuration remains the default for repositories. An optional sparse override may be supplied when creating a repository and changed at runtime through `ALTER REPO`. ALTER merges only non-null fields into the stored override; omitted fields retain their existing override, and fields never set continue to inherit the server default. The effective policy is live-swapped into the shared policy used by that repository and persisted under `durability` in its `system.repositories` record. Older records without this key decode as no override. On boot, a persisted override is resolved over the server default.

`DESCRIBE REPO` exposes both effective `durability` and stored `durability_override`, together with `lineage_id` (hex) and `history_epoch`. Accepted settings are `wal_commit` (`buffered` or `synced`), positive `wal_retention_max_bytes`, `on_wal_retention_exceeded` (`refuse_writes` or `declare_gap`), and `replication_serve` (`durable`, the default, or `completed_reseed`). Unknown fields and invalid enum values fail validation. Retention must be at least twice the effective WAL segment size. In-memory and hybrid WALs ignore `wal_commit`; this setting does not make them durable. Rust request DTOs always carry retention as a decimal string. TypeScript accepts positive safe numbers, bigint, or canonical decimal text and normalizes values above i64::MAX to strings. In effective responses, values through i64::MAX are native MessagePack integers and decode as number or bigint according to wire integer width; values above i64::MAX are decimal strings.

### Fencing & reseed after an unclean restart under `completed_reseed` (S5c-X PR8)

Under `completed_reseed` the leader serves `min(confirmed, W)` — the completed prefix WITHOUT the R clamp. This is safe by construction, not lossless: before the first speculative page is served, a durable lifecycle record `Armed` is written and flushed in the per-repository `__tx__` info store (an atomic fast path avoids re-reading it per pull). A clean shutdown — stopped admission, settled writes, durable data/WAL, journal horizon via `record_clean_horizon`, allocation evidence — writes the `Clean` certificate AFTER all of that landed; under `durable` mode shutdown writes nothing. On open under `completed_reseed`, an `Armed` record with no later `Clean` certificate means an unclean restart: recovery completes privately, `history_epoch` is then durably incremented (checked add; a failed bump/flush or overflow BLOCKS admission), the old replication continuity is invalidated — the lineage stays STABLE, only the epoch moves — and only then is service admitted. This costs a reseed even if the WAL bytes survived; it is conservative by design. The guarantee is "safe convergence, not lossless replication: speculative observations may be retracted, and an unclean restart conservatively costs a reseed even if the bytes survived."

Fencing binds every subscription/bookmark to `(lineage, history epoch)`: the PR7 identity fence runs at Hello and before every Pull, before any decode/apply, whether or not a seed receipt exists. A follower whose bookmark carries the old epoch after a bump is refused; the supervisor marks the subscription `resync_required` (also exported as counter `shamir_repl_resync_required_total`) and it stays quarantined until a replacement seed is installed — seed replacement converges the follower onto the new epoch. Legacy peers that omit lineage/epoch identity are refused (`MissingIdentity`), fail-closed, never adopted. Switching `durable` → `completed_reseed` with an old `Armed` record is safe (the next fast-mode open bumps once); switching back never erases the record, which is inert under `durable`. In-memory/hybrid repositories mint a new incarnation/new lineage at every reopen (RAM-backed identity store) and may serve replication only with explicit `completed_reseed`.

Cost, stated exactly: the commit path is unchanged; the serve path gains one atomic policy/mode load per replication page read and, on the first speculative pull after open, one Armed store write with a flush; the unclean-open epoch bump is one metadata key write + flush on the recovery path, never on the commit path. `wal_commit` (`buffered|synced`) is independent of `replication_serve`: a request's `durability` can strengthen an acknowledgement but can never weaken the repository's serve policy, and the WAL-less follower apply path keeps its durable-floor-before-history ordering regardless of mode. Observable state: `DESCRIBE REPO` carries `durability` (effective, incl. `replication_serve`), `durability_override`, `lineage_id`, `history_epoch`; gauges `shamir_replication_serve`, `shamir_replication_served_bound`, `shamir_wal_watermark_w`; counters `shamir_repo_history_epoch_bumps_total` plus the existing identity-mismatch and new resync-required counters.

Builder-only API examples belong in user guides, not this replication wire/protocol specification.
