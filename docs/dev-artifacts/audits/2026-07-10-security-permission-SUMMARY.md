בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Сводный security-аудит системы прав «Shomer» S.H.A.M.I.R. — единый саммари

_Синтез 5 параллельных отчётов @fxx (max effort), 2026-07-10. Защитный аудит собственной кодовой базы (авторизованный, для укрепления). Ниже объединены, дедуплицированы и ранжированы все находки пяти проходов по POSIX-подобной модели прав. Ничего нового не добавлено — только синтез уже написанного._

**Исходные отчёты:**
1. [`2026-07-10-security-permission-model-core.md`](./2026-07-10-security-permission-model-core.md) — core-логика (`access.rs`/`access_control.rs`): коллизии `principal_id`, `permits`/`class_of`, fail-open дефолты, `effective_fn_actor`.
2. [`2026-07-10-security-permission-gate-coverage.md`](./2026-07-10-security-permission-gate-coverage.md) — покрытие enforcement-гейтом всех execute-путей (query engine, admin DDL, WASM, подписки, репликация) + статус старых дыр.
3. [`2026-07-10-security-permission-admin-ddl.md`](./2026-07-10-security-permission-admin-ddl.md) — admin/DDL-поверхность: chmod/chown/chgrp, группы, роли, HMAC-гейт.
4. [`2026-07-10-security-permission-identity-session.md`](./2026-07-10-security-permission-identity-session.md) — сессии/тикеты и маппинг сессии на `Actor`.
5. [`2026-07-10-security-permission-wasm-functions.md`](./2026-07-10-security-permission-wasm-functions.md) — WASM-функции, setuid/SECURITY DEFINER, компиляция на хосте.

---

## Executive Summary

POSIX-ядро модели прав (`permits`/`class_of`/`authorize_access`) реализовано **корректно**: `Manage` требует строгого равенства owner, union-семантики нет, traversal с обязательным `Execute` на каждом ancestor работает, а обе ранее известные CRITICAL/HIGH-дыры (подписки #439 и WASM `db_execute` без actor-скоупа) **закрыты полностью** — живой data-path сейчас сквозно гейтится `authorize_access`. Это здоровый фундамент. Однако вокруг ядра остаётся кластер системных дефектов, объединённых одной темой: **security-идентичность выведена из некриптографического fxhash изменяемого username, а не из устойчивого `user_id`**, что открывает наследование чужого владения при пересоздании юзера и подбор коллизий id (включая потенциальное попадание в id 0 = System). Самая тяжёлая остаточная проблема — **компиляция недоверенного Rust-исходника прямо на хосте БД** (`cargo build` с нативными build-scripts/proc-macro без seccomp/rlimit) — это CRITICAL по остаточному риску, лишь частично смягчённый форбид-сканом и scrub-env. Третья критичная зона — **параллельная ролевая ось привилегий**: роль-строка `"superuser"` — необъявленный тумблер в `Actor::System` в обход всей owner/group/mode-модели, при этом два стора юзеров/ролей рассинхронизированы, а выдача этой роли не требует HMAC-подтверждения (в отличие от куда менее опасного `DropUser`). Дополнительно найдены fail-open дефолт при ошибке чтения каталога (любой сбой чтения меты → полный доступ вместо отказа) и эскалация до System через функцию с потерянным полем `owner` + Definer/setuid. Остальные находки — MEDIUM/LOW hardening: несведённый HMAC, непроверенные client-supplied id в chown/chgrp, отсутствие self-service ревокации тикетов (changePassword не подключён к серверу), глобальный (а не per-function) egress-allowlist. **Вывод: модель НЕ готова к продакшену в текущем виде** — ядро крепкое, но identity-слой, WASM-компиляция на хосте и ролевая ось требуют исправления до вывода в prod. После закрытия CRITICAL + шести HIGH модель можно считать production-ready.

---

## Сводная таблица находок (по убыванию серьёзности)

| # | Серьёзность | Область | Где (файл:строка) | Суть | Источник-отчёт |
|---|---|---|---|---|---|
| 1 | **CRITICAL** (остаточная) | WASM-функции | `shamir-wasm-host/src/compile.rs:399-586`, `:495`, `:537` | Недоверенный Rust-исходник компилируется на хосте БД: `cargo build` исполняет build-scripts/proc-macro как нативные процессы без seccomp/rlimit/FS-/сетевой-изоляции. Форбид-скан `include*!`/`env!` лексический и сам признан обходимым через proc-macro-зависимость | #5 (WASM), нах. 1 |
| 2 | **HIGH** | Identity / core-модель | `shamir-server/src/server/session.rs:246-252`, `shamir-server/src/db_handler/handler.rs:116-122`, `shamir-types/src/access.rs:33`, `admin_users_roles.rs:16-82` | Security-`Actor` привязан к `principal_id = fxhash(username)&i63` — некриптографический хэш **изменяемого** имени, а не к устойчивому `user_id[16]`. Следствия: (а) пересоздание юзера с тем же именем наследует ВСЁ владение старого; (б) коллизии подбираемы (fxhash обратим) → захват чужого владения/групп, потенциально id 0 = System; (в) create-user без проверки уникальности/коллизии — слепой upsert перетирает существующего | #1 (F2), #4 (нах. 1+2), #3 (нах. 4) |
| 3 | **HIGH** (fail-open) | Core-модель | `access_control.rs:44-99` (все `rec.ok().flatten()…unwrap_or_default()`) | Ошибка чтения меты каталога (I/O, corruption, десериализация) неотличима от «не найдено» — обе дают `ResourceMeta::open()` (System, 0o777). Любой транзиентный сбой чтения каталога → полностью открытый доступ вместо отказа (fail-open вместо fail-closed), включая traversal-Execute на ancestors | #1 (F1) |
| 4 | **HIGH** (комбо) | Core-модель / WASM | `access_control.rs:463-486` (`effective_fn_actor`) + `access.rs:236-249` | Функция с успешно загруженной записью, но БЕЗ поля `owner` (`from_record` → `unwrap_or(Actor::System)`) и с `Definer`/setuid → эскалация вызывающего до **System** (полный admin-bypass). Fail-closed guard на `:467` ловит только «записи нет целиком», не «поле owner отсутствует» | #1 (F3) |
| 5 | **HIGH** | Admin / роли | `shamir-connect/src/server/session.rs:36`, `shamir-server/src/db_handler/handler.rs:117-121` | Роль-строка `"superuser"` → `is_superuser` → `Actor::System` = полный bypass POSIX-гейта. Вторая, независимая от POSIX ось привилегий, склеенная лишь строковым сравнением имени. Имя `"superuser"`/`"replicator"` нигде не зарезервировано/валидировано; grant/create с ним HMAC не требует | #3 (нах. 1), #1 (F4) |
| 6 | **HIGH** | Admin / роли | `admin_users_roles.rs:16-82`, `shamir-server/src/db_handler/admin.rs:37-125`, `shamir-server/src/user_directory.rs` | Два несвязанных стора юзеров/ролей: `shamir-db` `users`-таблица (куда пишут `CreateUser`/`GrantRole`) и `FjallUserDirectory` (откуда `lookup_roles` берёт роли при логине). Wire-логин НЕ читает `shamir-db` `users.roles` → рассинхрон прав, «фантомные» гранты, ложная видимость управления | #3 (нах. 2) |
| 7 | **MEDIUM→HIGH** | Admin / HMAC | `shamir-server/src/db_handler/admin.rs:137-215` (`check_destructive_hmacs`), `handler.rs:341,378` | HMAC «did-you-mean-it» покрывает ТОЛЬКО `Drop*`/migration. `CreateUser`, `GrantRole`/`RevokeRole` (выдача `superuser`!), `Chmod`/`Chown`/`Chgrp`, `SetRetention`/`PurgeHistory`, group-CRUD идут БЕЗ HMAC. Украденный живой superuser-тикет → grant/chown/purge без подтверждения. Прошлая находка не закрыта | #3 (нах. 3), #2 (статус MEDIUM) |
| 8 | **MEDIUM** | Admin / Identity | `admin_access.rs:50-85`, `:76,113,270,307` (chown/chgrp/addGroupMember) | `op.owner`/`op.group`/`op.user` — сырой клиентский `u64`, пишется в каталог/членство БЕЗ проверки существования принципала. Owner может «подарить» ресурс System (`0`) или потерять его безвозвратно (осиротить); «висячий» owner-id потом унаследует будущий юзер с подходящим именем-хэшем | #3 (нах. 5), #4 (нах. 5) |
| 9 | **MEDIUM** | Identity / сессии | `shamir-connect/src/server/changepw.rs` (весь модуль) | Флоу `changePassword` (SCRAM-verify + kill сессий + bump `tickets_invalid_before_ns`) реализован в `shamir-connect`, но НЕ подключён к request-loop сервера (`grep change_password` по `shamir-server/src` пуст). Нет self-service пути отзыва скомпрометированных тикетов — только admin `kickSession`/`updateUser` | #4 (нах. 3) |
| 10 | **MEDIUM** | Identity / тикеты | `handler.rs:117`, `shamir-connect/src/server/resume.rs:381`, `ticket.rs:71-74` | При resume `roles` (в т.ч. `superuser`) берутся из СНАПШОТА в тикете и не переверифицируются из авторитетного user-record. Целостность держится на инварианте «любое изменение прав всегда bumpает `tickets_invalid_before_ns`» — если хоть один путь не bumpнет, устаревший тикет продолжит давать admin-bypass | #4 (нах. 4) |
| 11 | **MEDIUM** | WASM-функции | `shamir-db/src/shamir_db/shamir_db/core.rs:605-609` (`build_net_gateway`), `net_allowlist`, `context.rs:369` | Egress-allowlist глобальный по БД, а не per-function капабилити-бит, как декларирует `ACCESS_HIERARCHY.md:73-74`. Любая функция любого owner получает один egress-скоуп. TOCTOU-отзыва нет (гейт per-request), но per-function ограничения тоже нет | #5 (нах. 2) |
| 12 | **MEDIUM** | WASM-функции | `shamir-wasm-host/.../context.rs:204-210` (`seed_env`), `global_set` в `env.*` | Гость через `global_set("env.X", …)` может ПЕРЕТЕРЕТЬ засеянное ОС-значение `env.*` в общем `GlobalVars` — целостность секрета/помеха другим функциям в том же процессе. Не эскалация, но запись в `env.*`-неймспейс не гейтится | #5 (§2e / рек. 4) |
| 13 | **LOW-MEDIUM** | Core / Admin | `access_control.rs:93-95` (Root/User/Group → `open()`), `admin_list.rs:31` | `Root`/`User`/`Group` жёстко `open()` (0o777): (а) имена всех БД перечислимы любым аутентифицированным юзером (`List`→Read на Root); (б) сами объекты-принципалы (User/Group) неуправляемы — chmod/chown над ними невозможен, метаданные жёстко открыты | #1 (F5), #3 (нах. 6) |
| 14 | **LOW-MED** (hardening) | Gate-coverage | `db_execute.rs:52-70`, `db_tx.rs:139-169`, `query/auth/session.rs:234-563`, `access.rs:577-584` | Маппинг `BatchOp → (Action, ResourcePath)` продублирован в 3+ местах (два РЕАЛЬНЫХ enforcement-loop'а — байт-в-байт копии) с разными enum'ами Action, без единого источника. Новый `BatchOp` может «забыть» гейт — компилятор не заставит | #2 (нах. 1) |
| 15 | **LOW** (асимметрия) | Gate-coverage | `shamir-server/src/db_handler/handler.rs:341-350`, `tx_handlers.rs:103-112` | Coarse-гейт «любой `is_admin()` ⇒ нужен `is_superuser`» отбивает не-superuser'а ДО fine-grained admin-DAC; superuser=`Actor::System` (bypass). Итог: весь fine-grained admin-DAC на wire достижим ТОЛЬКО System — не-System admin-путь «мёртв» на проводе (жив лишь в WASM/tx-under-user). Не дыра (над-ограничение), но непротестированный слой | #2 (нах. 2) |
| 16 | **LOW** (TOCTOU) | Gate-coverage | `admin_db_repo.rs:29-48`, `:123-172` | В `handle_create_db`/`handle_create_repo` exists-check → authorize → create без повторной атомарной проверки между authorize и create. Окно узкое (create под внутренним локом), ACL соблюдён; race на идемпотентности, не обход прав | #2 (нах. 3) |
| 17 | **LOW / инфо** | WASM-функции | `shamir-query-types/src/admin/types/function_ops.rs:17-25` (`CreateFunctionOp`), `admin_function.rs:44-66` | Проводной `CreateFunction` не несёт `security`/`secret_grants`/`visibility`/setuid → по проводу функция всегда `Invoker`+пустые гранты+`owned_enforced(caller)`. Безопасно (нельзя выдать себе Definer), но `Definer`/`secret_grants` недостижимы через wire — функциональный разрыв, не дыра | #5 (нах. 3) |
| 18 | **LOW** (хрупкость) | Admin / группы | `access_control.rs:201-287` (`create_group`/`add_group_member`/…) | Голые group-CRUD и user-lifecycle методы сами Manage-проверок НЕ делают — безопасность держится ИСКЛЮЧИТЕЛЬНО на дисциплине диспетчера (каждый handler предваряет `authorize_access(Root, Manage)`). Новый вызыватель, забывший гейт, откроет самоэскалацию. Offline-обхода сейчас не найдено | #3 (§2), #2 (§Manage) |
| 19 | **LOW** (backdoor-риск) | Admin / bootstrap | `db_management.rs:12,72,306`, `table_management.rs:29,148`, `system_store.rs:145,170` | System-обёртки (`create_db`/`add_repo`/`rename_table` без `_as`) хардкодят `Actor::System`. Удалённо-достижимого пути к ним не найдено (wire всегда `execute_as(real_actor)`), но безопасность держится на дисциплине «wire → только real_actor»; будущий wire-хендлер, дёрнувший System-обёртку, станет бэкдором | #3 (§6) |

**INFO / by-design (подтверждено безопасным — НЕ дыры, зафиксировано для полноты):**
- Follower применяет реплицированные события `apply_replicated` без `authorize_access` — авторизация на leader'е (role-gate + per-repo `Read`), follower доверенный, actor чужого события не парсится (#2, нах. 4).
- Цепочечный setuid в WASM (`db_execute`→`Call`→повторный `effective_fn_actor`) — каждый шаг проходит `Execute`-гейт под текущим actor, эскалация без прав невозможна (#5, нах. 4).

---

## Статистика

**Всего находок (без INFO/by-design):** 19.

По серьёзности:
- **CRITICAL:** 1 (#1 — WASM-компиляция на хосте).
- **HIGH:** 5 (#2 identity-модель, #3 fail-open, #4 эскалация через missing-owner, #5 superuser→System, #6 рассинхрон сторов).
- **MEDIUM→HIGH:** 1 (#7 HMAC-асимметрия).
- **MEDIUM:** 5 (#8 непроверенный chown-id, #9 changePassword не подключён, #10 доверие ролям тикета, #11 глобальный egress, #12 env.* overwrite).
- **LOW-MEDIUM / LOW:** 7 (#13 open-мета Root/User/Group, #14 дублирование authz, #15 coarse-гейт, #16 TOCTOU, #17 wire Definer-разрыв, #18 дисциплина group-гейта, #19 System-обёртки).

По областям (наиболее проблемные → чистые):
- **Admin/DDL/роли — 6 находок** (#5,6,7,8,18,19): самая проблемная зона — параллельная ролевая ось, рассинхрон сторов, несведённый HMAC, непроверенные id.
- **WASM-функции — 4 находки** (#1 CRIT, #11, #12, #17): единственный CRITICAL живёт здесь (компиляция на хосте).
- **Identity/сессии — 3 находки** (#2 HIGH, #9, #10) + вклад в #2/#8.
- **Core-модель — 3 находки** (#3 HIGH, #4 HIGH, #13): два HIGH — fail-open и эскалация через missing-owner.
- **Gate-coverage — 3 находки** (#14,15,16): все LOW, только hardening — bypass-дыр на живом пути НЕ найдено.

**Чистые области (агент дыр не нашёл):**
- **POSIX-ядро `permits`/`class_of`/`action_perm`/`ancestors`** — корректная first-match owner→group→other семантика (не union), `Manage`=строгое owner-равенство, `parent()`/`ancestors()` не рвут цепочку, касты `u64→i64` безопасны (#1: пункты 2,3,6,7 чисты).
- **Enforcement-покрытие живого data-path** — wire non-tx, interactive-tx, WASM (`FacadeDbGateway.actor`→`execute_as`), батчевый `Call`, подписки (per-source `Table:Read`), репликация leader — всё сквозно гейтится; обе старые дыры (#439, WASM `db_execute`) закрыты; bypass не найден (#2).
- **Самоэскалация через группы** — прямого пути «любой юзер добавляет себя в привилегированную группу» НЕТ (group-CRUD гейтится `Manage(Root)`); chmod/chown/chgrp все под `Manage` (нет обхода через `Write`); `access_tree` wire-гейтится; `DropUser` скоуп-гейтится корректно (#3).
- **Криптографическая целостность тикетов и ревокация** — §7.5-ревокация на устойчивый `user_id`; тикет-actor binding под AES-256-GCM (клиент не подменит `user_id`/`username`/`roles`); resume не доверяет клиентскому username; PRECIS-нормализация имён полная (#4).
- **WASM effective-actor** — `effective_fn_actor` fail-closed (not-found→caller, не System); owner=caller при создании; setuid/Definer owner-only; builtin без катал. записи не эскалирует; net/env перепроверяются per-invocation (#5).

---

## Приоритизированный список рекомендаций

### Приоритет 1 — чинить в первую очередь (CRITICAL + HIGH, до продакшена)

1. **Убрать компиляцию недоверенного Rust с хоста БД** [#1]. Принимать только валидированный `.wasm` (путь `FunctionSource::Wasm` уже есть); если исходник обязателен — вынести `cargo build` в изолированный воркер (контейнер/gVisor или seccomp+rlimit+cgroup, read-only FS, запрет сети, лимит CPU/mem/времени, запрет path-deps proc-macro). Форбид-скан оставить как defence-in-depth. _(отчёт 5, рек. 1)_
2. **Отвязать security-identity от изменяемого/угадываемого атрибута** [#2]. Owner ресурса и enforcement-`Actor` должны нести устойчивый id (тот же `user_id`, что использует §7.5), а не `fxhash(username)`. `Session` хранит готовый numeric principal-id из user-record при логине, а не пересчитывает хэш имени на каждый запрос. `DropUser` должен явно осиротить/передать владение (owner→System либо запрет drop при наличии owned-ресурсов). Закрывает наследование при пересоздании, подбор коллизий и превентивно — будущий `RenameUser`. _(отчёты 1 рек.2+5, 4 рек.1+2+3)_
3. **Fail-closed вместо fail-open в `resource_meta`** [#3]. Разделить `Err` (deny+log) и `Ok(None)`; сменить сигнатуру на `DbResult<ResourceMeta>`; `open()`-fallback только для документированных implicit-путей. `authorize_access` при ошибке резолва — отказывать. _(отчёт 1, рек. 1)_
4. **`effective_fn_actor`: не эскалировать до System** [#4]. Если `res_meta.owner == Actor::System`, definer/setuid не должны давать System-контекст непривилегированному вызывающему; missing-owner-поле трактовать fail-closed. _(отчёт 1, рек. 3)_
5. **Зарезервировать привилегированные имена ролей** (`"superuser"`, `"replicator"`) [#5]. Grant/create с ними — только через явный отдельный привилегированный путь + HMAC. Согласовать ролевую ось и POSIX `Manage` в единую модель (сейчас `superuser`-роль — необъявленный тумблер в `Actor::System`). _(отчёты 3 рек.1, 1 рек.4)_
6. **Свести два стора юзеров/ролей к одному авторитетному** (`shamir-db` `users` vs `FjallUserDirectory`) [#6], либо явно задокументировать разделение и убрать ложную видимость управления живыми правами через `shamir-db` `GrantRole`. _(отчёт 3, рек. 4)_

### Приоритет 2 — важные доработки (MEDIUM→HIGH / MEDIUM)

7. **Расширить HMAC «did-you-mean-it»** [#7] на `GrantRole`/`RevokeRole`/`CreateUser`/`Chmod`/`Chown`/`Chgrp`/`SetRetention`/`PurgeHistory`/group-mutating ops — устранить асимметрию с `Drop*`. _(отчёты 3 рек.2, 2 рек.5)_
8. **Единая проверка id-коллизии и уникальности на create-user** (и симметрично create-group) [часть #2]: exists-guard по имени + обратный индекс `principal_id→name`, отвергающий коллизии; отказ от слепого upsert `SetOp`; рассмотреть каскад/запрет воскрешения owner-id после `DropUser`. _(отчёты 3 рек.3, 1 рек.2)_
9. **Валидировать client-supplied id в chown/chgrp/addGroupMember** против directory (существование principal/группы) до записи; запрет chown на `OWNER_SYSTEM` для не-System actor [#8]. _(отчёты 3 рек.5, 4 рек.7, 1 рек.6)_
10. **Ввести per-function `net_grants` капабилити** (симметрично `secret_grants`), пересекать с хостовым allowlist в `build_net_gateway(fn_name)` [#11] — привести к `ACCESS_HIERARCHY.md:73-74`. _(отчёт 5, рек. 2)_
11. **Гейтить `global_set` в `env.*`-неймспейс** из гостя, чтобы функция не перетирала засеянные ОС-секреты [#12]. _(отчёт 5, рек. 4)_
12. **Подключить changePassword к живому серверу** (или явно задокументировать admin-only ревокацию), с гарантированным `bump_tickets_invalid`(fsync)+kill сессий [#9]. _(отчёт 4, рек. 5)_
13. **При resume перечитывать роли/`superuser` из авторитетного directory по `user_id`** [#10], а не доверять снапшоту в тикете; либо формально доказать инвариант «любой путь изменения прав всегда bumpает `tickets_invalid_before_ns`». _(отчёт 4, рек. 6)_

### Приоритет 3 — архитектурные улучшения / hardening на будущее (LOW)

14. **Единый декларативный authz-реестр** [#14]: `BatchOp::required_access(&self, db) -> Option<(Action, ResourcePath)>` с exhaustive match без wildcard, читаемый ОБОИМИ per-op loop'ами — «забыть гейт» станет ошибкой компиляции. _(отчёт 2, рек. 1)_
15. **Интеграционная тест-матрица** `ACCESS_HIERARCHY.md ↔ реально защищённые ops` через входные точки (`execute_as`/`tx_execute_as`/WASM `db_execute`) под `Actor::User` без прав. _(отчёт 2, рек. 2)_
16. **Doc-guard / переименование** транспарентного `authorize` в движке (`query_runner.rs`) в `trace_access` — чтобы будущий рефактор не принял R2-trace за enforcement. _(отчёт 2, рек. 3)_
17. **Явно определить статус wire-admin DAC** [#15]: либо задокументировать «superuser/System-only by design», либо ослабить coarse-гейт для read-only introspection (`List`/`DescribeTable`/`GetTableSchema`), передав решение fine-grained DAC. _(отчёты 2 рек.4, 1 F4)_
18. **Единая модель суперюзера в core** — явный concept superuser (`Actor::Super`/флаг) либо документированный однозначный маппинг роли на входе в gate вместо двух параллельных истин. _(отчёт 1, рек. 4)_
19. **Инкапсулировать `Manage(Root)`-гейт внутрь group-CRUD/user-lifecycle** [#18] (или typestate «authorized»), чтобы новый вызыватель не обошёл проверку. _(отчёты 3 рек.6, 2 §Manage)_
20. **Пометить System-обёртки** (`create_db`/`add_repo`/`rename_table` без `_as`) как `pub(crate)`/`#[doc(hidden)]` или добавить контекст-guard [#19] — backdoor-предотвращение. _(отчёт 3, рек. 7)_
21. **Смоделировать/задокументировать права на объекты-принципалы** (`User`/`Group`) [#13]: сейчас `resource_meta` для них всегда `open()`, `ResourceRef` их не адресует. Опционально — Root-мета из каталога с enforced-дефолтом, если перечислимость всех БД нежелательна. _(отчёты 3 рек.8, 1 рек.7)_
22. **Закрыть TOCTOU create** [#16]: провести authorize→exists→create под одним локом контейнера, либо повторить exists-guard внутри `create_db_as`/`add_repo_as`. _(отчёт 2, рек. 6)_
23. **Крипто-хэш / выделенный счётчик id для principal id** (стратегически) — замена некриптографического fxhash устраняет класс «управляемых коллизий» и риск конструктивного попадания в id 0. Убрать дублирование логики `principal_id` (`access.rs:33-35` vs `session.rs:246-252`) — единый источник истины. _(отчёт 1, рек. 5+8)_
24. **Range-guard на вводе числовых gid/owner id** из клиента (превентивно, до каста `as i64`). _(отчёт 1, рек. 6)_
25. **`RenameUser` (если появится)** обязан либо перенести владение на новый id, либо инвалидировать все сессии/тикеты юзера. _(отчёт 4, рек. 4)_
26. **Явная Unicode-normalization на пути CreateUser** (прогонять `NormalizedUsername::from_raw` не только на логине) + опционально confusables-скрин поверх PRECIS. _(отчёт 4, рек. 8)_
27. **Regression-тест «builtin никогда не эскалирует»** и сохранение инварианта per-invocation ре-валидации net/env (не кэшировать `FnCtx`/gateway между вызовами разных эффективных actor). _(отчёт 5, рек. 5+6)_
28. **Явно решить судьбу проводного `Definer`/`secret_grants`/setuid** [#17]: либо добавить поля в `CreateFunctionOp` с owner+Manage-гейтом, либо задокументировать, что они внутрипроцессные. _(отчёт 5, рек. 3)_

---

## Заключение

Модель прав «Shomer» имеет **корректное и хорошо протестированное POSIX-ядро** и **сквозное enforcement-покрытие живого data-path** — обе исторические CRITICAL/HIGH-дыры закрыты, а обширный проход по gate-coverage не выявил ни одного обхода на живом пути. Это сильный, зрелый фундамент.

Однако модель **не готова к продакшену в текущем виде**. Блокирующими являются один CRITICAL (компиляция недоверенного Rust на хосте БД без изоляции) и шесть HIGH, среди которых системообразующие: security-идентичность выведена из некриптографического хэша изменяемого имени (наследование владения при пересоздании + подбираемые коллизии, вплоть до id 0 = System), fail-open дефолт при ошибке чтения каталога, эскалация до System через функцию с потерянным полем owner, и параллельная незарезервированная ролевая ось `superuser`→`Actor::System` при рассинхроне двух сторов юзеров.

Рекомендуемый путь к prod-готовности: закрыть Приоритет 1 (CRITICAL + 5 HIGH + рассинхрон сторов) полностью, затем Приоритет 2 (несведённый HMAC, непроверенные id, отсутствие self-service ревокации, per-function egress). Приоритет 3 — архитектурное укрепление, которое можно вести параллельно и после вывода в prod. После закрытия Приоритетов 1-2 модель прав можно считать production-ready.
