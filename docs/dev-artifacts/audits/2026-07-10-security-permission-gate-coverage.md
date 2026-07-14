בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью покрытия permission-гейта S.H.A.M.I.R. (защитный аудит)

_Агент: @fxx (max effort), 2026-07-10. Защитный аудит собственной кодовой базы (авторизованный, для укрепления). Фокус — enforcement-покрытие POSIX-подобной системы прав «Shomer»: проверка, что КАЖДАЯ входная точка исполнения операций над ресурсами проходит через `ShamirDb::authorize_access` (или эквивалентный `permits`-чек) для соответствующего действия из матрицы `docs/dev-artifacts/roadmap/ACCESS_HIERARCHY.md`._

**Метод.** Прошёл все call site `authorize_access` / `permits(` / `resource_meta(` по `crates/**`, затем обратным ходом — все входные точки исполнения (wire-`execute`/`execute_as`, interactive-tx `tx_*_as`, WASM `db_*`/`db_execute`, батчевый `Call`, подписки, репликация leader+follower, admin-DDL диспетчер) — и для каждой проверил наличие enforcement рядом с фактической мутацией/чтением. Прошёлся по обоим ранее найденным дырам (#439 подписки, WASM `db_execute`) на СЕГОДНЯШНЕМ коде.

**Итог верхнего уровня.** Обе ранее CRITICAL/HIGH-дыры **закрыты полностью**. Живой data-path (wire + WASM + interactive-tx + подписки + репликация) сейчас **сквозно гейтится** через `authorize_access`. Обнаруженных дыр обхода (bypass/no-op/замокан) на живом пути — **нет**. Основные замечания — архитектурного/hardening-класса: (1) хрупкость дублирования маппинга `BatchOp → Action/ResourcePath` в трёх местах без единого реестра, (2) над-ограничительный coarse `is_superuser`-гейт на wire, из-за которого fine-grained admin-DAC фактически недостижим не-System actor'ом по проводу (не дыра, но «мёртвый» слой защиты + потенциальный источник будущей рассинхронизации), (3) мелкие TOCTOU-окна create/authorize (LOW). Детали ниже.

## Топ-находки

| # | Серьёзность | Где | Суть | Эскиз фикса |
|---|---|---|---|---|
| 1 | **LOW-MED (hardening)** | `crates\shamir-db\src\shamir_db\execute\db_execute.rs:52-70`, `db_tx.rs:139-169`, `shamir-engine\src\query\auth\session.rs:234-563`, `access.rs:577-584` | Маппинг `BatchOp → (Action, ResourcePath)` продублирован в 3+ местах (wire per-op loop, tx per-op loop, session-классификатор) с РАЗНЫМИ enum'ами Action и без единого источника истины; новый `BatchOp` может «забыть» гейт — компилятор не заставит | Единый декларативный реестр `fn op_authz(op) -> Option<(Action, ResourcePath)>` в query-types, из которого читают ОБА per-op loop; exhaustive `match` без wildcard (как `is_write`) |
| 2 | **LOW (asymmetry)** | `crates\shamir-server\src\db_handler\handler.rs:341-350` + `tx_handlers.rs:103-112` vs `admin_*.rs` | Coarse wire-гейт «любой `is_admin()` ⇒ требуется `is_superuser`» отбивает не-superuser'а ДО того, как отработает fine-grained admin-DAC; superuser маппится в `Actor::System` (bypass). Итог: весь admin-DAC (`Manage`/`Create` на контейнерах) на wire достижим ТОЛЬКО System'ом — не-System admin-путь fine-grained проверок остаётся «мёртвым» на проводе (жив лишь в WASM/tx-under-user) | Осознанно задокументировать статус, либо снять coarse-гейт для read-only introspection admin-ops и позволить fine-grained DAC решать (см. §Детали 2) |
| 3 | **LOW (TOCTOU)** | `crates\shamir-db\src\shamir_db\execute\admin_db_repo.rs:29-48`, `123-172` | В `handle_create_db`/`handle_create_repo` проверка существования идёт ПЕРЕД `authorize_access`, а фактический create — сразу после authorize; между authorize и create нет повторной атомарной проверки. Окно узкое (create под внутренним локом), эксплуатация не найдена | Держать authorize→exists-check→create под одним локом контейнера, либо повторить exists-guard внутри `create_db_as`/`add_repo_as` |
| 4 | **INFO (by design, подтверждено безопасным)** | `crates\shamir-server\src\replication\follower_loop.rs:322-323`, `crates\shamir-engine\src\tx\apply_replicated.rs` | Follower применяет реплицированные события через `repo_instance.apply_replicated(ev, …)` — физический WAL/overlay-apply БЕЗ `authorize_access`. Это НЕ дыра: авторизация происходит на leader'е (pull-side, role-gate + per-repo `Read`), follower — доверенный downstream, apply идёт от системного контекста и не парсит actor'а из чужого события | — (подтверждено корректным; см. §Детали 4) |

---

## Статус ранее найденных дыр (аудит 2026-07-06 §1a)

### CRIT #439 — подписка обходит per-table read-ACL → **ЗАКРЫТО ПОЛНОСТЬЮ**

Прошлое состояние: `subscribe_handler.rs` спавнил bridge без ACL; `bridge.rs` фильтровал лишь по имени/маске.

Сегодня: enforcement реально вставлен в `bridge_task` перед подпиской на changefeed:
- `crates\shamir-server\src\subscriptions\bridge.rs:144-170` — цикл по `sources`, для каждого строится `ResourcePath::Table{db,store,table}` и вызывается `db.authorize_access(&actor, &path, Action::Read)`; неавторизованные source'ы исключаются с `tracing::warn`, при полном denial — `return` до создания любого receiver'а (нет push).
- Начальный snapshot (`bridge.rs:339`) идёт через `db.execute_as(actor.clone(), …)` — то есть тоже под actor'ом с полным per-table гейтом, а не под System.
- `subscribe_handler.rs:16-28` честно документирует partial-reject семантику (клиент получает `sub_id` синхронно, но bridge отдаёт лишь авторизованное подмножество).

Остаточное (не security, а UX): «all-denied» не сюрфейсится синхронной ошибкой клиенту (comment в `subscribe_handler.rs:22-28` это фиксирует как будущий HIGH-task по UX). Read-ACL при этом НЕ обходится — данные не текут.

### HIGH/CRIT — WASM `db_execute` без actor-скоупа → **ЗАКРЫТО ПОЛНОСТЬЮ**

Прошлое состояние: `host_db.rs` звал `gateway.execute(&req_bytes)` без actor'а; `FnCtx.actor` никуда не прокидывался; `Security::Definer/Invoker` «NOT enforced».

Сегодня:
- `crates\shamir-db\src\shamir_db\shamir_db\db_gateway.rs:29-36` — `FacadeDbGateway` теперь НЕСЁТ поле `actor: Actor` (effective actor функции), и ВСЕ четыре метода (`get`/`insert`/`query`/`execute`) роутятся через `self.shamir.execute_as(self.actor.clone(), &self.db_name, &req)` (строки 145, 199, 266, 287) — то есть под полным per-table гейтом, НЕ под System.
- Effective actor вычисляется fail-closed: `access_control.rs:463-486` `effective_fn_actor` — `Definer` ⇒ owner функции, `Invoker`+setuid ⇒ owner, иначе caller; при not-found/ошибке возвращается caller (никогда System через `open()`-default). `meta.rs:44-48` `Security` теперь реально консультируется (не dead weight).
- Точка входа функции сама гейтится: `function_management.rs:591/630/679/733` — `authorize_access(caller, Function, Execute)` ПЕРЕД `effective_fn_actor` и построением gateway; gateway получает `actor` (`function_management.rs:689-703`, `743-757`).
- `host_db.rs:166-190` по-прежнему прокидывает сырой `BatchRequest` в `gateway.execute`, НО теперь это безопасно: `execute_as` внутри авторизует и `Database(Read)`, и КАЖДУЮ target-таблицу (`db_execute.rs:33-70`). Функция низкопривилегированного юзера, дёрнувшая `db_execute` с батчем на `secrets` или DDL, упрётся в `authorize_access` под её effective actor'ом.

Замечание: `db_get/db_insert/db_query` (`host_db.rs:16-158`) фиксируют `repo` из `HostState`, но допускают ЛЮБУЮ `table` — это ОК, потому что per-table `authorize_access` в `execute_as`/gateway всё равно отобьёт неавторизованную таблицу. Скоуп обеспечивается гейтом, а не самоограничением хоста (как и требовал прошлый фикс).

### MEDIUM — admin create/grant/chown/retention НЕ под HMAC-гейтом

Статус: **не изменилось** (вне скоупа этого прохода — это «did-you-mean-it» hardening, а не обход ACL). `check_destructive_hmacs` (`admin.rs:137-215`) по-прежнему покрывает только `Drop*`/`*Migration`. `CreateUser`/`GrantRole`/`Chmod`/`Chown`/`Chgrp`/`SetRetention`/`PurgeHistory` идут без HMAC-подтверждения. Остаётся как асимметрия защиты (украденный живой superuser-тикет). Рекомендую отдельный task на расширение HMAC-списка (как и в прошлом аудите).

---

## Детальный проход по классам операций

Матрица: для каждого класса указано, ГДЕ на живом пути стоит enforcement.

### Data-ops (Read/Insert/Update/Delete/Set на Table/Record) — ПОКРЫТО

- **Wire non-tx:** `db_execute.rs:52-70` — per-op loop строит `ResourcePath::Table` и зовёт `authorize_access(actor, path, action)` с маппингом `Read→Read, Insert→Create, Set/Update→Write, Delete→Delete`. `authorize_access` traverse'ит ancestors (`Execute` на db/store) + target. До этого — `Database(Read)` на строке 33.
- **Wire interactive-tx:** `db_tx.rs:139-169` — тот же per-op loop с ACL inline-кэшем (стек-локальный, не шарится между вызовами). `tx_begin_as`/`tx_commit_as` гейтят `Database` Read/Write (`db_tx.rs:61, 224`).
- **WASM:** через `FacadeDbGateway` → `execute_as` (см. выше).
- **Замечание про `query_runner.rs`:** внутри движка `run()` (строки 320/375/468/590/706) зовёт `authorize(&actor, &resource, …)` — это ТРАНСПАРЕНТНЫЙ trace-гейт из `access.rs:567` (всегда `Ok`), НЕ enforcement. Это не дыра: реальный enforcement стоит уровнем выше — в `execute_as`/`tx_execute_as` ДО входа в движок. Движковый `authorize` — только R2-трейс для observability. Стоит добавить doc-комментарий, что это НЕ точка enforcement, чтобы будущий разработчик не принял её за таковую и не убрал внешний гейт.

### DDL Create/Delete на Database/Store/Table/Index — ПОКРЫТО

- `admin_db_repo.rs`: CreateDb→`Root:Create` (43), DropDb→`Database:Delete` (77), CreateRepo→`Database:Create` (164), DropRepo→`Store:Delete` (247), RenameRepo→`Store:Write` (314), RenameDb→`Database:Write` (357).
- `admin_table_index.rs`: create/drop/rename table+index — `authorize_access` на строках 52/125/268/315/465/523 (Create/Delete/Write на Table/Store соответственно).
- Все идут ЧЕРЕЗ `execute_admin` → `ShamirAdminExecutor{actor}` (`admin_dispatch.rs:10-14`), actor прокинут из `execute_as`/`tx_execute_as`.
- TOCTOU: см. Топ-находка #3 (LOW).

### Manage (Chmod/Chown/Chgrp/Create*Group/Grant/Revoke/Role) — ПОКРЫТО

- `admin_access.rs`: chmod/chown/chgrp → `authorize_access(path, Manage)` (35/72/109) на КОНКРЕТНОМ ресурсе; group-ops → `Root:Manage` (143/176/222/261/298/341); AccessTree → `Root:Manage` (341).
- `admin_users_roles.rs`: CreateUser/DropUser → `authorize_user_lifecycle` (owner-delegation: `Root:Manage` ИЛИ `Database:Manage` для scope, `admin_dispatch.rs:141-172`); CreateRole/DropRole/Grant/Revoke → `Root:Manage` (235/322/396/606/700).
- **Важно про библиотечные функции групп:** `access_control.rs:201-287` (`create_group`/`add_group_member`/`remove_group_member`/`drop_group`/`rename_group`) сами НЕ делают Manage-проверки — они «тупые» исполнители, как и задокументировано в промте. Проверил ВСЕ пути вызова: единственные вызовы — из `admin_access.rs` handlers, каждый из которых предваряет вызов `authorize_access(Root, Manage)`. Отдельного offline-CLI, дёргающего эти функции в обход DDL-слоя, в кодовой базе **не найдено** (grep по `create_group`/`add_group_member` вне tests/docs даёт только `admin_access.rs` + определения). Прямых публичных вызовов `shamir.create_group(...)` из бинарей нет. Риск закрыт тем, что функции `pub` но вызываются лишь через гейтящий DDL.

### Execute на Function (Call/invoke) — ПОКРЫТО

- Батчевый `Call`: `query_runner.rs:214-222` → `ShamirFunctionInvoker::invoke_call` (`function_invoker.rs:19-53`) → `invoke_function_in_db_as` → `authorize_access(caller, Function, Execute)` (`function_management.rs:679`). `Call` НЕ в `is_admin()` (batch_op.rs:462-534), поэтому не отбивается coarse-гейтом — но Execute-ACL на функцию его гейтит. Внутри функции DB-доступ идёт под effective actor'ом (setuid/definer).
- Standalone `invoke_function_*_as` — все четыре варианта гейтят `Function:Execute` перед исполнением (591/630/679/733).

### List/Read-introspection (List/AccessTree/GetTableSchema/DescribeTable/ListValidators/InternerDump/ChangesSince/GetBufferConfig) — ПОКРЫТО (но недостижимо не-superuser'ом на wire, см. #2)

- `admin_list.rs`: List→`Root:List`/`Database:List` (31/41/59), function-list→`FunctionNamespace:List` (190/221/247), sensitive-list→`Root:Manage` (78/117).
- `admin_describe.rs:47`, `admin_schema.rs` (GetTableSchema/DescribeTable → Read на Table), `admin_interner.rs:51/130`, `admin_buffer.rs:31/87/136` — все под `authorize_access`.
- НО: все эти ops в `is_admin()==true`, значит на wire coarse-гейт `handler.rs:341` отбивает не-superuser'а РАНЬШЕ; superuser=System bypass'ит fine-grained. Итог: fine-grained read-DAC на этих ops жив только для WASM/tx-under-user. См. #2.

### Репликация — ПОКРЫТО (leader) + by-design (follower)

- **Leader / pull-side** (`repl_handler.rs`): role-gate `replicator`/superuser (46), затем per-repo `authorize_access(actor, store(db,repo), Read)` в `handle_hello` (88-95, репо без Read молча опускаются — нет existence-leak) и `handle_pull` (132, отказ `denied_repo`). Корректно.
- **Follower / apply-side** (`follower_loop.rs:322`): `apply_replicated` — физический apply без ACL. Подтверждено безопасным: авторизация на leader'е, follower доверенный, actor чужого события не парсится как доверенный. См. Топ-находка #4.

### Interactive-tx admin-ops — коррект, но с оговоркой (#2)

`tx_execute_as` (`db_tx.rs:113-199`) per-op-loop проверяет ТОЛЬКО ops с `table_ref()` (DML). Admin-ops (без table_ref) НЕ пред-проверяются здесь — они идут в `execute_in_open_tx` → `ShamirAdminExecutor` (actor прокинут, `db_tx.rs:180-184`), который авторизует внутри каждого handler'а. Плюс на wire `tx_handlers.rs:103-112` отбивает не-superuser'а на любом `is_admin()`. Так что admin в tx покрыт двумя слоями. Дыр нет.

---

## Детали по Топ-находкам

### #1 — Дублирование authz-маппинга (LOW-MED hardening)

Маппинг «какой `BatchOp` → какое `Action` + какой `ResourcePath`» физически продублирован:
- `db_execute.rs:54-60` (wire non-tx, `access::Action`),
- `db_tx.rs:141-147` (interactive-tx, `access::Action`) — БАЙТ-В-БАЙТ копия предыдущего,
- `session.rs:234-563` (`extract_action_resource`, но с ДРУГИМ enum'ом `query::auth::Action`, и это test-only scaffolding по комментарию `session.rs:26-34`),
- `access.rs:577-584` (`action_perm`: Action→Perm-бит).

Опасность: два per-op loop'а (`db_execute.rs` и `db_tx.rs`) — это РЕАЛЬНЫЕ enforcement-точки, и они идентичны, но поддерживаются раздельно вручную. Новый `BatchOp`-вариант, добавленный без table_ref (напр. новая read-introspection над таблицей, которую забыли завести в `table_ref()`), не получит per-op-проверку — а `matches!` в `is_admin` тоже можно забыть обновить (в отличие от exhaustive `is_write`). Фикс: единый `fn required_access(&self, db: &str) -> Option<(Action, ResourcePath)>` на `BatchOp` (exhaustive match, no wildcard), из которого читают ОБА loop'а; тогда «забыть гейт» станет ошибкой компиляции.

### #2 — Coarse `is_superuser`-гейт делает fine-grained admin-DAC «мёртвым» на wire (LOW asymmetry)

`handler.rs:341-350` и `tx_handlers.rs:103-112`: если `!is_superuser` и в батче есть любой `is_admin()`-op → `permission_denied` ДО движка. `session_actor` (`handler.rs:116-122`): superuser → `Actor::System` (bypass в `authorize_access:358`), обычный юзер → `Actor::User(id)`.

Следствие: по проводу admin-op исполняет ЛИБО System (bypass всех fine-grained проверок), ЛИБО никто (не-superuser отбит coarse-гейтом). Значит вся fine-grained admin-DAC-логика (`Manage` на конкретном ресурсе, `Create` на родителе, owner-delegation `authorize_user_lifecycle`) на wire-пути **никогда не исполняется под `Actor::User`** — она достижима лишь через WASM `db_execute` (функция как non-System effective actor) и interactive-tx-under-user. Это НЕ дыра (fail-closed: скорее над-ограничение), но: (а) большой слой защиты фактически не тестируется живым wire-трафиком; (б) при будущем ослаблении coarse-гейта (чтобы дать owner'у БД управлять своими объектами без глобального superuser) вся тяжесть ляжет на fine-grained DAC, который сейчас проверяется в основном юнит-тестами. Рекомендация: либо явно задокументировать, что fine-grained admin-DAC — это защита для WASM/tx-путей, а wire admin = «superuser-only by design»; либо снять coarse-гейт хотя бы для read-only introspection (`List`/`AccessTree`/`DescribeTable`/`GetTableSchema`) и позволить DAC-у решать per-resource (тогда owner БД сможет `DESCRIBE` свою таблицу без superuser).

### #3 — TOCTOU create/authorize (LOW)

`handle_create_db` (`admin_db_repo.rs:29-48`): порядок = `has_db()` → (если есть и `if_not_exists`) return → `authorize_access(Root, Create)` → `create_db_as`. `handle_create_repo` (144-172): аналогично `has_repo()` → `authorize_access(Database, Create)` → `add_repo_as`. Между `authorize_access` и фактическим create нет повторной атомарной exists-проверки; два конкурентных create одного имени могут оба пройти authorize, затем оба вызвать `create_db_as`/`add_repo_as`. Практический риск минимален (create под внутренним локом инстанса, второй перезапишет/зафейлится детерминированно, ACL при этом соблюдён — оба actor'а прошли `Create`). Не security-обход прав, а потенциальный race на идемпотентности. Фикс: провести authorize→exists→create под одним локом контейнера или повторить exists-guard внутри `*_as`.

### #4 — Follower apply без ACL (INFO, by design — подтверждено безопасным)

`follower_loop.rs:322-323` применяет `ChangelogEvent` через `repo_instance.apply_replicated(ev, bookmark)` — это физический overlay/WAL-apply на уровне storage, без `authorize_access`. Это корректно и НЕ является каналом обхода прав, потому что: (1) авторизация выполнена на LEADER'е при pull (role-gate + per-repo `Read`, `repl_handler.rs`); (2) follower — доверенный downstream в доверенном кластере (транспорт TLS+SCRAM, leader аутентифицирован); (3) event НЕ несёт «actor'а», которого follower принял бы как доверенного — apply идёт от системного storage-контекста, никакой per-op ACL исходного actor'а тут не реконструируется и не «повышается». Единственная предпосылка безопасности — доверенность самого leader-пира (обеспечивается транспортным auth + epoch-fencing `§5.2`). Замечаний нет; зафиксировано для полноты.

---

## Рекомендации

1. **Единый декларативный authz-реестр (закрывает #1).** Ввести на `BatchOp` метод `required_access(&self, db_name: &str) -> Option<(Action, ResourcePath)>` с **exhaustive match без wildcard** (как `is_write`), возвращающий пару для data-ops и `None` для чисто-admin-ops (которые авторизуются внутри своих handler'ов). Оба per-op loop'а (`db_execute.rs`, `db_tx.rs`) читают из него. Тогда новый `BatchOp` без явной классификации — ошибка компиляции, а не молчаливый пропуск гейта.

2. **Интеграционный тест-матрица ACCESS_HIERARCHY.md ↔ реально защищённые ops.** Автотест, который для каждой пары (объект × операция) из `ACCESS_HIERARCHY.md` прогоняет реальный `execute_as`/`tx_execute_as`/WASM-`db_execute` под `Actor::User` без прав и утверждает `access_denied`, а под owner'ом/System — `Ok`. Сейчас часть покрытия — юнит-тесты на `authorize_access` напрямую (`enforcement_tests.rs`, `access_ddl.rs`); нужна сквозная матрица именно через входные точки, чтобы поймать «забыли гейт на новом op».

3. **Doc-guard на транспарентный `authorize` в движке.** В `query_runner.rs` рядом с вызовами `authorize(&self.actor, …)` (строки 320/375/468/590/706) добавить явный комментарий «это R2-trace, НЕ enforcement; реальный гейт — `execute_as`/`tx_execute_as` уровнем выше», чтобы будущий рефактор не принял его за enforcement и не снёс внешний гейт. Опционально — переименовать движковый `authorize` в `trace_access`, чтобы имя не путали с `authorize_access`.

4. **Явно определить статус wire-admin DAC (по #2).** Либо задокументировать в `ACCESS_HIERARCHY.md`, что wire-admin — «superuser/System-only by design», а fine-grained admin-DAC защищает только WASM/tx-under-user; либо ослабить coarse-гейт для read-only introspection и передать решение fine-grained DAC-у (owner БД читает свою схему без глобального superuser). Сейчас это «серая зона»: код fine-grained есть, но на главном пути не задействован.

5. **Расширить HMAC-«did-you-mean-it» на privilege-granting/create/retention (переносится из аудита 2026-07-06 §1a MEDIUM).** `CreateUser`/`GrantRole`/`Chmod`/`Chown`/`Chgrp`/`SetRetention`/`PurgeHistory` — добавить в `check_destructive_hmacs`, чтобы украденный живой superuser-тикет не мог тихо раздать права/сменить владельца без per-op HMAC-подтверждения, требуемого сейчас лишь для `Drop*`.

6. **Закрыть TOCTOU create (#3).** Провести authorize→exists→create под одним локом контейнера в `handle_create_db`/`handle_create_repo`, либо повторить exists-guard внутри `create_db_as`/`add_repo_as`.

---

### Что подтверждено безопасным (не трогать)

Обе ранее найденные дыры закрыты (подписки #439, WASM `db_execute`). Живой data-path сквозно гейтится `authorize_access` на всех входных точках: wire non-tx (`execute_as`), interactive-tx (`tx_*_as`), WASM (`FacadeDbGateway.actor` → `execute_as`), батчевый `Call` (`Function:Execute` + effective-actor), подписки (per-source `Table:Read` в `bridge_task` + snapshot через `execute_as`), репликация leader (role + per-repo `Read`). `effective_fn_actor` fail-closed (никогда System через `open()`-default). Библиотечные group-функции достижимы только через гейтящий `admin_access.rs`; offline-CLI в обход DDL не найдено. Follower-apply без ACL — корректно by design (авторизация на leader'е, доверенный downstream).
