בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью: стык «аутентифицированная сессия → Actor» в системе прав Shomer (защитный аудит)

_Агент: @fxx (max effort), 2026-07-10. Защитный аудит собственной кодовой базы (авторизованный, для укрепления). Фокус — НЕ handshake (он покрыт `docs/dev-artifacts/audits/2026-07-06-security-network-surface.md` и признан без пре-аутентификационного обхода), а **identity mapping** и его целостность во времени жизни сессии/тикета: как сессия/тикет превращается в `Actor`, используемый в `authorize_access`._

**Метод.** Прошёл по коду путь идентичности от полного SCRAM и от resume до enforcement-гейта:
`ServerHandshake` → `NormalizedUsername::from_raw` → `Session::new(user_id:[u8;16], username:String, roles)` (`connection/handshake.rs:202,405,414`) → на каждый запрос `session_actor(&Session)` (`db_handler/handler.rs:116`) → `Actor::User(session.principal_id())` где `principal_id = fxhash::hash64(username) & i64::MAX` (`server/session.rs:246`) → `ShamirDb::execute_as(actor,…)` → `authorize_access(actor, path, action)` → `permits` (сравнение `actor.to_owner_id()` с `meta.owner`, `access.rs:592–625`). Параллельно прошёл §7.5-путь ревокации (`connect/server/dispatch.rs:89,141`) и resume (`server/resume.rs:process_resume`).

**Ключевой вывод.** В системе сосуществуют **ДВА независимых идентификатора одного пользователя**:

| Идентификатор | Значение | Где используется |
|---|---|---|
| `user_id: [u8; 16]` | случайный (CSPRNG) при `insert`, устойчивый | §7.5-ревокация, кап сессий per-user, kickSession, тикет (`user_id`) |
| `principal_id(username): u64` | `fxhash::hash64(username) & i64::MAX`, **производный от строки-имени** | владелец/группа ресурсов (owner/group в каталоге), enforcement-`Actor::User`, chown/chgrp |

Enforcement (владение ресурсами, gate) целиком завязан на **username-производный** `principal_id`, а НЕ на устойчивый `user_id`. Это порождает identity-confusion при пересоздании пользователя с тем же именем, при коллизиях/нормализации имён и при (гипотетическом) переименовании. Ни один из этих путей не является пре-аутентификационным обходом — все требуют либо валидной сессии, либо superuser-действия — но каждый ослабляет целостность прав.

---

## Топ-находки

| # | Серьёзность | Где | Суть | Эскиз фикса |
|---|---|---|---|---|
| 1 | **HIGH** | `server/session.rs:246–252` + `db_handler/handler.rs:116–122`; `types/access.rs:33` | `Actor` привязан к `principal_id(username)` (fxhash имени), а НЕ к устойчивому `user_id[16]`. Пересоздание юзера с тем же именем (`DropUser`+`CreateScramUser`) даёт НОВОГО пользователя, который наследует ВСЕ ресурсы старого (owner совпадает) | Хранить numeric owner-id = стабильный id, не производный от имени; `Session` должен нести устойчивый principal-id, а не пересчитывать хэш имени на каждый запрос |
| 2 | **MEDIUM-HIGH** | `server/session.rs:251`; `types/access.rs:33` | `principal_id` = **некриптографический** fxhash, 63 бита, полностью предсказуем и подвержен подбору коллизий: атакующий может целенаправленно выбрать/зарегистрировать имя, чьё имя-хэш совпадает с `principal_id` жертвы → унаследовать её владение | Криптостойкая привязка id к имени НЕ решает (нужна отвязка id от имени вовсе — см. #1); как минимум задокументировать коллизионную модель и запретить самопроизвольную регистрацию имён |
| 3 | **MEDIUM** | `connect/server/changepw.rs` (весь модуль) — НЕ вызывается из `shamir-server` | Флоу `changePassword` (SCRAM-verify старого пароля + kill всех сессий + `tickets_invalid_before_ns=now`) реализован в `shamir-connect`, но **не подключён** к живому request-loop сервера (`grep change_password` по `shamir-server/src` пуст). Смена пароля через живой сервер недоступна → компрометированные тикеты нельзя отозвать сменой пароля | Прокинуть `verify_change_password_request_with_sid` + `finalize_change_password` в диспетчер сервера, либо явно задокументировать, что ревокация идёт только через admin `kickSession`/`updateUser` |
| 4 | **MEDIUM** | `superuser`→`Actor::System` (`handler.rs:117`); `resume.rs:381`; `ticket.rs:74` | Роль `superuser` в СЕССИИ/ТИКЕТЕ маппится в `Actor::System` (полный bypass гейта). Тикет несёт `roles` (снапшот на момент SCRAM) и resume пересобирает `SessionPermissions::from_roles` из них. Понижение прав юзера отражается на живой resume-цепочке лишь через `tickets_invalid_before_ns` (bump в `update_roles`), а не через переверификацию роли при resume | Подтверждать роли из авторитетного user-record при resume, а не доверять снапшоту в тикете; либо гарантировать, что каждое изменение ролей делает bump (сейчас — да, но зависит от вызова `update_roles`) |
| 5 | **LOW-MEDIUM** | `admin_access.rs:76,113,270,307` (Chown/Chgrp/AddGroupMember) | `op.owner` / `op.group` / `op.user` — сырой клиентский `u64`, записываемый в каталог/членство БЕЗ проверки, что id соответствует РЕАЛЬНОМУ существующему пользователю/группе. Опечатка/произвол админа создаёт «висячего» владельца, которого потом может унаследовать будущий юзер с подходящим именем-хэшем | Валидировать, что `op.owner`/`op.user` резолвится в существующий principal (сверять с directory) до записи |

---

## Детально

### 1. HIGH — enforcement-Actor привязан к имени, а не к устойчивому user_id

**Файлы:** `crates/shamir-server/src/server/session.rs:246–252`, `crates/shamir-server/src/db_handler/handler.rs:116–122`, `crates/shamir-types/src/access.rs:33–35`.

```rust
// session.rs:246
pub fn principal_id(&self) -> u64 {
    fxhash::hash64(&self.username) & (i64::MAX as u64)
}
// handler.rs:120
Actor::User(session.principal_id())
```

`Session` хранит ДВА идентификатора: устойчивый `user_id: [u8; 16]` (случайный CSPRNG, назначается в `FjallUserDirectory::insert` → `fresh_user_id`, `user_directory.rs:263,350`) и `username: String`. Enforcement-Actor вычисляется КАЖДЫЙ запрос как `Actor::User(fxhash(username))`. Владение ресурсами (`owner` в каталоге) хранится как `principal_id(username)` (`access.rs:213–219` `inject_into`; chown — `admin_access.rs:76`). Устойчивый `user_id[16]` в enforcement НЕ участвует вообще — он используется только для §7.5-ревокации (`connect/server/dispatch.rs:89,141`), капа сессий (`session.rs:339`) и как поле тикета.

**Сценарий эксплуатации (пересоздание юзера — inheritance):**
1. `alice` создаёт БД/таблицу; owner записывается как `principal_id("alice")`.
2. Админ дропает `alice` (`BatchOp::DropUser`) — при этом ресурсы `alice` остаются в каталоге с owner=`principal_id("alice")` (drop юзера не переносит владение).
3. Админ (или сам процесс регистрации) создаёт НОВОГО пользователя с тем же именем `alice` (`CreateScramUser`). Новый `alice` получает НОВЫЙ случайный `user_id[16]`, но ТОТ ЖЕ `principal_id("alice")`.
4. Новый `alice` логинится → `Actor::User(principal_id("alice"))` → проходит `permits` как **owner** всех ресурсов прежней `alice`. Полное наследование владения между двумя РАЗНЫМИ (по устойчивому id) субъектами.

Это классический identity-reuse: устойчивый `user_id` меняется, а enforcement-идентичность — нет. §7.5-ревокация здесь НЕ помогает: она бьёт по `user_id`, а новый юзер имеет свежий `user_id` с `tickets_invalid_before_ns=0`.

**Переименование:** явной операции `RenameUser` в кодовой базе НЕТ (`grep RenameUser/rename_user` — только `RenameRole`/`RenameGroup`). Это по факту спасает от «смены имени рвёт владение», но лишь потому, что смена имени невозможна; функция-фича, добавленная в будущем, немедленно откроет обратную сторону (переименованный юзер теряет ВСЁ своё владение, т.к. `principal_id` пересчитается от нового имени). Инвариант «id не производен от изменяемого атрибута» здесь нарушен by design.

**Фикс:** отвязать enforcement-id от строки-имени. Либо (а) использовать устойчивый `user_id` (свести к `u64` или хранить owner как 16-байтный id) как owner/actor; либо (б) `Session` должна нести заранее вычисленный стабильный principal-id, полученный из user-record при логине, а не `fxhash(username)` на каждый запрос. Владение при `DropUser` следует явно осиротить/передать System.

### 2. MEDIUM-HIGH — principal_id некриптографичен и подбираем

**Файлы:** `crates/shamir-types/src/access.rs:33`, `crates/shamir-server/src/server/session.rs:251`.

`principal_id = fxhash::hash64(username) & i64::MAX`. fxhash — не криптографический, публично известный алгоритм, обратимый в смысле лёгкого поиска прообразов/коллизий. Пространство — 63 бита, но найти ДВА имени с совпадающим `principal_id` (или имя, совпадающее с чужим известным id) вычислительно дёшево для fxhash (он не спроектирован против коллизий по прообразу).

**Сценарий:** если в развёртывании возможна самостоятельная/полу-доверенная регистрация имён (или админ создаёт юзеров по запросу), атакующий заранее подбирает имя `X`, такое что `principal_id(X) == principal_id("admin_service")` (или id владельца целевой таблицы), регистрируется под `X` и наследует владение целевого ресурса, проходя `permits` как owner. Комментарий в коде («collision-resistant for short username strings», `session.rs:242`) вводит в заблуждение: fxhash не даёт таких гарантий.

**Фикс:** тот же, что #1 — не выводить security-id из хэша имени. Пока не исправлено — задокументировать модель угроз и не допускать неконтролируемую регистрацию имён.

### 3. MEDIUM — changePassword не подключён к живому серверу; ревокация тикетов при смене пароля недоступна

**Файлы:** `crates/shamir-connect/src/server/changepw.rs` (весь), отсутствие ссылок в `crates/shamir-server/src/`.

`verify_change_password_request_with_sid` (SCRAM-verify старого пароля) и `finalize_change_password` (kill всех сессий юзера + вернуть `now_ns` для `tickets_invalid_before_ns`) реализованы корректно и constant-time (`changepw.rs:163` `constant_time_eq`). Но `grep change_password|ChangePw|changepw` по `crates/shamir-server/**` даёт **ноль** совпадений — флоу не проброшен в `dispatch_request_view`/request-loop сервера. То есть на живом сервере сменить пароль нельзя, а значит — нельзя отозвать украденные resumption-тикеты через смену пароля (типовой ответ на компрометацию).

**Последствие в контексте прав:** украденный живой тикет (см. также finding 1d прошлого аудита — тикет портативен между TLS-сессиями) продолжает давать доступ от имени жертвы; жертва не имеет self-service пути ревокации. Остаётся только admin `kickSession`/`updateUser` (bump `tickets_invalid_before_ns`), что требует привлечения администратора.

**Фикс:** подключить changepw-эндпоинты к серверному диспетчеру, обеспечив вызов `bump_tickets_invalid` (persist+fsync) и kill сессий; либо явно задокументировать, что ревокация — только admin-path.

_Замечание по корректности изоляции идентичности внутри самого changepw:_ `verify_change_password_request_with_sid` использует `session.username` (`from_normalized_unchecked`) и `session.channel_binding_at_auth` для `auth_message_cp` — привязка к сессии корректна; здесь identity-confusion НЕ обнаружено.

### 4. MEDIUM — superuser→System и доверие ролям из тикета при resume

**Файлы:** `crates/shamir-server/src/db_handler/handler.rs:117`, `crates/shamir-connect/src/server/resume.rs:381,392`, `crates/shamir-connect/src/server/ticket.rs:71–74`.

`session_actor`: `if is_superuser { Actor::System }` — суперюзер получает полный bypass гейта (`permits` возвращает `true` для `System`, `access.rs:613`). Роль `superuser` определяется из `SessionPermissions::from_roles(roles)` (`session.rs:35`). При resume `roles` берутся из ТИКЕТА (`TicketPlain.roles`, снапшот на момент полного SCRAM, `ticket.rs:71`), и `process_resume` пересобирает `SessionPermissions::from_roles(plain.roles)` (`resume.rs:381`) — авторитетный user-record при resume НЕ перечитывается для ролей.

**Сценарий:** админ понижает роли пользователя (`updateUser` без `superuser`). `FjallUserDirectory::update_roles` bumpает `tickets_invalid_before_ns` (`user_directory.rs:405`) — это то, что рвёт старые сессии/тикеты (по `user_id` в §7.5). Механизм РАБОТАЕТ, но целостность держится на инварианте «любое изменение прав всегда bumpает `tickets_invalid_before_ns`». Тикет самодостаточен и криптографически несёт `superuser`, поэтому если хоть один путь изменения прав не сделает bump (или bump отработает не по тому `user_id`), устаревший тикет продолжит давать admin-bypass. Тикет привязан к `user_id` (16 байт, внутри GCM-plaintext, `ticket.rs:54`) — подмена невозможна (GCM-тег покрывает всё), это хорошо; но `roles`-снапшот в тикете доверяется без переверификации.

**Фикс:** при resume перечитывать роли из авторитетного directory по `user_id` (а не доверять `plain.roles`), либо оставить снапшот, но формально гарантировать bump на КАЖДОМ пути изменения прав (сейчас — только `update_roles`).

### 5. LOW-MEDIUM — chown/chgrp/addGroupMember принимают непроверенный клиентский id

**Файлы:** `crates/shamir-db/src/shamir_db/execute/admin_access.rs:76,113,270,307`.

`handle_chown` пишет `meta.owner = Actor::from_owner_id(op.owner)` (сырой `u64` из запроса), `handle_chgrp` — `meta.group = op.group`, `handle_add_group_member`/`remove` — `op.user` (сырой `u64`). Все под `authorize_access(Manage)`/superuser-гейтом, то есть это доверенная админ-операция — не обход. Но НИ ОДИН из них не сверяет, что переданный id резолвится в реального существующего пользователя/группу. Админ может назначить владельцем несуществующий principal-id (опечатка), создав «висячее» владение, которое затем унаследует будущий юзер с подходящим именем-хэшем (усиливает #1/#2).

**Фикс:** валидировать `op.owner`/`op.user` против directory (existence-check) до записи в каталог; для группы — проверять, что gid существует (для `AddGroupMember` gid уже резолвится, но member-id — нет).

---

## Что подтверждено безопасным (в рамках этого фокуса)

- **§7.5-ревокация** (`connect/server/dispatch.rs:89–95,141–146`) корректно ключуется на устойчивый 16-байтный `user_id` и сравнивает `session.created_at_ns > tickets_invalid_before_ns` (`session.rs:259`). `tickets_invalid_before_ns` читается из авторитетного in-memory-кэша, warmed-all-at-open (`user_directory.rs:194–212,230`), с fsync на каждый bump (`user_directory.rs:322`). Стабильный id здесь — правильный выбор.
- **Тикет ↔ actor binding криптографичен.** `TicketPlain.user_id`/`username_nfc`/`roles` лежат ВНУТРИ AES-256-GCM-plaintext; AAD-tag покрывает весь plaintext (`ticket.rs:20,175–196`). Клиент НЕ может подменить `user_id`/`username`/`roles` в resume — любая правка ломает GCM-верификацию (`ticket.rs:237–261`). `plain.version != wire.version` — defense-in-depth (`ticket.rs:267`). Replay точного тикета блокирует monotonic per-(user,family) counter-CAS (`resume.rs:333`, strict `>`). Pre-rotation тикеты отвергаются по `identity_key_version` (`resume.rs:263`).
- **Resume НЕ доверяет клиентскому username/id.** `ResumeRequest` (`resume.rs:181`) несёт только `ticket_wire_bytes` + nonce + `binding_mode_now` + `channel_binding_now`; username/id/roles берутся ИСКЛЮЧИТЕЛЬНО из расшифрованного тикета (`resume.rs:269,357,393`), клиент не передаёт их отдельным полем. Подменить identity в resume-запросе нельзя.
- **Username-нормализация — полный RFC 8265 PRECIS UsernameCaseMapped** (`connect/common/username.rs:40–61`): width-mapping + case-fold (не наивный `to_lowercase`) + NFC + directionality + restrictions. `"Admin"`/`"admin"` → одинаковый нормализованный вид (единый principal — DoS-омоглиф через регистр не проходит); NFC применяется после case-mapping, так что NFC/NFD-эквиваленты сходятся к одной строке → одинаковый `principal_id`. Confusion между байтово-разными, но визуально/канонически идентичными именами НЕ обнаружено на уровне нормализации. Нормализация выполняется ДО хэширования (`connection/handshake.rs:202` → `Session` хранит `username.as_str()` пост-PRECIS, `handshake.rs:416`), так что `principal_id` считается от канонической формы. (Остаточный риск confusables ВНУТРИ IdentifierClass — за пределами PRECIS UsernameCaseMapped — существует, но это свойство профиля, а не дефект реализации.)
- **CreateUserInput валидируется до записи** (`connect/server/admin.rs:88–99`): `require_superuser`, дубль-имя reject, `kdf_params == server defaults`, `validate_server_floor`. Username к этому моменту уже пост-PRECIS (caller-normalized, комментарий `admin.rs:66`).
- **Admin-операции идентичности под `Manage`/superuser-гейтом** (`admin_access.rs`, `connect/server/admin.rs:51`): create/kick/update/unlock требуют `is_superuser`; chmod/chown/chgrp/group-ops требуют `authorize_access(Manage)` (для `System` — bypass; для `User` — owner-only). Пре-аутентификационного или cross-tenant пути к ним не найдено.
- **DB/repo-scope** авторизуется именно по `db_name`/repo запроса под текущим `Actor` (`execute_as`/`tx_*_as`, подтверждено прошлым аудитом) — промежуточного шага, где `Actor` конструируется из произвольного клиентского поля, НЕ найдено: единственный источник `Actor` в живом пути — `session_actor(&Session)`, а `Session` создаётся только после `Accepted`/валидного тикета.

_SCRAM/channel-binding не переаудитировался — см. `docs/dev-artifacts/audits/2026-07-06-security-network-surface.md` (handshake без пре-аутентификационного обхода; resume-тикет портативен между TLS-сессиями — finding 1d, решение #512 «no code change» по RFC 9266)._

---

## Рекомендации

1. **Отвязать security-identity от изменяемого/угадываемого атрибута (главное).** Владелец ресурса и enforcement-`Actor` должны нести УСТОЙЧИВЫЙ id (тот же `user_id`, что использует §7.5), а не `fxhash(username)`. Это одновременно закрывает #1 (наследование при пересоздании), #2 (подбор коллизий) и превентивно — будущий `RenameUser`.
2. **`Session` должна хранить готовый numeric principal-id, полученный из user-record при логине, и НИКОГДА не пересчитывать `fxhash(username)` на каждый запрос.** Если id обязан оставаться username-производным для обратной совместимости каталога — минимум задокументировать коллизионную модель fxhash и заблокировать неконтролируемую саморегистрацию имён.
3. **DropUser должен явно осиротить/передать владение** (owner → System либо запрет drop при наличии owned-ресурсов), чтобы пересоздание имени не давало наследования.
4. **RenameUser (если появится) обязан либо перенести владение на новый id, либо инвалидировать все сессии/тикеты юзера** (bump `tickets_invalid_before_ns` + kill), иначе старая сессия станет «чужой»/«ничьей».
5. **Подключить changePassword к живому серверу** (или явно задокументировать admin-only ревокацию), чтобы существовал self-service путь отзыва скомпрометированных тикетов; убедиться, что он делает `bump_tickets_invalid`(fsync)+kill.
6. **При resume перечитывать роли/`superuser` из авторитетного directory по `user_id`**, а не доверять `roles`-снапшоту в тикете; либо формально доказать инвариант «любой путь изменения прав всегда bumpает `tickets_invalid_before_ns`».
7. **Валидировать client-supplied id в chown/chgrp/addGroupMember** против directory (существование principal/группы) до записи в каталог.
8. **Явная Unicode-normalization policy на этапе CreateUser** уже де-факто есть (PRECIS в handshake); стоит прогонять `NormalizedUsername::from_raw` и НА пути создания (не только логина), чтобы `principal_id` создаваемого и логинящегося юзера гарантированно совпадали, и добавить (опционально) confusables-скрин поверх PRECIS для визуально-идентичных имён.
