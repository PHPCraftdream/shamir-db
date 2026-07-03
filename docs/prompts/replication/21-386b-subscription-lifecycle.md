בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# 386-b — subscription lifecycle: старт follower-loop + registry

> Контекст: 386-a (cd0d8fd6) персистит subscriptions в `system/subscriptions`
> (поля `name, upstream, publication, profile, state`). R1-c даёт
> `run_follower_loop` + `WireReplSource` + `ReplSource`
> (`crates/shamir-server/src/replication/`). Профили в
> `system/replication_profiles` (streams = scope+direction+mode).

## Задача

Связать декларативный конфиг с движком: активная subscription → запущенный
`run_follower_loop`, тянущий с `upstream` и применяющий на локальные
(db,repo) из scope привязанного профиля. Управление — pause/resume/drop.

## Дизайн (пинится здесь — §5.6-выровнен)

**`SubscriptionSupervisor`** в `crates/shamir-server/src/replication/`
(новый файл `supervisor.rs`):
- Держит `Arc<ShamirDb>` + реестр активных loop'ов:
  `scc::HashMap<String /*sub name*/, SubHandle>` где
  `SubHandle { cancel: CancellationToken, join: JoinHandle<()>, profile: String }`.
- **Boot-reconcile:** метод `reconcile().await` — прочитать
  `system/subscriptions` (через `ShamirDb`/SystemStore read; сверь как
  admin_replication.rs `read_all` читает), для каждой `state=="active"`
  подписки, которой НЕТ в реестре — стартовать loop; для отсутствующих/
  paused — отменить. Идемпотентно (можно звать повторно).
- **Реактивность:** подписаться на changefeed repo `system` (таблица
  `subscriptions`) через `ShamirDb::subscribe_changelog`/
  `read_changelog_from` (как это делает существующий subscription bridge) и
  на каждое событие звать `reconcile()`. ЛИБО (проще для первого шага, если
  changefeed на system сложен) — публичный метод `notify_changed()`, который
  дёргает reconcile, + вызвать его из места, где сервер знает о завершении
  admin-batch (или периодический reconcile-tick). Выбери РАБОТАЮЩИЙ путь;
  предпочти changefeed-watch (§5.6, event-driven), но если в объёме дорого —
  boot-reconcile + notify_changed с TODO на changefeed-watch, и отметь.
- **Старт loop'а для подписки:** резолвить профиль (`system/
  replication_profiles` по `sub.profile`), взять его streams → для каждого
  stream с `direction=pull` поднять `run_follower_loop` на (scope.db,
  scope.repo) с `WireReplSource`, подключённым к `sub.upstream` как
  replicator-аккаунт. Учётные данные upstream: для R1 — из конфига/env или
  фиксированные (upstream-строка может нести их, либо отдельная конфиг-
  секция; сделай минимально рабоче, отметь как это решено). CancellationToken
  → в SubHandle.
- **§5.6:** каждый loop — своя `tokio::spawn` задача, не блокирует сервер.

## Интеграция

- Supervisor создаётся при boot сервера (ServerLauncher/serve) и держится в
  server-state; `reconcile()` зовётся после старта. Найди, где живёт
  `ShamirDbHandler`/server-state, повесь supervisor рядом. Если полная
  wire-интеграция в ServerLauncher велика — сделай Supervisor
  самодостаточным + юнит-тестируемым (конструктор от `Arc<ShamirDb>` +
  ReplSource-фабрика), а boot-подключение минимальным, отметив остаток.
- **ReplSource-фабрика:** чтобы тестировать без реального upstream — параметр
  `source_factory: Fn(&Subscription) -> Arc<dyn ReplSource>`, в проде
  создающий `WireReplSource`, в тестах — `InProcessReplSource` (R1-c) на
  leader `Arc<ShamirDb>`.

## Тесты (shamir-server, InProcessReplSource-фабрика)

Leader `Arc<ShamirDb>` + follower `Arc<ShamirDb>`. На follower'е создать
профиль+subscription (через admin batch, 386-a). Supervisor с фабрикой,
дающей InProcessReplSource на leader:
1. **create→converge:** записать на leader, `reconcile()`, дождаться →
   follower применил, bookmark вырос, данные читаются.
2. **pause:** alter_subscription pause → `reconcile()` → loop отменён
   (bookmark не растёт при новых записях на leader).
3. **resume:** resume → reconcile → снова догоняет.
4. **drop:** drop_subscription → reconcile → loop остановлен, из реестра убран.

(Loop'ы завершай через CancellationToken/`max_iterations` в тестах —
не бесконечный sleep.)

## Гейт

- `./scripts/test.sh @server` зелёный.
- `cargo fmt` тронутых крейтов `--check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- `SubscriptionSupervisor` + реестр + reconcile + старт/стоп loop'ов по
  state, ReplSource-фабрика для тестируемости.
- 4 теста (create/pause/resume/drop) на InProcessReplSource зелёные.
- §5.6 неблокирующе; boot-подключение (или обоснованный минимум + TODO).
- Финальное сообщение: как решён changefeed-watch vs reconcile-tick, как
  upstream-креды, boot-интеграция, вывод test.sh, остаточные риски.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
