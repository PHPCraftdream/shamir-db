בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# 386-c — подключить SubscriptionSupervisor к boot сервера + changefeed-watch

> Контекст: 386-b (788fc702) дал `SubscriptionSupervisor`
> (`crates/shamir-server/src/replication/supervisor.rs`) —
> самодостаточный, но НЕ конструируется при старте. R1-c даёт
> `WireReplSource`. Разблокирует #388 (двухсерверный e2e).

## Задача

Поднять supervisor при старте сервера, чтобы follower-сервер реально
начинал тянуть с leader'а по своим подпискам.

## Точка врезки

`crates/shamir-server/src/server/server_launcher.rs`, после конструирования
`handler_concrete` (~строка 315), где `shamir: Arc<ShamirDb>` уже доступен.

1. **Prod ReplSourceFactory** → создаёт `WireReplSource` (R1-c) на
   `Subscription.upstream` как replicator-аккаунт. Креды: минимально рабоче
   — из конфига (новая опц. секция `[replication]` в Config/ktav:
   `replicator_user`/`replicator_password` или per-upstream map) ИЛИ env;
   выбери минимальный работающий путь, отметь TODO для полноценного
   per-subscription credential store. WireReplSource подключается к upstream
   через `shamir_client::Client::connect` — сверь его сигнатуру (TLS
   no-CA + SCRAM, как e2e).
2. **Сконструировать supervisor** `SubscriptionSupervisor::new(shamir.clone(),
   factory, node_id)` (node_id — из конфига или сгенерённый/хостнейм).
3. **reconcile()** после старта листенеров (там, где сервер уже готов
   принимать; НЕ раньше, чтобы follower не дёргал upstream до готовности).
4. **Хранить supervisor** в `ServerHandle` (server_handle.rs) — иначе он
   дропнется и loop'ы умрут. Добавить поле + при `shutdown` отменить все
   подписки (supervisor cancel-all).
5. **Реактивность (changefeed-watch, §5.6 — предпочтительно):** фоновая
   `tokio::spawn` задача, читающая changefeed repo `system` (таблица
   `subscriptions`) через `ShamirDb::subscribe_changelog`/
   `read_changelog_from` (как subscription bridge) → на каждое событие
   `supervisor.notify_changed()`. Если полноценный watch дорог — периодический
   reconcile-tick (напр. каждые N сек) + отметь TODO на event-driven. Выбери
   РАБОТАЮЩИЙ путь.

## Config (минимально)

Добавить в `Config` опц. `replication: Option<ReplicationConfig>` с полями
для replicator-кред и (опц.) node_id. serde + ktav-парсинг по образцу
других секций (`security`/`observability`). Дефолт None → supervisor всё
равно поднимается (просто без кред тянуть не сможет — reconcile для пустых
подписок no-op). НЕ ломать существующие конфиги (поле optional с serde
default).

## Тесты

- Юнит/интеграция в shamir-server: сервер стартует с supervisor'ом (пустые
  подписки) — boot не падает, shutdown чистый.
- Если можешь without-MSVC: интеграционный тест, что при наличии активной
  подписки в system/subscriptions после boot supervisor её подхватывает
  (можно через тестовую фабрику-инъекцию, если prod-WireReplSource требует
  реального upstream — тогда оставь prod-путь под #388, а здесь проверь boot
  + reconcile-вызов + shutdown).
- Существующие @server и mvp_e2e тесты НЕ должны сломаться (boot-путь
  изменён).

## Гейт

- `./scripts/test.sh @server` зелёный (особенно mvp_e2e — boot не сломан).
- `cargo fmt` тронутых крейтов `--check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- Supervisor конструируется при boot, reconcile после старта, хранится в
  ServerHandle, отменяется при shutdown.
- Реактивность (changefeed-watch или reconcile-tick) подключена.
- Config replication-секция (optional, не ломает существующее).
- @server + mvp_e2e зелёные.
- Финальное сообщение: как решены креды/node_id/реактивность, boot не сломан
  (mvp_e2e pass), вывод test.sh, остаточные риски (что оставлено на #388).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
