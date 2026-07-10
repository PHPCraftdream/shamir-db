בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью core-модели прав Shomer S.H.A.M.I.R. (защитный аудит)

_Агент: @fxx (max effort), 2026-07-10. Часть панели ревью системы прав. Авторизованный защитный аудит собственной кодовой базы. Фокус — чистая логика core-модели (типы `Actor`/`ResourcePath`/`Action`/`Mode`/`ResourceMeta`, функции `principal_id`/`class_of`/`action_perm`/`permits`, gate `authorize_access`/`effective_fn_actor`/`resource_meta`). Call-site coverage (какие операции реально зовут gate) — вне зоны, ревьюит другой агент._

**Метод.** Прочитал полностью `crates/shamir-types/src/access.rs` (core-типы + чистая функция решения), `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` (резолвер меты + enforcement gate + группы + setuid/definer), путь создания юзера `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs`, точку вычисления principal id `crates/shamir-connect/src/server/session.rs:246`, спецификацию `docs/roadmap/ACCESS_HIERARCHY.md`. Прошёл grep по всем call-site'ам `permits(` и `Action::List` — подтвердил, что enforcement централизован в `authorize_access` (единственные два `permits(...)` — `access_control.rs:371,383`). Проверил `parent()`/`ancestors()` по всем веткам match, целочисленные касты `u64→i64`, decision-table `effective_fn_actor`.

Главные находки — **fail-open дефолт при ошибке чтения каталога** (F1, системная), **отсутствие проверки коллизий principal_id при создании юзера** (F2, дизайн-риск с реальным birthday-bound), и их **комбинированная эскалация до System через definer/setuid** (F3). Логика `permits`/`class_of`/`action_perm`/`ancestors` — корректна, дыр не найдено (пункты 2, 3, 6, 7 — чисто).

## Топ-находки

| # | Серьёзность | Где (файл:строка) | Суть | Эскиз фикса |
|---|---|---|---|---|
| F1 | **HIGH** (fail-open) | `access_control.rs:44-99` (все ветки `rec.ok().flatten()…unwrap_or_default()`) | Ошибка БД при чтении меты (I/O, corruption, десериализация) неотличима от «не найдено» — обе дают `ResourceMeta::open()` (System-owned, 0o777). Любой транзиентный сбой чтения каталога → полностью открытый доступ, а не отказ. Fail-open вместо fail-closed. | Разделить `Err` и `Ok(None)`: на `Err` — вернуть `Result`/fail-closed (deny), логировать; `open()` только на явное `Ok(None)` для документированных «implicit» путей (folder #118) |
| F2 | **MEDIUM-HIGH** (дизайн) | `admin_users_roles.rs:16-82` (create_user) + `access.rs:33`, `session.rs:246` | При создании юзера НЕ проверяется коллизия `principal_id = fxhash64(name)&i63` с существующими юзерами. Два разных username с одинаковым 63-бит id → один `Actor::User(id)` для обоих: захват чужого владения/групп/сессий. Атакующий, который может создавать/переименовывать юзеров, форсит коллизию с жертвой (birthday-bound реалистичен на масштабе); id=0 (System) недостижим только «астрономически» — но это на удачу, а не по проверке | При create/rename user: вычислить `principal_id`, отклонить если id уже занят другим username (KeyExists) ИЛИ id==0; вести обратный индекс `id→name`. Долгосрочно — крипто-хэш или монотонный счётчик id вместо fxhash |
| F3 | **HIGH** (комбо F1+F2) | `access_control.rs:463-486` (`effective_fn_actor`) + F1 | Сам `effective_fn_actor` fail-closed корректно (`let Ok(Some(rec))… else return caller`). НО он читает `res_meta.owner` из `from_record`, где отсутствие поля `owner` → `Actor::System` (`access.rs:241`). Функция с загруженной записью, но БЕЗ поля `owner` и с `Definer`/setuid → эскалация вызывающего до **System** (полный admin-bypass в `permits`) | В `effective_fn_actor` отказывать в эскалации, если `res_meta.owner == Actor::System` (не эскалировать «до System» — только до реального не-System owner); либо трактовать missing-owner как fail-closed |
| F4 | **LOW-MED** (дизайн) | `access.rs:612-617`, `access_control.rs:358` | Единственный «суперюзер» модели — `Actor::System` (all-bypass). Нет пути group/other `Manage`, нет отдельного «root-owner админ-bypass» из спеки (ACCESS_HIERARCHY.md:70 «Root owner / admin bypasses»). `is_superuser` (роль) живёт отдельно в `SessionPermissions` и в core-`permits` не участвует — потенциальное расхождение двух моделей «админа» | Свести к единой модели: явный «superuser» concept либо маппинг роли `superuser`→`Actor::System` на входе в gate (передать другому агенту — это call-site) |
| F5 | **LOW** (наблюдение) | `access_control.rs:93-95` (Root→open), `admin_list.rs:31` | `Root` и `User`/`Group` всегда `open()` (0o777). Значит `List` на Root (list databases) проходит для ЛЮБОГО не-System юзера (Other-Read на 0o777, ancestors Root пусты). Это, вероятно, осознанно, но фиксирует, что имена всех БД перечисляемы любым аутентифицированным юзером | Если нежелательно — сделать Root-мету настраиваемой/enforced-дефолтом |

Пункты задания без находок (подтверждено чистыми): **2** (`class_of` first-match — корректная POSIX-семантика, не union; нет мест-нарушителей), **3** (`List`→Read + обязательный ancestor-Execute в `authorize_access` — обхода нет), **6** (`parent()`/`ancestors()` — ни одна ветка не рвёт цепочку раньше Root), **7** (касты `u64→i64` — безопасны, group id из недоверенного источника в этом файле не приходит).

---

## 1. F1 — Fail-open при ошибке чтения каталога (главная находка)

**Где.** `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:44-99`. Каждая ветка резолвера `resource_meta` имеет форму:

```rust
let rec = self.system_store.load_database(db).await;   // -> Result<Option<QueryValue>, _>
rec.ok()            // Result -> Option: Err стирается в None
   .flatten()       // Option<Option> -> Option
   .map(|r| ResourceMeta::from_record(&r))
   .unwrap_or_default()   // None -> ResourceMeta::default() == open()
```

Строки: Database `44-50`, Store `51-57`, Table `58-64`, Function `65-71`, FunctionFolder `72-82`, FunctionNamespace `83-92`. `ResourceMeta::default()` = `open()` = `{owner: System, group: None, mode: 0o777}` (`access.rs:176-182, 271-275`).

**Дефект.** `.ok()` на строках 45/52/59/66/77/88 сворачивает `Err(_)` (реальная ошибка storage: I/O-сбой, повреждение файла каталога, ошибка десериализации записи) в `None` — ровно тот же путь, что и легитимное «записи нет». Оба ведут в `unwrap_or_default()` → `open()`. Т.е. **любой транзиентный или структурный сбой чтения меты превращается не в отказ (fail-closed), а в полностью открытый доступ (fail-open).**

**Сценарий эксплуатации.** (1) Атакующий или естественная деградация вызывает ошибку чтения записи таблицы `foo` (например, повреждённая страница каталога, гонка при конкурентной перезаписи мета-записи, ошибка сериализации нового поля). (2) `resource_meta(Table{foo})` возвращает `open()` вместо реальной приватной меты (owner=victim, mode=0o700). (3) `authorize_access` для непривилегированного `Actor::User` проходит: `permits(user, open(), Read, _)` → Other-Read на 0o777 → `true`. Приватная таблица становится читаемой/записываемой всеми. Это же затрагивает **traversal**: ошибка чтения меты ancestor'а даёт Execute всем (0o777 → Other-x), открывая проход к глубоким объектам.

**Почему это системно.** Принцип безопасного дефолта — fail-closed: неопределённость авторизации = отказ. Здесь неопределённость = полный доступ. Любой будущий баг в слое чтения каталога (а их класс широк: миграции схемы мета-полей, гонки, corruption) автоматически становится auth-bypass, а не отказом в обслуживании. Инвариант «нет меты → open» задокументирован намеренно для backward-compat legacy-записей (`access.rs:161-163, 197-209` — legacy без mode-поля грузятся как open), но он **неразличимо** покрывает и ошибку чтения.

**Фикс.** Разделить `Err(e)` и `Ok(None)`:
- `Err(e)` → fail-closed: изменить сигнатуру `resource_meta` на `DbResult<ResourceMeta>` (или добавить вариант, который `authorize_access` трактует как deny) и логировать ошибку. `authorize_access` при ошибке резолва меты обязан вернуть `AccessError`, а не пропустить.
- `Ok(None)` → сохранить текущий `open()`-fallback ТОЛЬКО для документированных implicit-путей (never-created function folder, #118, `access_control.rs:73-75`), где open — сознательный выбор.

---

## 2. F2 — Нет проверки коллизий `principal_id` при создании юзера

**Где.** Создание юзера — `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:16-82` (`handle_create_user`). Юзер сохраняется по строковому ключу `name` (строки 63-67), а `principal_id` **вообще не вычисляется** на этом пути — id материализуется позже, только при аутентификации, в `crates/shamir-connect/src/server/session.rs:246-252` (`Session::principal_id` = `fxhash::hash64(&self.username) & (i64::MAX as u64)`), дублируя логику `access.rs:33-35`. Grep по всему пути create/rename user на `collision`/`principal_id`/`== 0` — **нет ни одной проверки** (см. также `rename_group`/`create_group` — там uniqueness по имени есть, но id-коллизии principal'ов не касаются).

**Дефект.** `principal_id` — некриптографический fxhash, суженный до 63 бит. Два различных username, дающих один 63-битный id, при аутентификации становятся **одним и тем же** `Actor::User(id)`. Вся модель владения сравнивает только id (`class_of`, `access.rs:593`: `actor.to_owner_id() == meta.owner.to_owner_id()`; `permits` Manage, `access.rs:617`). Следствия коллизии:
- юзер B, чей id совпал с id владельца A, получает класс **Owner** на всех ресурсах A (полный rwx + Manage — chmod/chown/chgrp);
- групповое членство хранится по id (`group_members` → `Vec<u64>`, `access_control.rs:313-322`), так что B наследует все группы A;
- сессии, tickets и invalidation также завязаны на username/id — размывается изоляция.

**Оценка эксплуатируемости (оба сценария из задания).**
- **Захват произвольного (не целевого) аккаунта / birthday.** При 63-битном пространстве коллизия любой пары появляется по birthday-bound на ~2^31.5 (~3 млрд) юзеров — для БД астрономически много, birthday-риск сам по себе не практичен. НО атакующий, который может **массово создавать юзеров** (или знает набор username'ов и может подбирать свои имена), ищет коллизию своего создаваемого имени с id **любого** привилегированного существующего аккаунта — это уже не полная birthday-пара, а «попасть своим свободно-выбираемым именем в фиксированный набор занятых id». Стоимость ≈ перебор username'ов до совпадения хэша с одним из N целей ≈ 2^63/N попыток — дорого для одной жертвы, дёшево когда цель — «любой из многих админов».
- **Целевой захват конкретной жертвы (знаем её username).** Найти второй username, коллизирующий с фиксированным hash(victim), — это поиск прообраза-коллизии для fxhash64&i63 ≈ 2^63 работы. fxhash НЕ криптографичен (простая мультипликативная свёртка), поэтому это может быть **сильно дешевле 2^63** аналитически (fxhash легко инвертируется/управляется побайтово) — потенциально практично для мотивированного атакующего с контролем над username. Это надо считать реальной угрозой, а не теоретической.
- **Попадание в `OWNER_SYSTEM = 0`.** `access.rs:21` — id 0 зарезервирован за System. Комментарии (`access.rs:31-32`, `session.rs:244`) полагаются на «hash64 ненулевой для непустого входа» и «астрономически маловероятно» — но это НЕ проверка, а вера. Для fxhash подобрать имя, дающее 0 после `&i63` (т.е. младшие 63 бита hash == 0), для некриптографической свёртки может быть выполнимо конструктивно. Юзер с id 0 → `Actor::from_owner_id(0) == Actor::System` (`access.rs:48-54`), и `permits` даёт **полный admin-bypass** (`access.rs:613`). Это прямой путь к эскалации до System, гейтящийся только отсутствием валидации.

**Фикс.** На create/rename user вычислять `principal_id(new_name)` и:
1. отклонять, если id уже принадлежит другому username (обратный индекс `id→name` в каталоге, KeyExists);
2. отклонять `id == OWNER_SYSTEM (0)` явно;
3. долгосрочно — заменить `fxhash&i63` на криптографический хэш (SHA-256 усечённый) или на монотонный счётчик id (как у групп, `create_group`, `access_control.rs:201-235`), развязав identity id от строкового имени.

---

## 3. F3 — Комбо: эскалация до `System` через missing-owner + Definer/setuid

**Где.** `effective_fn_actor`, `access_control.rs:463-486`; в связке с `ResourceMeta::from_record`, `access.rs:236-249`.

**Сам `effective_fn_actor` — fail-closed корректно.** Строка 467: `let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else { return caller.clone(); }` — на ошибку/не-найдено вызывающий возвращается без эскалации (не через `open()`-дефолт). Decision-table (475-485) верна: `Definer`→owner всегда; `Invoker`+setuid→owner; `Invoker`+clear→caller. Дыр в самой таблице нет.

**Дефект — на входе owner.** `res_meta = ResourceMeta::from_record(&rec)` (470). В `from_record` (`access.rs:237-241`) отсутствие поля `owner` в записи → `.unwrap_or(Actor::System)`. Т.е. функция, чья catalogue-запись **загрузилась успешно**, но **без явного поля `owner`** (legacy-запись, частично записанная запись, или запись, где owner потерян по F1-подобной причине на уровне поля), имеет `res_meta.owner == Actor::System`. Тогда:
- `Definer` → возвращает `Actor::System` → функция исполняется с полным admin-bypass;
- `Invoker` + setuid-бит → тоже `Actor::System`.

Любой непривилегированный вызывающий такой функции получает эффективный `Actor::System` внутри её тела — полная эскалация. Это ровно та комбинация, что упомянута в задании (missing/open owner + definer/setuid), и она **не** закрыта fail-closed guard'ом на строке 467 (тот ловит только «запись отсутствует целиком», а не «запись есть, поле owner отсутствует»).

**Сценарий.** (1) В каталоге существует функция с записью без поля `owner` (legacy до внедрения owner-поля, либо частичная запись) и с `SECURITY DEFINER` (или setuid-битом при Invoker). (2) Непривилегированный юзер вызывает функцию. (3) `effective_fn_actor` → `Actor::System`. (4) Тело функции (`db_execute` и пр.) исполняется под System — читает/пишет любые таблицы, DDL, создаёт юзеров.

**Фикс.** В `effective_fn_actor` перед возвратом owner проверять: если `res_meta.owner == Actor::System`, НЕ эскалировать (вернуть `caller` — нельзя «эскалировать до System» через definer/setuid; System-owned функция для не-System вызывающего должна либо явно требовать System-контекст, либо fail-closed). Дополнительно — трактовать missing-owner-поле как ошибку записи (fail-closed), а не как `System`.

---

## 4. F4 — Единственный «суперюзер» = `Actor::System`; расхождение с ролевой моделью

`permits` (`access.rs:612-625`): `Actor::System`→всегда true (613-615); `Manage`→только owner (616-618); групповой/other Manage отсутствует (это корректно по POSIX — chmod только owner/root). Отдельного «root/superuser» вне `Actor::System` в core нет. При этом спека (ACCESS_HIERARCHY.md:70) говорит «Root owner / admin bypasses all checks», а в рантайме существует независимый флаг `SessionPermissions.is_superuser` (роль `superuser`, `session.rs:26-40`), который в core-`permits` **не участвует**. Итог — две несведённые модели «админа»: core знает только `System`, сессия знает роль `superuser`. Как именно роль `superuser` маппится в `Actor` на входе в gate — call-site (зона другого агента), но фиксирую расхождение здесь для передачи: если роль `superuser` НЕ маппится в `Actor::System`, суперюзер не получит admin-bypass в core; если маппится — это неявная, не отражённая в core точка эскалации.

**Рекомендация.** Свести к единой модели: явный concept «superuser» в core (например `Actor::Super` или флаг), либо документированный однозначный маппинг роли на входе. Не оставлять две параллельные истины.

## 5. F5 — Root/User/Group всегда `open()` → перечислимость всех БД

`access_control.rs:93-95`: `Root | User | Group => ResourceMeta::open()` (жёстко, не из каталога). `admin_list.rs:31` авторизует list-databases как `authorize_access(actor, Root, List)`. Ancestors Root пусты (`access.rs:439` → parent=None), target-мета Root = 0o777, `List`→Read (`access.rs:579`), Other-Read set → `permits`==true для любого не-System юзера. Значит **имена всех баз данных перечислимы любым аутентифицированным юзером**. Аналогично User/Group всегда open. Вероятно осознанно (admin-домен), но фиксирую как факт разглашения структуры. Если нежелательно — Root-мету надо сделать резолвимой из каталога с enforced-дефолтом.

---

## Пункты без находок (подтверждено)

- **class_of / union-семантика (п.2).** `class_of` (`access.rs:592-600`): first-match owner→group→other, ранний возврат — строгая POSIX-семантика, не union. Grep по всем call-site'ам `permits(` дал только `access_control.rs:371,383` (внутри gate) + тесты — нигде класс не трактуется как объединение прав. Чисто.
- **List без traversal (п.3).** `List`→`Perm::Read` (`access.rs:579`) — осознанный дизайн (ACCESS_HIERARCHY.md:47). Но `authorize_access` (`access_control.rs:368-378`) требует `Execute` на КАЖДОМ ancestor ДО target-check. Все `Action::List` call-site'ы (`admin_list.rs`, `admin_replication.rs`) идут через `authorize_access`. Прямых `permits(...List...)` в обход gate нет. Контрпримера «List без traversal» не найдено. Чисто.
- **parent()/ancestors() (п.6).** Обошёл все ветки match `parent()` (`access.rs:437-478`): `None` возвращается ТОЛЬКО для `Root` (439). Все прочие варианты, включая `FunctionFolder`(456-465), `Function` со slash-именем (466-475, деривация папки из префикса), Record/Index (445-454, →Table), User/Group (476, →Root) — дают `Some`, цепочка `ancestors()` (482-490) идёт до Root. Способа сконструировать путь, чей `ancestors()` пропускает промежуточный уровень, не нашёл. (Замечание-нюанс, не дыра: `Function{name:"a/b/c"}`.parent → `FunctionFolder{[a,b]}`, чей parent → `FunctionFolder{[a]}` → `FunctionNamespace` → Root: все уровни присутствуют.) Чисто.
- **Целочисленные касты (п.7).** owner id: `principal_id` маскирован в i63, `to_owner_id() as i64` (`access.rs:218,257`) безопасен. group id: `group as i64` (`access.rs:224,262`) — group id аллоцируется монотонным счётчиком от 1 (`create_group`, `access_control.rs:201-235`), в этом файле недоверенный group_id не приходит извне без резолва через `resolve_group_id`. Пути передать сырой `u64::MAX` group_id из недоверенного источника в этот каст не нашёл. Чисто (замечание: если DDL Chgrp когда-либо примет сырой числовой gid от клиента без диапазон-проверки — `as i64` на значении > i64::MAX даст отрицательный wraparound; сейчас такого пути нет, но добавить range-guard при вводе gid стоит превентивно).

---

## Рекомендации по доработке

1. **Fail-closed вместо fail-open в `resource_meta` (F1, приоритет 1).** Разделить `Err` (deny + log) и `Ok(None)` (документированный open только для implicit function-folder). Сменить сигнатуру на `DbResult<ResourceMeta>`; `authorize_access` при ошибке резолва — отказывать. Это закрывает самый широкий класс «баг чтения каталога → auth-bypass».
2. **Проверка коллизий principal_id + запрет id 0 (F2).** На create/rename user: reject если id занят другим username или id==0; вести обратный индекс `id→name`. Долгосрочно — крипто-хэш (усечённый SHA-256) или монотонный счётчик id, развязка identity от строкового имени.
3. **`effective_fn_actor`: не эскалировать до System (F3).** Если `res_meta.owner == Actor::System`, definer/setuid НЕ должны давать System-контекст непривилегированному вызывающему; missing-owner-поле трактовать fail-closed.
4. **Единая модель суперюзера (F4).** Явный concept superuser в core (или задокументированный однозначный маппинг роли `superuser`→актор на входе в gate) вместо двух параллельных истин (`Actor::System` в core vs `is_superuser` в сессии).
5. **Крипто-хэш для principal id (стратегически).** Замена некриптографического fxhash на криптографический (или на выделенный счётчик id) устраняет и F2, и класс «управляемых коллизий», и риск конструктивного попадания в id 0.
6. **Range-guard на вводе числовых gid/owner id из клиента (превентивно, F5/п.7).** При будущем Chmod/Chown/Chgrp с сырым числовым id — проверять диапазон `<= i64::MAX` до каста.
7. **Root/User/Group-мета из каталога с enforced-дефолтом (F5, опционально).** Если перечислимость всех БД любым юзером нежелательна — сделать Root-мету настраиваемой.
8. **Убрать дублирование `principal_id`.** Логика продублирована в `access.rs:33-35` и `session.rs:246-252` (идентичный fxhash&i63) — свести к единственному источнику истины, чтобы будущая замена хэша (реком. 5) не разошлась между слоями.
