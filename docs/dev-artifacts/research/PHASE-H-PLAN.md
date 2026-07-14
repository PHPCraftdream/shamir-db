בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase H — Leader-Follower Replication (Movement C · шаг 3)

> **Статус:** созерцание + сформированная кампания, **ждёт слова пользователя о
> направлении**. Это документ-преемник `NEXT-CAMPAIGN.md` (выбравшего Phase E) и
> `PHASE-G-PLAN.md` (Phase G). Здесь: три линзы → ранжирование кандидатов →
> рекомендация (Phase H) → поэтапная декомпозиция H.0–H.5, заземлённая в коде
> (file:line). Реализация — по явной просьбе; коммит/пуш/агенты — по правилам.

---

## 0. Где мы стоим (после Phase G + кампаний ②/③)

`ACTION-ITEMS.md`: **все P0–P3 закрыты** (Phase D/E/G + кампании ②/③). С момента
написания этого документа **кампания ③ закрыла кандидат-③ ниже («DX sweep»)**:
e2e-eval добивка (`like/regex`/existence/`aggregateFn`/`func`/`history`/`page`),
computed-`DEFAULT`/server-stamping (transform-фреймворк), TS `litU64`/`bin`. Из
research-корпуса **open-работы не осталось**. Фундамент хартии **S·H·A·M·_·R**
закрыт; единственный открытый пилон — **«I» (Interconnected)** = Movement C
(`PLAN.md §2`). Нарративная нить проекта: «как запись пишется/управляется/
распространяется» → «как авторы пишут код внутри БД» → «темпоральность, вложенные
tx, duplex, TS-клиент» → «как **клиенты подписываются на живые данные**» (✅ сделано)
→ **следующий бит: «как реплики следуют за лидером»**. Литеральный следующий шаг —
**репликация**. Остались две большие кампании: **① Phase H (репликация)** —
рекомендуется, и **② Perf group-commit** (закрытый пилон «H», альтернатива).

---

## 1. Три линзы (что делало прошлые кампании сильными)

- **Линза Phase D** — реально недостающая *capability* с ценностью корректности.
- **Линза keyset** — *машинерия в движке уже есть*, нужен только surface →
  лучший ROI («engine-ready, surface-absent»).
- **Линза нарратива** — продолжает магистральную нить проекта, открывает
  закрытый пилон, а не полирует закрытый.

---

## 2. Кандидаты (ранжировано)

### ① Phase H — Leader-Follower Replication ⭐ ГЛАВНЫЙ ВЫБОР

Открывает пилон «I». Фундамент **поразительно готов** (см. §4): changelog-событие
— это уже write-ahead apply-log; журнал + live-broadcast + watermark + gap-signal
+ duplex-транспорт + resume-тикеты существуют. Недостаёт только **follower-стороны**
(apply-движок + узел-реплика + bootstrap).

| Линза | Оценка |
|---|---|
| Phase-D (capability) | ★★★★★ — это **сам charter-пилон**, не фича |
| keyset (engine-ready) | ★★★★☆ — changelog-as-apply-log + транспорт готовы; новое = applier + узел |
| Нарратив | ★★★★★ — буквальный следующий бит («реплики следуют за лидером») |
| Риск/скоуп | L, но **front-loaded**: вся correctness-критичность в H.1 (in-process, без сети, полностью тестируема) |

### ② Perf write-path — group commit (`remaining-optimizations-plan.md`)

Одна архитектурная идея: WAL как единственный носитель истины при коммите.
A (WAL v3 интернер-дельта) → B (сжать `commit_lock`) → C (write_set_keys) →
D (group commit ×N коммиттеров/fsync) → E (writev fan-out) → F (format bump).

- **Польза:** ★★★★☆ — ×N commit-throughput, измеримо.
- **Похожесть:** ★★★☆☆ — когерентный арк, но это **закрытый пилон «H»**
  (`PLAN.md`: Movement B DONE) → убывающая стратегическая отдача.
- **Риск:** ★★★★☆ высокий — SSI-ordering, инварианты критической секции коммита.

### ③ Completeness & DX sweep (P2/P3-остаток) — ✅ **ВЫПОЛНЕНО** (кампании ①/②/③)

E5 (unify uniqueness) — ✅ кампания ②.3. TS DX-билдеры (B5 `Doc`/`WriteValue`, B6
`Handle`, B7 `tryBuild`/`deliverCall`/interner) — ✅ кампания ①. e2e-eval добивка
(`like/regex`/existence/`aggregateFn`/`func`/`history`/`page`) + `litU64`/`bin` —
✅ кампания ③ (③.1b/③.3a). DbRequest-билдеры — ✅ by-design в SDK (①.4). Остаток
P2/P3 — только server-gated `resume()`/`commitMigration`/`dropUser`-e2e (не блокеры).

- **Польза:** ★★☆☆☆ DX-полировка. **Риск:** низкий. **Похожесть:** twin Phase E/G.
- Эта «разгрузочная» кампания УЖЕ отработана — остаются только две магистральные
  (① Phase H, ② Perf group-commit).

### Развилка

| # | Кампания | Пилон | Польза | Риск | Вердикт |
|---|---|---|---|---|---|
| ① | **Phase H — Репликация** | «I» (открыт) | ★★★★★ | L, front-loaded | **рекомендую** |
| ② | Perf group-commit | «H» (закрыт) | ★★★★☆ | высокий | сильная альтернатива |
| ③ | DX sweep | — | ★★☆☆☆ | низкий | разгрузка, не магистраль |

**Рекомендация: ① Phase H.** Пересечение всех трёх линз ложится на репликацию:
открывает единственный незакрытый пилон, фундамент engine-ready, риск front-loaded
в тестируемый H.1. Perf-кампания объективно ценна, но это уже закрытый пилон и
выше риск; брать её, если хочется забанчить throughput перед распределёнкой.

---

## 3. Что такое репликация здесь (и чем ОНА НЕ подписки)

`LIVE_SUBSCRIPTIONS.md` (#201, ✅ реализовано) пушит события **приложению** —
клиент реагирует на изменения. **Репликация** скармливает тот же поток
**follower-узлу**, который *применяет* записи к собственному стору, побайтно
реконструируя состояние лидера, и обслуживает read-only запросы. Разные
потребители одного потока: подписка → код приложения; репликация → apply-движок.

---

## 4. Фундамент — что УЖЕ есть (заземлено в коде)

| Примитив | Точка входа | Роль в репликации |
|---|---|---|
| **Apply-log событие** | `ChangelogEvent { repo, commit_version, tx_id, actor, timestamp_ns, changes: Vec<RecordChange{table,key,op:Put\|Delete,value?}> }` — `crates/shamir-tx/src/changefeed.rs:87` | Каждый `RecordChange` = атом применения: Put(value@key) / Delete(key) |
| **Catch-up pull** | `ShamirDb::read_changelog_from(db,repo,from,limit) -> Vec<ChangelogEvent>` — `changelog.rs:120`, ascending, resumable | Догон follower'а от своего watermark |
| **Gap-aware pull** | `read_changelog_from_journal -> JournalRead{events, gap_at: Option<u64>}` — `changelog.rs:153` | Триггер snapshot-resync при дыре |
| **Live stream** | `subscribe_changelog -> broadcast::Receiver<Arc<ChangelogEvent>>` — `changelog.rs:99` | Живой хвост после догона |
| **Watermark seed** | `current_commit_version(db,repo) -> u64` — `changelog.rs:145` | Старт/возобновление курсора |
| **Эмиссия (tx+non-tx)** | `commit.rs:536` (`project_event`) + `write_exec.rs:257` (`emit_nontx`); `commit_version` монотонен per-repo независимо от пути | Полнота лога: ни одна запись не теряется |
| **Server-push транспорт** | подписки: демукс push-кадров, `SubscribeChangelog{from_version}` с backfill+live, gap-маркеры — `crates/shamir-server/src/subscriptions/*` (✅) | Готовый провод для потока репликации |
| **Single-log MVCC** | состояние реконструируемо применением версионированных записей по порядку (`TEMPORAL.md`) | Корректность apply by construction |

**Крукс-риск (вынесен в дизайн H.0):** changelog-`value` — это MessagePack
`InnerValue` с **интернированными u64-ключами** (`changelog.rs:9-13`). Follower
обязан разрешать те же id → нужна **репликация интернера** (дельта рядом с
changelog для байт-идентичности) **или** де-интерн на лидере / ре-интерн на
follower'е (логическая идентичность). Это главное решение дизайн-доки.

---

## 5. Поэтапная декомпозиция Phase H

Принцип: **front-load корректность**. Вся распределёнка-флакость живёт в H.3+,
но вся *correctness*-критичность (идемпотентность, порядок, gap) — в H.1, который
тестируется in-process без сети. Commit-per-phase, RED→GREEN→zero-trust verify.

### H.0 — `REPLICATION.md` дизайн-доку (prompt-first, без кода) · S

Контракт кампании — durable-артефакт. Зафиксировать:
- **Единица:** per-repo changelog (`commit_version` монотонен per-repo).
- **Apply-семантика:** идемпотентность по `commit_version` (re-apply ≤ watermark
  = no-op); gap-aware (отказ на не-смежной версии → resync).
- **Порядок:** тотальный per-repo через `commit_version`.
- **Конфликты:** single-leader → follower read-only, конфликтов записи нет.
- **Интернер** (крукс §4): дельта-рядом-с-changelog vs де-интерн/ре-интерн —
  **решить здесь**.
- **Bootstrap:** snapshot@V0 + tail vs full-replay-from-0 (зависит от retention;
  `purge_history` может усечь журнал — открытый Q §7 `LIVE_SUBSCRIPTIONS.md`).
- **Durability:** follower персистит applied-watermark → рестарт резюмит от него.
- **Failover/election:** **явно отложено** (H.5, P2P-территория).
- **ACL:** follower-стрим проверяет `Action::Read` на repo-сторе (как
  `changes_since`).

**Выход:** `docs/dev-artifacts/roadmap/REPLICATION.md` (закоммитить ДО кода).

### H.1 — Follower apply-движок `ReplicaApplier` (in-process, без сети) · M ★

Ядро. Структура, применяющая `ChangelogEvent`'ы к целевому стору:
- Per-event (ascending): применить каждый `RecordChange` (Put: write value@key;
  Delete: remove key) в стор follower'а для `{repo,table}`.
- Durable applied-watermark per-repo (`AtomicU64`-зеркало + персист).
- **Идемпотентность:** skip событий `commit_version ≤ watermark`.
- **Gap-aware:** `commit_version > watermark+1` → `GapError` (caller → resync).
- **Интернер:** по решению H.0 (применить дельту первой / ре-интерн).

**RED-тесты (in-process, ноль сети):**
- apply [V1,V2,V3] → стор follower'а == стор лидера (одинаковые reads);
- re-apply V2 → no-op (watermark не двигается, нет двойной записи);
- apply V5 при watermark=V3 → `GapError`;
- Delete-событие удаляет ключ;
- рестарт: персист-watermark выживает, резюм с него.

**ROI:** высший, риск низший — чистая корректность, без распределёнка-флакости.
Engine-ready (changelog есть), surface-absent (applier'а нет).

### H.2 — Поток `ReplicateFrom` (переиспользовать транспорт подписок) · M

Непрерывный упорядоченный поток от watermark: catch-up (журнал) → live (broadcast).
**Решение:** переиспользовать существующий `SubscribeChangelog{from_version}` (он
уже делает backfill-from-version + live-bridge + gap-маркеры) ИЛИ добавить
выделенный `ReplicateFrom`, если репликации нужно больше (интернер-дельта, сырые
байты). Если поток подписки уже несёт всё для applier'а (`changes`+`value`) →
H.2 в основном «подключить клиента подписки к H.1-applier'у», минимум нового wire.

**Тест:** follower-консьюмер получает catch-up + live по порядку, кормит applier.

### H.3 — Узел `ReplicaFollower` + e2e-репликация · L

Follower-процесс: держит локальный `ShamirDb`, коннектится к лидеру как клиент
(resume-тикет), шлёт `ReplicateFrom(local_watermark)`, кормит поток в H.1-applier,
обслуживает read-only запросы на реплицированном сторе; **запись на follower
отвергается**.

**e2e (два `ShamirDb` / два сервера in-process):**
- write на лидера → follower отражает (eventually, version-ordered);
- kill+resume потока → follower догоняет с watermark (без gap, без дублей);
- gap-сигнал → follower триггерит resync (H.4);
- запись на follower → отказ.

### H.4 — Bootstrap / snapshot-трансфер · M

Холодный follower (нет данных / watermark позади retention журнала):
- **Вариант A:** full-replay от версии 0 (если журнал хранится с генезиса);
- **Вариант B:** snapshot-трансфер — лидер дампит текущее состояние таблиц @V0,
  follower грузит, затем тейлит от V0.
Решить по retention (для прода журналы не вечны → вероятно нужен snapshot).

**Тест:** пустой follower → bootstrap → == лидер; follower позади retention →
gap → snapshot-resync → == лидер.

### H.5 — (ОТЛОЖЕНО / отдельная кампания) Failover, election, multi-follower · L

Детект отказа лидера, промоушн follower'а, multi-follower fan-out, anti-split-brain.
Это consensus/P2P-территория (Raft-lite / gossip). **Явно вне Phase H** — не
строить наперёд. End-state P2P/chat надстраивается над этим.

**Порядок:** H.0 → H.1 → H.2 → H.3 → H.4 (H.5 deferred). Зависимости: H.1←H.0
(контракт), H.2←H.1 (applier-контракт стрима), H.3←H.1+H.2, H.4←H.3.

---

## 6. Дисциплина

Каждый шаг проектным путём: research → implement → **zero-trust verify** (диффы +
независимый зелёный гейт через `./scripts/test.sh`, бенчи в изолированном
`CARGO_TARGET_DIR`) → отдельный чистый коммит. Делегирование — prompt-first (бриф
в `docs/dev-artifacts/prompts/replication/` ДО агента; бриф запрещает git-мутации). `REPLICATION.md`
(H.0) — durable-артефакт, переживает потерю рабочего дерева.

---

## 7. Решение (ждёт пользователя)

Рекомендация — **① Phase H (Репликация)**, H.0→H.4, H.5 отложен. Альтернативы:
**② Perf group-commit** (забанчить throughput) или **③ DX sweep** (разгрузка).
Старт — по слову; таски заведутся при «делай».
