בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью системы прав WASM-функций S.H.A.M.I.R. (защитный аудит)

_Агент: @fxx (max effort), 2026-07-10. Авторизованный защитный аудит собственной кодовой базы — фокус на POSIX-подобной системе прав «Shomer» для WASM-функций (`ResourcePath::Function`), setuid/`Security::Definer`, капабилити egress/env, и на перепроверке двух критичных находок сетевого аудита 2026-07-06 в WASM-слое._

**Метод.** Прошёл построчно путь привилегий функции: `authorize_access` (traversal + target) → `effective_fn_actor` (Definer/Invoker × setuid) → `build_invoke_ctx` / `invoke_function_in_db_as` → `FacadeDbGateway{actor}` → `execute_as(actor, …)` → per-op `authorize_access` внутри `execute_as`. Прогрел все call-site `effective_fn_actor` через grep, проверил хост-импорты `db_get/insert/query/execute`, `http_fetch`, засев `env.*`, а также путь создания функции по проводу (`CreateFunctionOp` → `admin_function::handle_create_function` → `create_function_*_as`). Сверился с моделью `ACCESS_HIERARCHY.md` (egress = капабилити-бит функции) и с тестами `enforcement_tests.rs` / `getter_only_e2e.rs`.

**Главный вывод.** Обе критичные дыры прошлого аудита в WASM-слое **закрыты по существу**: `effective_fn_actor` реально консультирует `Security::Definer/Invoker` + setuid и **применяется на всех 4 путях вызова**; `FacadeDbGateway::execute` (за `db_execute`) теперь маршрутизирует через `execute_as(self.actor, …)`, а `execute_as` делает **per-op `authorize_access`** по целевой таблице — т.е. гостевой сырой `BatchRequest` больше не исполняется «как System / без проверки». Компиляция недоверенного Rust на хосте частично укреплена (scrub env + timeout + запрет `include*!`/`env!`), но **фундаментально остаётся исполнением недоверенного кода на хосте БД без seccomp/rlimit** — это по-прежнему CRITICAL по остаточному риску. Новых дыр эскалации через System-owner/builtin-shadowing **не найдено**: по проводу нельзя ни выставить `Definer`/setuid при создании, ни создать System-owned привилегированную функцию.

## Топ-находки

| # | Серьёзность | Где | Суть | Эскиз фикса |
|---|---|---|---|---|
| 1 | **CRITICAL (остаточная)** | `crates\shamir-wasm-host\src\compile.rs:399-586` (`compile_rust_source_with_timeout`), `:495` (`env_clear`), `:537` (`wait_timeout`) | Недоверенный Rust-исходник по-прежнему компилируется на хосте БД: `cargo build` запускает build-scripts / proc-macro как **нативные процессы** с FS-доступом и без seccomp/rlimit. Форбид-скан `include*!`/`env!` (`:57-63`) — лексический и сам признан обходимым через proc-macro-зависимость. Env почищен, таймаут есть, но arbitrary-native-exec остаётся | Не компилировать недоверенный Rust на хосте вообще: принимать только валидированный `.wasm`. Если исходник неизбежен — вынести компиляцию в изолированный воркер (контейнер/gVisor/seccomp+rlimit, запрет сети, read-only FS, cgroup CPU/mem). Запретить path-deps proc-macro |
| 2 | **MEDIUM** | `crates\shamir-db\src\shamir_db\shamir_db\core.rs:605-609` (`build_net_gateway`); `net_allowlist` в `ShamirDb`; `context.rs:369` (`with_net`) | Egress-allowlist — **глобальный по БД** (`self.net_allowlist`), а НЕ per-function капабилити-бит, как декларирует `ACCESS_HIERARCHY.md:73-74`. Любая функция с любым owner, вызванная кем угодно, получает один и тот же egress-скоуп; секрет-грантов на net нет. TOCTOU-риска отзыва нет (гейт читается на каждый `http_fetch` из свежего снимка), но нет и per-function ограничения | Ввести per-function `net_grants` (как уже сделано для `secret_grants`), пересекать с хостовым allowlist в `build_net_gateway(fn_name)`; хранить в катал. записи функции |
| 3 | **LOW / информ.** | `crates\shamir-query-types\src\admin\types\function_ops.rs:17-25` (`CreateFunctionOp`); `admin_function.rs:44-66` | Проводной `CreateFunction` **не несёт** полей `security`/`secret_grants`/`visibility`/setuid → по проводу функция всегда создаётся `Invoker` + пустые гранты + `owned_enforced(caller)` (owner=caller, `0o700`). Это безопасно (нельзя выдать себе Definer при создании), но `Security::Definer` и `secret_grants` **недостижимы через wire** — функциональный разрыв, а не дыра. Достижимы только внутрипроцессно (`create_function_with_opts_as`) | Осознанно решить: либо добавить поля в op с гейтом (setuid/Definer/grants — только owner+Manage, как chmod), либо задокументировать разрыв |
| 4 | **INFO (подтверждение, НЕ дыра)** | `crates\shamir-db\src\shamir_db\execute\function_invoker.rs:19-53`; `db_gateway.rs:282-291` | Цепочечный setuid: гостевой `db_execute` может нести `Call`-оп → `invoke_call(actor=эффективный)` → `invoke_function_in_db_as(caller=эффективный)` → повторный `effective_fn_actor`. Это sudo-подобная цепочка, но каждый шаг проходит `authorize_access(Execute)` под текущим actor, поэтому эскалация невозможна без прав `Execute` на следующую функцию. Корректно | — (оставить; см. §4) |

---

## 1. Статус ранее найденных дыр (перепроверка 2026-07-06 → 2026-07-10)

### 1a. CRITICAL «db_execute без Actor и без скоупа» (был host_db.rs:166-190 / context.rs:299 / meta.rs:44-48) — **ЗАКРЫТ по существу**

Прошлый аудит: `db_execute` звал `gateway.execute(&req_bytes)` без `Actor`; `FnCtx.actor` никуда не прокидывался; `Security::Definer/Invoker` «NOT enforced».

Сегодня:

- `crates\shamir-wasm-host\src\wasm\host_db.rs:166-190` — сигнатура `db_execute` не изменилась (по-прежнему берёт сырой `BatchRequest` из гостевой памяти и зовёт `gateway.execute(&req_bytes)`), **но** реализация `execute` теперь актор-осведомлённая:
- `crates\shamir-db\src\shamir_db\shamir_db\db_gateway.rs:282-291` — `FacadeDbGateway::execute` декодирует `BatchRequest` и вызывает `self.shamir.execute_as(self.actor.clone(), &self.db_name, &req)`. Поле `FacadeDbGateway.actor` (`:29-36`) заполняется **эффективным** actor функции.
- `crates\shamir-db\src\shamir_db\execute\db_execute.rs:27-70` — `execute_as` делает (1) `authorize_access(actor, Database, Read)` и (2) **per-op** `authorize_access(actor, Table{repo,table}, action)` для КАЖДОГО op батча (Read→Read, Insert→Create, Set/Update→Write, Delete→Delete). System байпасит; `User` реально проверяется по mode-битам целевой таблицы. Тем самым сырой гостевой батч на чужую `secrets`-таблицу теперь ловится ACL, а не «самоограничением gateway».
- `FnCtx.actor` действительно прокидывается: `context.rs:299` (поле `actor`), `:384-392` (`with_actor`/`actor`), а `FacadeDbGateway` строится с `actor: actor.clone()` в `function_management.rs:689-693` и `:743-747`.
- `Security::Definer/Invoker` больше не «dead weight»: `access_control.rs:463-486` (`effective_fn_actor`) читает `FunctionMeta::security` из катал. записи и реализует таблицу решений (Definer→owner; Invoker+setuid→owner; Invoker−setuid→caller). Fail-closed: при not-found/ошибке возвращается `caller`, НЕ System (`:467-469`).

Остаточное замечание (не регрессия, а по дизайну): `db_get/insert/query` фиксируют `repo` из `HostState`, но `table` — любую; это ОК, потому что per-op ACL в `execute_as` проверяет каждую таблицу. `db_execute` допускает любой repo/table/op в батче — тоже под per-op ACL. Скоуп держится на ACL, а не на white-list имён — это консистентно с остальной системой прав.

### 1b. CRITICAL «компиляция недоверенного Rust на хосте с полным env» (был compile.rs:21,78,86) — **ЧАСТИЧНО закрыт, остаётся CRITICAL по остаточному риску** (см. Топ-находка #1)

Сегодня в `crates\shamir-wasm-host\src\compile.rs` добавлено (CRIT-6 / audit #440 part A):

- **Форбид-скан** `include!`/`include_str!`/`include_bytes!`/`env!`/`option_env!` (`:57-63`, `:399-414`) со стрипом строк/комментариев (`:96-198`) — отсекает самые дешёвые пути `env!("SECRET")`/`include_str!("/etc/…")`. Сам код признаёт (`:20-23`, `:79-82`), что это НЕ полный барьер (proc-macro-зависимость обходит).
- **Scrubbed env** (`:336-387`, применяется `:495` `env_clear()` + переигрыш allowlist): секреты `*_KEY`/`*_SECRET`/`DATABASE_URL` не наследуются дочерним `cargo build`.
- **Wall-clock timeout** `WASM_COMPILE_TIMEOUT = 120s` (`:47`, `:537-557`) с kill+reap и дренажом pipe на отдельных потоках — закрывает compilation-bomb/DoS-зависание.

Что НЕ закрыто: сам факт запуска `cargo build` = исполнение произвольного нативного кода (build.rs, proc-macro) на хосте БД без seccomp/rlimit/сетевой-изоляции/FS-изоляции. Модуль честно это фиксирует (`:12-16`: «Full seccomp/rlimit isolation is out of scope … targets win32»). Пока `create_function_from_source*` доступен (внутрипроцессно — по проводу он есть в `CreateFunctionOp.source`, `admin_function.rs:44-53`), недоверенный исходник компилируется на хосте. Это остаётся CRITICAL.

---

## 2. Детальный разбор (файл:строка + сценарий)

### 2a. `effective_fn_actor` — call-sites и корректность применения

`crates\shamir-db\src\shamir_db\shamir_db\access_control.rs:463-486`. Все call-site (grep):

- `function_management.rs:600` — `invoke_function_as` (после `authorize_access(caller, Function, Execute)` на `:591-599`).
- `function_management.rs:639` — `invoke_function_with_batch_as` (после Execute-гейта `:630-638`).
- `function_management.rs:688` — `invoke_function_in_db_as` (после Execute-гейта `:679-687`); эффективный actor кладётся и в `FacadeDbGateway.actor` (`:693`), и в `FnCtx.with_actor` (`:703`).
- `function_management.rs:742` — `invoke_function_in_db_with_batch_as` (после Execute-гейта `:733-741`).

**Порядок верный:** сначала `authorize_access(caller, …, Execute)` под РЕАЛЬНЫМ вызывающим (не эскалированным), затем эскалация. То есть право «выполнить функцию» проверяется по caller, а «под чьими правами она работает внутри» — по `effective_fn_actor`. Это правильная sudo-семантика.

**Fail-closed доказан тестами** `enforcement_tests.rs`: `effective_fn_actor_missing_meta_returns_caller_not_system` (`:469-485`) — отсутствующая запись → caller, никогда System; `effective_fn_actor_definer_escalates_without_setuid` (`:559`), `…_invoker_without_setuid_returns_caller` (`:607`), `…_invoker_with_setuid_legacy_escalates` (`:634`) — таблица решений покрыта.

### 2b. Owner функции при создании = caller (не дефолтный System) — **корректно**

`function_management.rs:213-215` — `create_function_with_opts_as` пишет `save_function(name, &record, &ResourceMeta::owned_enforced(actor.clone()))`. `owned_enforced` (`access.rs:203-209`) = owner=actor, group=None, mode=`0o700`. Т.е. новая функция приватна создателю. Проводной путь передаёт настоящего actor (`admin_function.rs:50,63` — `self.actor.clone()`), а `self.actor` в `ShamirAdminExecutor` берётся из `execute_as(actor, …)` (`db_execute.rs:76-80`), т.е. из аутентифицированной сессии.

Сценарий «анонимного/System-owner при создании» реализуется ТОЛЬКО когда вызывающий код явно передаёт `Actor::System` (внутрипроцессные удобные обёртки `create_function_from_wasm`/`_from_source` без `_as`, `:63-71`,`:93-101`, и `create_function_with_opts` `:123-131`). По проводу такого пути нет. Тест `owner_on_create_function_system_stays_system` (`tests/ddl_wire_e2e/ownership.rs:196`) фиксирует: System-owned функция создаётся System-owned намеренно и остаётся такой — но по проводу System недостижим для обычного клиента.

### 2c. Builtin-функции: owner и shadowing — **дыры не найдено**

Builtin `argon2id` регистрируется в `registry.rs:33-38` (`with_builtins`) БЕЗ катал. записи. Следствия:

- `effective_fn_actor("argon2id", caller)`: `load_function` вернёт `Ok(None)` → **возвращается caller** (`access_control.rs:467-469`), НЕ System. Т.е. builtin НЕ даёт System-полномочий вызывающему. Это ключевой момент безопасности: builtin без катал. записи → нет setuid/Definer → нет эскалации.
- `access_tree` (`access_control.rs:640`) метит builtin через `function_meta(&fname).is_none()` — согласуется: у builtin нет `FunctionMeta` в памяти и нет катал. строки.
- Shadowing builtin непривилегированным юзером: `create_function_with_opts_as` при `replace=false` откажет (`function_management.rs:164-169`, `contains(name)` → `AlreadyExists`). При `replace=true` — сначала `authorize_access(caller, FunctionNamespace, Create)` (`:141`). Даже если юзер перезапишет `argon2id` СВОЕЙ записью, он станет её owner (`owned_enforced(caller)`), и эффективный actor будет он сам (Invoker−setuid) — эскалации нет. System-owned он сделать не может.

Вывод: непривилегированный юзер не может через builtin-shadowing получить чужой/System owner.

### 2d. Кто может выставить setuid/Definer (§4 запроса) — **owner-only, дыры не найдено**

- `chmod` (setuid-бит): `admin_access.rs:13-47` — `handle_chmod` требует `authorize_access(actor, path, Action::Manage)`. `Manage` в `permits` (`access.rs:616-618`) = `actor == meta.owner` (System байпасит). Значит setuid на функцию ставит только её owner. Это ожидаемо (owner сам решает сделать свою функцию привилегированной).
- `Security::Definer`: по проводу НЕдостижим (`CreateFunctionOp` без поля, §Топ-3). Единственный способ — `create_function_with_opts*` внутри процесса, где вызывающий и так задаёт owner.
- **Нет** builtin/системных функций с owner=System И setuid/Definer, доступных любому юзеру: builtin вообще без катал. записи (→ caller, см. 2c), а System-owned setuid-функцию по проводу создать нельзя. Значит «бесплатной» System-эскалации через чужую setuid-функцию нет. Единственная эскалация — вызвать явно-привилегированную функцию другого юзера (owner=A, Definer/setuid), но только если A дал тебе `Execute` (mode-бит) на неё — это дизайн, как sudo-скрипт с setuid, установленный самим A.

### 2e. Капабилити net/env: ре-валидация vs кэш (§2 запроса)

- **net (egress).** `build_net_gateway` (`core.rs:605-609`) на КАЖДЫЙ вызов функции создаёт свежий `CurlNetGateway::new(self.net_allowlist.to_vec())` из текущего снимка allowlist. Гейт `check_url_allowed_resolved` (`net_gateway.rs:157-209`) выполняется на КАЖДЫЙ `http_fetch` (`host_http.rs:145`), с DNS-pin против rebind-TOCTOU. Кэша «старого разрешения» между вызовами нет: изменение `net_allowlist` применится к следующему вызову функции. Внутри одного долгого вызова снимок фиксирован (гейт всё равно перепроверяет каждый запрос против этого снимка) — окно отзыва ограничено длительностью одного вызова, что приемлемо. **НО** allowlist глобальный, не per-function (Топ-2) — это ослабление модели `ACCESS_HIERARCHY.md`, а не TOCTOU.
- **env (secret_grants).** Гранты читаются из `function_meta(name).secret_grants` на КАЖДЫЙ вызов (`function_management.rs:694-697`, `build_invoke_ctx` `core.rs:590-593`) и кладутся в `FnCtx.secret_grants`. `global_get("env.X")` гейтится этим множеством. `env.*` засевается из ОС (`context.rs:204-210`, `seed_env`) один раз в `GlobalVars`, но выдача гостю фильтруется per-invocation грантами. Отзыв гранта (пересоздание/replace функции с меньшим списком) применится к следующему вызову — кэша разрешения нет. TOCTOU не обнаружен.

Замечание (унаследовано из сет-аудита, вне основного скоупа): `global_set("env.X", …)` из гостя может ПЕРЕТЕРЕТЬ засеянное значение `env.*` в общем `GlobalVars` — целостность засеянного секрета/помеха другим функциям в том же процессе. Не эскалация прав, но стоит гейтить запись в `env.*`-неймспейс.

---

## 3. Что проверено и признано корректным (без находок)

- `execute_as` per-op ACL по целевой таблице для всех data-op (`db_execute.rs:52-70`), включая маршрут из WASM `db_execute`.
- `effective_fn_actor` применяется на всех 4 invoke-путях с правильным порядком (Execute под caller → эскалация).
- Fail-closed `effective_fn_actor` (not-found → caller, не System).
- Owner=caller при создании функции (`owned_enforced`), проводной actor из сессии.
- setuid/Definer — owner-only (Manage-гейт в chmod).
- Builtin без катал. записи → не эскалирует (caller), не shadow-эксплуатируем.
- Компиляц.: scrub-env + timeout + форбид-скан (частичное укрепление).
- net/env капабилити перепроверяются per-invocation, без кэша разрешения.

## 4. Рекомендации

1. **[#1, CRITICAL] Убрать компиляцию недоверенного Rust с хоста БД.** Предпочтительно — принимать только валидированный `.wasm` (путь `FunctionSource::Wasm` уже есть). Если исходник обязателен как продукт-фича — вынести `cargo build` в изолированный воркер: контейнер/gVisor или seccomp+rlimit+cgroup, read-only FS вне temp, запрет сети, лимит CPU/mem/времени, запрет path-deps proc-macro. Форбид-скан оставить как defence-in-depth, но не как единственный барьер.
2. **[#2, MEDIUM] Ввести per-function `net_grants` капабилити** (симметрично `secret_grants`): хранить в катал. записи функции, в `build_net_gateway(fn_name)` пересекать с хостовым allowlist. Приведёт код в соответствие `ACCESS_HIERARCHY.md:73-74` («egress — капабилити-бит функции»).
3. **[#3, LOW] Явно решить судьбу проводного `Definer`/`secret_grants`/setuid.** Либо добавить поля в `CreateFunctionOp` с owner+Manage-гейтом (setuid/Definer выставляет только owner, как chmod), либо задокументировать, что они внутрипроцессные, чтобы разрыв не воспринимался как баг.
4. **[env-целостность, MEDIUM] Гейтить `global_set` в `env.*`-неймспейс** из гостя, чтобы функция не перетирала засеянные ОС-секреты в общем `GlobalVars`.
5. **[дизайн-инвариант] Зафиксировать тестом «builtin никогда не эскалирует»:** e2e, где юзер зовёт builtin и проверяет, что эффективный actor — он сам, а не System (сейчас это следствие fail-closed, но явный regression-тест закрепит инвариант при будущих правках builtin-реестра).
6. **[периодическая ре-валидация]** Модель уже перечитывает net/env-капабилити per-invocation — сохранить этот инвариант при любой будущей оптимизации (не кэшировать `FnCtx`/gateway между вызовами разных эффективных actor).
