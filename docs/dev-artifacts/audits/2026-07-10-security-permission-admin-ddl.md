בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью системы прав: admin/DDL-поверхность S.H.A.M.I.R. (защитный аудит)

_Агент: @fxx (max effort), 2026-07-10. Авторизованный защитный аудит собственной кодовой базы. Фокус — admin/DDL-поверхность POSIX-подобной системы прав «Shomer» (chmod/chown/chgrp, create/drop user, group CRUD, grant/revoke role) и её смежная защита (HMAC «did-you-mean-it»). Смежные векторы (subscription-ACL, WASM `db_execute`, resumption-ticket binding) покрыты аудитом 2026-07-06 и здесь не дублируются._

**Метод.** Прошёл цепочку авторизации admin-операции по коду: wire-фрейм → `ShamirDbHandler::handle` (admin-гейт `is_superuser`, `handler.rs:341`) → `session_actor` (`handler.rs:116`: `is_superuser → Actor::System`) → `execute_as(actor, …)` → `ShamirAdminExecutor::execute_admin` (`admin_dispatch.rs:18`) → per-op хендлер → `authorize_access(actor, path, Action::Manage)` (`access_control.rs:348`) → `permits` (`access.rs:612`). Проверил все восемь пунктов брифа. Разобрал связку «ролевая система ↔ POSIX System-bypass», проверил проверки уникальности id при create-user/create-group, проследил chmod/chown/chgrp построчно, оценил offline-пути и статус HMAC-асимметрии.

**Главный вывод.** POSIX-ядро (`permits`/`authorize_access`) само по себе корректно: `Action::Manage` строго требует `actor == owner` (или `System`), а групповая DDL централизованно гейтится `Manage` на Root. Прямой самоэскалации через `AddGroupMember` **нет** (см. §2). Но есть **системный структурный дефект**: ролевая система (`GrantRole` → роль `"superuser"` → `is_superuser` → `Actor::System`) — это ВТОРАЯ, независимая от POSIX ось привилегий, которая даёт полный admin-bypass, при этом (а) имя роли `"superuser"` нигде не зарезервировано/валидируется на слое `shamir-db`, (б) две таблицы пользователей/ролей (`shamir-db` `users` vs `shamir-server` `FjallUserDirectory`) рассинхронизированы, (в) grant роли `superuser` НЕ проходит HMAC-«did-you-mean-it», в отличие от `DropUser`. Плюс подтверждён и не закрыт прошлый пункт про HMAC-асимметрию.

## Топ-находки «ОБЯЗАНЫ УЛУЧШИТЬ»

| # | Серьёзность | Где | Суть | Эскиз фикса |
|---|---|---|---|---|
| 1 | **HIGH** | `crates\shamir-connect\src\server\session.rs:36`; `crates\shamir-server\src\db_handler\handler.rs:117-121` | Роль `"superuser"` → `is_superuser` → `Actor::System` (полный bypass POSIX-гейта). Ролевая ось привилегий существует ПАРАЛЛЕЛЬНО POSIX owner/group/mode и склеивается с ней только в одной точке — по строковому имени роли. `GrantRole`/`CreateUser` со `roles:["superuser"]` не валидируют это зарезервированное имя | Зарезервировать имя `"superuser"` (и `"replicator"`); grant/create с ним — только через отдельный явный привилегированный путь + HMAC; согласовать ролевую и POSIX-модель в единую политику |
| 2 | **HIGH** | `crates\shamir-db\src\shamir_db\execute\admin_users_roles.rs:16-82`; `crates\shamir-server\src\db_handler\admin.rs:37-125`; `crates\shamir-server\src\user_directory.rs` | Два несвязанных стора юзеров/ролей: `shamir-db` `users`-таблица (куда пишут `CreateUser`/`GrantRole`) и `FjallUserDirectory` (откуда `lookup_roles` берёт роли при логине, `handshake.rs:409`). Wire-логин НЕ читает `shamir-db` `users.roles` — рассинхрон прав, «фантомные» юзеры/гранты | Одна авторитетная таблица юзеров/ролей; либо документировать разделение и убрать ложную видимость управления правами через `GrantRole` |
| 3 | **MEDIUM→HIGH** | `crates\shamir-server\src\db_handler\admin.rs:137-215`; `crates\shamir-server\src\db_handler\handler.rs:341,378` | HMAC «did-you-mean-it» (`check_destructive_hmacs`) покрывает ТОЛЬКО `Drop*`/migration; `CreateUser`, `GrantRole/RevokeRole`, `Chmod/Chown/Chgrp`, `SetRetention/PurgeHistory`, group-CRUD идут БЕЗ HMAC. Утёкший живой superuser-тикет → grant `superuser` / chown / purge без подтверждения. Прошлая находка #5 — статус: НЕ закрыта | Расширить `check_destructive_hmacs` на privilege-granting/create/chmod/chown/retention |
| 4 | **MEDIUM** | `crates\shamir-db\src\shamir_db\execute\admin_users_roles.rs:16-82` | `handle_create_user` не проверяет коллизию `principal_id(username)` и вообще уникальность имени: пишет `SetOp` по ключу `name` слепо (upsert). Второй юзер с тем же именем ПЕРЕТИРАЕТ первого; коллизия `fxhash & i64::MAX` двух разных имён → общий owner-id → наследование ресурсов | Проверять `if exists by name` перед вставкой; вести обратный индекс `principal_id → name` и отвергать коллизии id |
| 5 | **MEDIUM** | `crates\shamir-db\src\shamir_db\execute\admin_access.rs:50-85` | `handle_chown` не проверяет существование целевого owner-id и допускает chown на произвольный u64 (в т.ч. `0`=System и несуществующего юзера). Owner может «подарить» ресурс System или потерять его безвозвратно | Валидировать, что owner-id соответствует существующему принципалу; запрет chown на `OWNER_SYSTEM` для не-System actor |
| 6 | **LOW→MEDIUM** | `crates\shamir-db\src\shamir_db\shamir_db\access_control.rs:93-95`, `resource_meta` для `User`/`Group` | chmod/chown/chgrp на `ResourceRef` не поддерживает User/Group (нет варианта в `ResourceRef`), а `resource_meta(User/Group)` всегда возвращает `open()` (owner=System). Права на сами объекты-принципалы неуправляемы и «жёстко открыты» на чтение через `access_tree`-резолвер | Смоделировать owner/mode для User/Group или явно задокументировать, что принципалы вне POSIX-модели |

---

## 1. Проверка ядра `permits` / `authorize_access` (корректно)

`crates\shamir-types\src\access.rs:612` `permits`:
- `Actor::System` → `true` (bypass) — строка 613.
- `Action::Manage` → `actor.to_owner_id() == meta.owner.to_owner_id()` — строка 617. Это единственный путь для chmod/chown/chgrp/grant. **Группового/other-обхода Manage нет** — union-семантики нет, только строгое равенство owner.
- Иначе — `class_of` (owner→group→other, first-match, `access.rs:592`) + `Mode::is_set`.

`crates\shamir-db\src\...\access_control.rs:348` `authorize_access`:
- System → `Ok` (строка 358).
- Traversal: каждый ancestor требует `Execute` (строка 371). Для `Manage`-операций на Root ancestors пусты (`ResourcePath::Root.ancestors()` = `[]`), так что gate сводится к target-check `permits(actor, open_root, Manage)` → `actor==System`. Для не-System user на Root это `false` → deny. **Корректно.**

Вывод: непривилегированный `Actor::User` не может пройти `Manage` на Root (owner Root = System) и `Manage` на чужой ресурс. Ядро держит.

## 2. Самоэскалация через группы (пункт 1 брифа) — НЕ найдена

`handle_add_group_member` (`admin_access.rs:242-277`) и все прочие group-CRUD (`create/drop/rename/remove`) **централизованно** вызывают `authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)` ПЕРЕД мутацией (строки 143, 176, 222, 261, 298). Поскольку Root owned by System, только `Actor::System` (т.е. `is_superuser`-сессия) проходит. Голые методы `access_control.rs:277-287` (`add_group_member`/`remove_group_member`) сами прав НЕ проверяют — но их единственный wire-достижимый вызыватель (диспетчер) уже прогейтил Root-Manage. **Пути, где ЛЮБОЙ авторизованный юзер зовёт `AddGroupMember`, нет.**

Замечание (не находка, но хрупкость): защита группового DDL держится ИСКЛЮЧИТЕЛЬНО на том, что диспетчер не забыл вызвать гейт — сами `create_group`/`add_group_member` (`access_control.rs:201,278`) не имеют внутренней проверки и доверяют вызывающему слою (как и отмечено в брифе). Любой новый вызыватель этих pub-методов, забывший гейт, откроет самоэскалацию. Рекомендация — перенести проверку `Manage(Root)` внутрь или ввести typestate «authorized».

Отдельно: гранулярность «Root-Manage на весь группостор» означает, что нет понятия «владельца группы» — группами может управлять только полный admin. Это консервативно-безопасно (не даёт делегированной самоэскалации), но означает, что делегированное управление группами невозможно; если позже введут «owner группы», надо будет пере-проверить, не появится ли путь «добавь себя в привилегированную группу».

## 3. Chmod / Chown / Chgrp (пункт 2 брифа)

Построчный разбор `admin_access.rs:13-122`:
- **chmod** (13-48): `to_path` → `authorize_access(actor, path, Manage)` → `resource_meta` → `meta.mode = op.mode` → `set_resource_meta`. Гейт корректен: только owner/System. Обходного пути через `Write` НЕТ — все три хендлера используют именно `Action::Manage`.
- **chown** (50-85): тот же гейт `Manage`. **НО** (находка #5): `meta.owner = Actor::from_owner_id(op.owner)` (строка 76) ставит произвольный u64 без проверки существования принципала. `op.owner` может быть `0` (=`OWNER_SYSTEM` → ресурс становится System-owned, только admin сможет им управлять) или id несуществующего юзера (ресурс «осиротеет» — никто, кроме System, не пройдёт Manage). Это не эскалация (нужен уже owner/admin), но footgun/DoS: владелец-не-admin может безвозвратно потерять управление ресурсом, отдав его System. POSIX ограничивает chown до root — здесь любой owner может chown (в т.ч. «сбросить» на System).
- **chgrp** (87-122): гейт `Manage`. `meta.group = op.group` — тоже без проверки существования группы (`op.group: Option<u64>`), можно поставить несуществующий gid. Низкий риск (group-класс просто никогда не сматчится), но грязь.

Обходного пути «не-owner делает chmod/chown через путь, проверяющий только Write» — **не найдено**; вся тройка проходит `Manage`.

## 4. CreateUser / DropUser (пункт 3 брифа)

`handle_create_user` (`admin_users_roles.rs:16-82`):
- Гейт: `authorize_user_lifecycle(op.database)` (`admin_dispatch.rs:141`) — global-admin (Manage Root) ИЛИ db-owner (Manage на `Database{db}`). System bypass.
- **Находка #4 — нет проверки уникальности.** Запись идёт слепым `SetOp` с ключом `{"name": …}` (`admin_users_roles.rs:63-67`) через `set_via_implicit_tx` — это upsert. Нет `if exists by name`, нет обратного индекса `principal_id → name`. Последствия:
  1. Повторный `CreateUser` с тем же `name` молча ПЕРЕТИРАЕТ существующего юзера (сбрасывает пароль-хэш/роли/скоуп) — а db-owner может создавать юзеров в своём скоупе, т.е. db-owner может перетереть запись юзера с тем же именем, если тот привязан к его db.
  2. `principal_id(name) = fxhash::hash64(name) & i64::MAX` (`access.rs:33`) — коллизия двух РАЗНЫХ имён в один owner-id даёт общий доступ к ресурсам (owner-класс матчится по id, не по имени, `class_of` `access.rs:593`). Код создания юзера коллизию id не детектит вовсе (проверяется только строковый `name` как ключ таблицы, да и то без exists-guard). Астрономически маловероятно для случайных имён, но это чистый id-коллизийный класс, о котором просил бриф — здесь он НЕ закрыт. Это критический системный момент: owner-идентичность на ресурсах — 63-битный fxhash имени, невалидированный, без обратного индекса.

  Замечание про «пересоздать себя, приняв id другого/удалённого юзера»: DropUser удаляет запись из `users`-таблицы (`admin_users_roles.rs:179-209`), но ресурсы, где `owner = principal_id(dropped_name)`, НЕ переназначаются (нет каскада). Создав нового юзера с тем же именем, получаешь тот же `principal_id` → **наследуешь все ресурсы, owned удалённым тёзкой**. Для wire-логина это, впрочем, гейтится отдельным `FjallUserDirectory` (см. #2), но на слое `shamir-db` owner-id воскрешается тривиально.

`handle_drop_user` (`admin_users_roles.rs:84-214`): гейт по скоупу удаляемого юзера (db-owner может дропать только юзеров своей db) — логика корректна; `scope=None` (юзер не найден) → только global-admin. Ок.

## 5. GrantRole / RevokeRole vs POSIX (пункт 4 брифа) — структурный дефект

**Да, есть отдельная ролевая система, и она конфликтует с POSIX-моделью.**

Цепочка эскалации (находка #1):
1. `SessionPermissions::from_roles` (`shamir-connect\src\server\session.rs:36`): `is_superuser = roles.iter().any(|r| r == "superuser")`.
2. `session_actor` (`shamir-server\src\db_handler\handler.rs:117-121`): `if is_superuser { Actor::System }`.
3. `Actor::System` → полный bypass в `permits`/`authorize_access`.

То есть **роль `"superuser"` — это НЕ POSIX-объект, а прямой тумблер в `Actor::System`**, минуя всю owner/group/mode-модель. Две независимые оси привилегий, склеенные строковым сравнением имени роли. Проблемы:
- **Имя `"superuser"` нигде не зарезервировано.** `handle_grant_role` (`admin_users_roles.rs:587`) и `handle_create_user` пишут любую строку в `roles`, включая `"superuser"`/`"replicator"`, без спец-обработки. Гейт на сам grant — `Manage(Root)` (строка 605), т.е. грантить может только текущий admin. Это единственная преграда: эскалацию до superuser может выдать только уже-superuser. Но HMAC-подтверждения на это НЕТ (см. #3) — в отличие от гораздо менее опасного `DropUser`.
- **Рассинхрон сторов (находка #2).** `GrantRole` на слое `shamir-db` пишет в `users`-таблицу системного стора. А wire-логин берёт роли из `FjallUserDirectory.lookup_roles` (`handshake.rs:409`), который пишется через `create_scram_user`/`update_roles` (`shamir-server\src\db_handler\admin.rs:118`) — ДРУГОЙ стор (fjall keyspace `users_v1`). Я не нашёл пути, где `shamir-db` `users.roles` попадает в `SessionPermissions`. Следствие: `GrantRole superuser bob` через query-builder DDL правит `shamir-db` таблицу, но на живой вход `bob` это может не влиять (роли берутся из fjall). Это одновременно (а) снижает эксплуатируемость эскалации через `shamir-db` `GrantRole`, но (б) создаёт опасную ложную видимость управления правами и потенциальный privilege-drift, если какой-то путь всё же сошьёт эти таблицы. Требует явного архитектурного решения: какая таблица авторитетна.

Противоречие «склеить одну систему через другую»: прямого пути «дай себе роль superuser без прохождения `authorize_access`» не найдено (grant гейтится `Manage(Root)`), но сам факт, что привилегия наивысшего уровня выражается ре-именуемой (`RenameRole`, `admin_users_roles.rs:365`) строковой ролью без резервирования имени — латентная мина. Например `RenameRole "superuser" → "x"` + создание новой роли `"superuser"` с пустыми permissions логически бессмысленны для POSIX-bypass (bypass смотрит только имя), но демонстрируют, что «permissions» роли (`CreateRoleOp.permissions`) на POSIX-путь вообще не влияют — роль `superuser` эскалирует по ИМЕНИ, а не по содержимому. Ролевые permissions (`roles`-таблица) — мёртвый груз относительно enforcement-гейта.

## 6. Offline / CLI / bootstrap-пути (пункт 5 брифа)

- `access_tree` (`access_control.rs:510`) — чистое read-only чтение; гейт `Manage(Root)` накладывает ВЫЗЫВАТЕЛЬ. `handle_access_tree` (`admin_access.rs:340`) корректно вызывает `authorize_access(actor, Root, Manage)` — не-admin `User` отклоняется. Комментарий «offline CLI runs as System» (строка 509) — это про прямой библиотечный вызов `db.access_tree(...)` в offline-CLI, НЕ через wire. Wire всегда идёт через `handle_access_tree` с реальным actor. **Бэкдора нет.**
- Множество management-путей (`db_management.rs:12,72,306`; `table_management.rs:29,148`; `validator_management.rs`; `function_management.rs`; `system_store.rs:145,170`) хардкодят `Actor::System`. Это библиотечные удобные обёртки (`create_db`, `add_repo`, `rename_table` без `_as`-суффикса) — они запускаются как System. **Ключевой вопрос: достижимы ли они удалённо?** Wire-путь (`handler.rs:386`) всегда зовёт `execute_as(session_actor(session), …)`, т.е. с реальным actor, а не через System-обёртки. System-обёртки — это внутренний/offline API (bootstrap, тесты, migration-раннеры). Я НЕ нашёл wire-эндпоинта, дёргающего `*_as(Actor::System)` или голую System-обёртку с недоверенным вводом. Bootstrap (`shamir-server\src\bootstrap.rs`, `server_launcher.rs:161`) — стартап-время, до приёма соединений. **Удалённо-достижимого System-пути (бэкдора) не найдено.** Но это держится на дисциплине «wire → только `execute_as(real_actor)`»: любой будущий wire-хендлер, вызвавший System-обёртку, станет бэкдором. Рекомендация — пометить System-обёртки `#[doc(hidden)]`/`pub(crate)` или добавить debug-assert «не из wire-контекста».

## 7. HMAC «did-you-mean-it» асимметрия (пункт 6 брифа) — статус на 2026-07-10

Прошлая находка (#5 в аудите 2026-07-06) **подтверждена и НЕ закрыта**. Точные координаты сегодня:

- `require_superuser` — `crates\shamir-connect\src\server\admin.rs:51-57` (гейт по `session.permissions.is_superuser`, без HMAC). Не изменился.
- Admin-гейт wire-хендлера — `crates\shamir-server\src\db_handler\handler.rs:341-350` (`if !is_superuser { deny admin ops }`).
- HMAC-гейт — `crates\shamir-server\src\db_handler\admin.rs:137-215` (`check_destructive_hmacs`), вызывается из `handler.rs:378`. Полный список покрытых `match`-веток (строки 157-195): `DropDb`, `DropRepo`, `DropTable`, `DropIndex`, `DropUser`, `DropRole`, `StartMigration`, `CommitMigration`, `RollbackMigration`. Всё остальное — `_ => continue` (строка 196), т.е. **HMAC НЕ требуется** для:
  - `CreateUser` (`batch_op.rs:485` — admin, но не в HMAC-списке),
  - `GrantRole` / `RevokeRole` (`batch_op.rs:490`) — **самое опасное: выдача роли `superuser`**,
  - `Chmod` / `Chown` / `Chgrp` (`batch_op.rs:492-494`),
  - `CreateGroup` / `DropGroup` / `AddGroupMember` / `RemoveGroupMember` / `RenameGroup` (`batch_op.rs:497-498…`),
  - `SetRetention` / `PurgeHistory` (`batch_op.rs:519-520`) — необратимая потеря истории,
  - `CreateRole` / `DropRole`(?) — `DropRole` покрыт, `CreateRole` нет.

**Сценарий эксплуатации.** Украден/перехвачен ЖИВОЙ superuser-тикет (без знания пароля — например утечка `session_id`/`resumption_ticket`, см. 1d/1b аудита 2026-07-06). Атакующий НЕ может дропнуть таблицу (нужен HMAC, ключ которого выводится из session-secret — а тикет его не содержит напрямую?), но МОЖЕТ: `GrantRole superuser attacker`, `Chown` чужого ресурса на себя, `PurgeHistory` (уничтожить audit-хвост), `Chmod 0o777` на секретную таблицу — всё БЕЗ HMAC-подтверждения. Асимметрия: наименее-разрушительный `DropRole` защищён HMAC, а выдача полного superuser — нет.

**Фикс.** Расширить `check_destructive_hmacs` match на `GrantRole`, `CreateUser`, `Chmod/Chown/Chgrp`, `SetRetention`, `PurgeHistory`, group-mutating ops; добавить соответствующие `canonical_*` в `shamir_query_types::hmac`.

---

## Рекомендации (сводно, по приоритету)

1. **Зарезервировать привилегированные имена ролей** (`"superuser"`, `"replicator"`) и провести grant/create с ними ТОЛЬКО через явный отдельный привилегированный путь с HMAC-подтверждением. Согласовать ролевую ось и POSIX Manage в единую модель (сейчас `superuser`-роль — необъявленный тумблер в `Actor::System`). [#1, #5]
2. **Расширить HMAC-«did-you-mean-it»** (`check_destructive_hmacs`, `db_handler\admin.rs:137`) на все privilege-granting/create/chmod/chown/chgrp/retention/group-mutating операции — устранить асимметрию с `Drop*`. [#3, §7]
3. **Единая проверка id-коллизии и уникальности на create-user** (и симметрично create-group): exists-guard по имени + обратный индекс `principal_id → name`, отвергающий коллизии; отказ от слепого upsert `SetOp` в `handle_create_user`. Рассмотреть каскад/запрет воскрешения owner-id после `DropUser`. [#4]
4. **Свести два стора юзеров/ролей к одному авторитетному** (`shamir-db` `users` vs `FjallUserDirectory`) или явно задокументировать разделение и убрать ложную видимость управления живыми правами через `shamir-db` `GrantRole`. [#2]
5. **Валидация целей chown/chgrp**: owner-id/gid должны соответствовать существующему принципалу/группе; запрет chown на `OWNER_SYSTEM` для не-System actor. [#5, §3]
6. **Инкапсулировать гейт в group-CRUD и user-lifecycle**: перенести `Manage(Root)`-проверку внутрь `create_group`/`add_group_member`/… или ввести typestate «authorized», чтобы новый вызыватель не мог случайно обойти проверку (сейчас безопасность держится на дисциплине диспетчера). [§2]
7. **Пометить System-обёртки** (`create_db`/`add_repo`/`rename_table`/… без `_as`) как `pub(crate)`/`#[doc(hidden)]` или добавить контекст-guard, чтобы исключить их случайное появление на wire-пути (бэкдор-предотвращение). [§6]
8. **Явно смоделировать или задокументировать** права на объекты-принципалы (`User`/`Group`): сейчас `resource_meta` для них всегда `open()` и `ResourceRef` их не адресует — chmod/chown над принципалами невозможен, а их метаданные жёстко открыты. [#6]

### Что подтверждено безопасным (не трогать)
POSIX-ядро `permits`/`class_of` корректно (first-match owner→group→other, `Manage`=строгое owner-равенство, нет union-обхода). Групповой DDL централизованно гейтится `Manage(Root)` — прямой самоэскалации через `AddGroupMember` НЕТ. chmod/chown/chgrp все проходят `Action::Manage` (нет обхода через `Write`). `access_tree` над wire гейтится `Manage(Root)`, offline-System-путь через wire недостижим. `DropUser` скоуп-гейтится корректно (db-owner дропает только своих). `create_group` монотонно-безопасно аллоцирует id (bump counter до записи, `access_control.rs:227-233`). `rename_group`/`rename_role` guard'ят уникальность имени назначения.
