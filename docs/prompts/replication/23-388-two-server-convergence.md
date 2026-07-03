בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# 388 — двухсерверный e2e конвергенции (leader+follower, JS проверяет)

> Контекст: репликация теперь ЖИВАЯ end-to-end. 386-c (27b13aab) поднимает
> SubscriptionSupervisor при boot: follower-сервер с `[replication]`-конфигом
> + активной подпиской в system/subscriptions реально тянет с leader'а.
> napi `repl` + Node-биндинг — #387 (d6f50550). Харнесс —
> `tests/e2e/helpers/server.js` (`startServer` пишет ktav + спавнит).

## Задача

Поднять ДВА реальных `shamir-server` (leader + follower), follower
реплицируется с leader'а, Node-клиент проверяет конвергенцию.

## Часть A — харнесс (server.js)

Расширить `startServer` (или добавить `startServerWithReplication`), чтобы
follower получал в ktav-конфиге секцию `replication` (по форме
`ReplicationConfig` из 386-c `crates/shamir-server/src/config.rs`:
`node_id`, `replicator_user`, `replicator_password`, `server_name`) —
сверь ТОЧНЫЙ ktav-синтаксис секции по тому, как ktav парсит другие секции
(security/observability) и как config.rs объявил поля. Follower'у нужны
креды replicator-аккаунта, заведённого на leader'е.

## Часть B — тест `tests/e2e/tests/17-replication-convergence.test.js`

По образцу 16-replication.test.js + двух-серверный setup. Поток:
1. Старт **leader** (обычный сервер). Через Node-клиент (admin): создать
   db `app` + repo `main` + таблицу `items`, создать replicator-аккаунт
   (`createScramUser("repl","pw",["replicator"])`), открыть доступ к
   `app/main` (chmod 0o777 или owner), записать несколько строк.
2. Старт **follower** с `[replication]`-конфигом (replicator_user="repl",
   password="pw", upstream=leader host:port). На follower'е (admin) создать
   ту же db/repo/таблицу `app/main/items` (схема должна существовать —
   apply_replicated пишет в существующую таблицу) + создать
   replication_profile со stream pull на scope app/main + create_subscription
   на leader-upstream с этим профилем (через repl-DDL builders — они
   исполняются 386-a; но ЧЕРЕЗ Node-клиент это admin batch execute).
   Supervisor follower'а (386-c reconcile-tick, 10s, или сразу при boot если
   подписка уже в конфиге) подхватит подписку и запустит loop.
3. **Проверка конвергенции:** poll follower через Node-клиент
   (`client.execute` SELECT from items) до появления leader-данных или
   таймаута (напр. до 30s, шаг 500ms — с учётом 10s reconcile-tick +
   pull-loop). Ассертить, что follower отдаёт те же N строк.
4. **Инкремент:** записать ещё на leader → follower догоняет.
5. **Read-only гейт (опц.):** если follower запущен с NodeMode ReadOnly —
   клиентская запись на follower → read_only_replica. (Если конфиг
   NodeMode на follower'е не проброшен — отметь, оставь на будущее.)

## Окружение (ВАЖНО)

`shamir-client-node` — MSVC-only, `npm test` требует `npm run build` (cargo
release + napi build) на MSVC-хосте. В этом окружении прогон, скорее всего,
НЕДОСТУПЕН. Тогда:
- Напиши харнесс + тест корректно (соответствие 16-test + server.js).
- Сверь ktav replication-секцию по config.rs/парсеру (чтобы follower
  реально сконфигурировался).
- НЕ проваливай задачу из-за невозможности `npm test` здесь — отметь, что
  нужен MSVC-хост; верификация — ревью + соответствие образцам.
- В финале ЧЁТКО: что написано, что требует MSVC-прогона, известные
  нюансы (reconcile-tick 10s → poll-таймаут должен это учитывать;
  подписка создаётся рантайм через admin batch, а supervisor реагирует
  reconcile-tick'ом — убедись, что poll-окно ≥ tick-интервала).

## Definition of done

- server.js умеет поднимать follower с replication-конфигом.
- `17-replication-convergence.test.js`: leader+follower, конвергенция +
  инкремент.
- Соответствие образцам (16-test, server.js); ktav-секция сверена с config.rs.
- Финал: файлы, MSVC-требования, нюансы тайминга, что оставлено на будущее.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
