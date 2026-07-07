בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-07-03 [replication-complete]

## Session summary

Эпическая сессия: с нуля построена, оттестирована и связана end-to-end вся
фича **Interconnected** (буква «I» из S.H.A.M.I.R.) — leader→follower
репликация. Работа шла через /babygoal + /babysit (heartbeat cron
`be33ed11`, теперь ЗАГЛУШЕН на Шаббат) конвейером: на каждый лист —
committed-бриф до запуска (prompt-first, `docs/prompts/replication/00..23`),
делегирование агенту (сначала /crush, потом по просьбе user'а — `om` =
Opus 4.8 через Agent tool), затем ЛИЧНАЯ верификация оркестратором (дифф +
перегон тестов, не по конверту агента) и коммит каждого листа отдельно.

**Готово (вся вертикаль, ~30 коммитов):**
- **Пре-рефакторинг PR0–PR4** (#367-371): смоук system-changefeed,
  `BatchOp::is_write()`, `SessionPermissions::has_role()`,
  `finalize_sync_post_publish` (общее ядро коммита, PR3), `NodeMode`
  read-only гейт.
- **R0 pull-API** (#377-379): wire-типы `DbRequest::Repl` (newtype,
  вложенные теги repl_op/repl_kind), leader `repl_handler` (роль
  `replicator` + per-repo authorize_access deny-by-default, long-poll,
  leader_epoch VR-fencing), e2e через TLS+SCRAM.
- **R1 apply-engine** (#381-384): `apply_replicated` (raw-apply через
  `MvccStore::apply_committed_ops`, идемпотентность по watermark,
  follower-локальные версии + chain re-emit), durable bookmark (leader-
  домен версий), фоновый pull-loop (epoch-fencing, §5.6 неблокирующе),
  leader→follower e2e-конвергенция.
- **Серверное исполнение #386** (390/391/392): 386-a storage+dispatch
  репл-DDL в repo `system`; 386-b `SubscriptionSupervisor` (scc-реестр,
  reconcile, ReplSource-фабрика); 386-c boot-wiring в ServerLauncher +
  `[replication]` Config-секция + reconcile-tick. Follower-сервер с
  конфигом подписки РЕАЛЬНО тянет с leader'а.
- **Клиентская поверхность** (#372-376): 10 репл-DDL ops, Rust + TS
  билдеры, cross-language byte-identical msgpack паритет (fixture).
- **Node e2e #387**: napi `repl`-метод + `16-replication.test.js`.
- **Три реальных бага пойманы+починены:** MVCC-attach ordering (#385,
  `49160a8a`); `TableManager.bindings_len` per-clone desync (#380,
  `6ed6d575`); `SortedIndexManager` каталог per-clone desync (#380 второй
  корень, `f2c97b1f`, найден om-агентом) — оба clone-desync класса:
  value-clone `TableManager` требовал Arc-sharing счётчиков/реестров.

**In-flight на момент паузы:** ничего не крутится. Последнее действие —
я собирался запустить #388 (двухсерверный JS e2e) через om-агента, но user
отклонил tool-use и попросил паузу на Шаббат. #388 возвращён в pending;
бриф `docs/prompts/replication/23-388-two-server-convergence.md` УЖЕ
закоммичен (`c49e8efb`) — готов к запуску после Шаббата.

**Ключевой инспектированный код:** `apply_replicated.rs`, `finalize.rs`,
`repl_handler.rs`, `supervisor.rs`, `prod_factory.rs`, `server_launcher.rs`,
`system_store.rs`, `admin_replication.rs`, `table_manager.rs`,
`sorted_index_manager.rs`, `tests/e2e/helpers/server.js`,
`crates/shamir-client-node/src/lib.rs`. Дизайн — `docs/roadmap/
REPLICATION.md` (+ инвариант §5.6 «репликация не блокирует commit-ack») и
`REPLICATION-CLIENT-SURFACE.md`.

## Active goal

none (был /babygoal-режим; babysit-cron удалён на Шаббат)

## TaskList

### in_progress
(пусто)

### pending
- #388 Двухсерверный e2e конвергенции в tests/e2e (leader+follower серверы, JS проверяет) — бриф закоммичен c49e8efb; ТРЕБУЕТ MSVC-хост для прогона (napi build); блокеры #386/#387 сняты (готовы)
- #389 WASM functions_lifecycle таймауты под полной параллельностью nextest — test-infra: расширить .config/nextest.toml override на весь класс WASM-тестов ИЛИ test-group для ограничения параллелизма; НЕ поднимать таймаут глобально как маскировку

### recently completed (последние 10)
- #392 386-c boot-wiring supervisor
- #391 386-b subscription lifecycle
- #390 386-a репл-DDL execution
- #387 Node e2e (napi repl + 16-test)
- #386 серверное исполнение репл-DDL (umbrella)
- #385 MVCC-attach ordering fix
- #384 R1-d leader→follower e2e
- #383 R1-c follower pull-loop
- #382 R1-b bookmark
- #381 R1-a apply_replicated
(+ #365-380 ранее — вся R0/R1/pre-refactor/client-surface)

### deleted this session
1 (#366 umbrella pre-refactor — декомпозирован в PR0-PR4)

## Decisions

- **om-агенты вместо /crush** (по просьбе user'а в этой сессии) — Agent tool
  subagent_type `om` (Opus 4.8). Прежде — /crush. Prompt-first дисциплина
  сохраняется для обоих.
- **Порядок «оба по очереди»**: сначала R1-ядро, потом DDL-слой (user выбрал
  вместо параллельного — избежать рассинхрона формы DDL с тем, что реально
  нужно R1).
- **apply_replicated: follower-локальные версии**, не leader's (refine §4.1)
  — нужно для chain-репликации + консистентности gate-floor; bookmark хранит
  leader-версию только для идемпотентности.
- **clone-desync баги → Arc-sharing**: `bindings_len` и sorted-index каталог
  wrapped в Arc (как `validator_bindings`/`IndexManager`) — value-clone
  `TableManager` требует общего состояния между DDL- и read-клонами.
- **#380 WASM-таймауты = отдельный класс** (не ACL-drift): legit-медленные
  WASM голодают по CPU под полной параллельностью → отдельная таска #389, НЕ
  маскировать таймаутом.
- **Двухсерверная конвергенция (#388) блокировалась #386** правильно —
  follower-серверу нужен триггер запуска loop'а (реализован 386-c boot).

## Open questions

- **#388 прогон** — код можно написать, но `npm test` требует MSVC-хоста
  (napi build: cargo release + napi build). В текущем окружении не
  проверяется рантаймом — верификация ревью + компиляция.
- **Реактивность supervisor'а** — сейчас reconcile-tick (10s), event-driven
  changefeed-watch на system/subscriptions оставлен TODO (386-c).
- **upstream-креды репликации** — сейчас единый shared replicator-аккаунт из
  Config `[replication]`; per-subscription credential store — TODO.
- **NodeMode follower'а через конфиг** — проброс read-only режима в
  boot-config не сделан (в #388 отмечено как «на будущее»).

## Repo state

```
?? docs/checkpoints/2026-07-02-replication-design.md
(рабочее дерево чисто — все листы закоммичены; crates/*/target untracked как обычно)
```

```
c49e8efb docs(prompts): brief for 388 two-server convergence e2e
27b13aab feat(server): wire SubscriptionSupervisor into server boot (386-c)
26958500 docs(prompts): brief for 386-c supervisor boot wiring
788fc702 feat(server): SubscriptionSupervisor — start/stop follower loops per subscription (386-b)
624a6d84 docs(prompts): brief for 386-b subscription lifecycle
```

_Следующий шаг после Шаббата: запустить #388 (бриф c49e8efb готов) через
om-агента, затем #389 (WASM-таймауты). Gate-статус на момент паузы:
@oracle 1608-1630/зелёный, @server 566/566, shamir-db 127/127, флейковость
устранена (2 clone-desync фикса)._
