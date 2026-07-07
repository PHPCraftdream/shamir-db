בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью сетевой поверхности S.H.A.M.I.R. (защитный аудит)

_Агент: @fxx (max effort), 2026-07-06. Часть панели из 5 агентов ревью проекта после завершения векторной кампании. Это защитный аудит собственной кодовой базы (авторизованный, для укрепления)._

**Метод.** Прошёл state-machine handshake по коду: `handle_connection` → rate-limit(subnet) → чтение 1-го фрейма (bounded + timeout) → развилка resume/full → (full) lockout-precheck → `ServerHandshake::new` (binding-policy+version+nonce) → challenge → proof → `verify_proof` → session только после `Accepted`. **Пре-аутентификационного обхода authz на транспорт/connect-слое НЕ найдено** — сессия/данные недоступны до `ProofOutcome::Accepted` или полной валидации тикета. Полный SCRAM канально-связан (`identity_sig` покрывает `tls_exporter`), SCRAM-сравнения constant-time (`subtle`), ротация идентити защищена от двойного запуска и pre-rotation replay, счётчик тикетов monotonic-CAS. Основные проблемы — на **query/authz-слое** (подписки, WASM-шлюзы), в **DoS-осторожности** (незакрытые окна хендшейка, отсутствие post-auth троттлинга) и в **гигиене секретов** (derive-Debug на секрето-несущих структурах).

## Топ-5 «ОБЯЗАНЫ УЛУЧШИТЬ»

| # | Серьёзность | Где | Суть | Эскиз фикса |
|---|---|---|---|---|
| 1 | **CRITICAL** | `crates\shamir-engine\src\query\auth\session.rs:545` + `crates\shamir-server\src\subscriptions\bridge.rs`, `db_handler\subscribe_handler.rs:38` | `Subscribe` классифицируется как `(Read, Global)`; обещанная «проверка таблиц при активации» не выполняется — live `Records/Keys` отдаются без per-table read-ACL | Вызывать `authorize_access(actor, table, Read)` для каждого source в `bridge_task`/`activate_subscriptions`; не пушить неавторизованные таблицы |
| 2 | **CRITICAL** | `crates\shamir-wasm-host\src\compile.rs:21,78,86` | Компиляция недоверенного Rust-исходника на хосте БД с полным env → `include_str!("/etc/…")`/`env!("SECRET")` → эксфильтрация; компиляц. бомба без таймаута | Не компилировать недоверенный Rust на хосте; принимать только валидированный `.wasm`; иначе — scrubbed env + seccomp + rlimit + timeout + запрет `include*!`/`env!` |
| 3 | **HIGH→CRIT** | `crates\shamir-wasm-host\src\wasm\host_db.rs:166-190`; `context.rs:299`; `meta.rs:44-48` | `db_execute` пробрасывает сырой гостевой `BatchRequest` без скоупа repo/table/op и без `Actor`; `Security::Definer/Invoker` «NOT enforced» → межтенантное чтение/запись/DDL | Прокинуть `Actor` во все методы `DbGateway`, авторизовать внутри execute; whitelist таблиц для db_get/insert/query; валидация операций |
| 4 | **HIGH** | `crates\shamir-server\src\server\server_launcher.rs:830,888,958,899,967`; `crates\shamir-server\src\conn_limiter.rs` | `acceptor.accept`/WS-upgrade без таймаута удерживают глобальный conn-слот вечно; нет per-IP лимита. `auth_init_timeout` — только после TLS. Полный unauth-DoS с одного хоста | `tokio::time::timeout(handshake_timeout, …)` вокруг TLS-accept и WS-upgrade; добавить per-IP cap до accept |
| 5 | **HIGH** | `crates\shamir-connect\src\server\bootstrap.rs:213-227`; `server\admin.rs:64-78`; `server\config.rs:27-32` | `BootstrapRequest` (`token`+`server_key`), `CreateUserInput` (`server_key`), `ServerSecrets` (`server_secret`/`lockout_secret`) — `#[derive(Debug)]`/plain-массивы; латентная утечка секретов в логи, нет zeroize | Custom-redact `Debug` (как у `Session`/`UserRecord`), обернуть в `Zeroizing`; `ServerSecrets` не должен быть `Clone` |

---

## 1. ОБЯЗАНЫ УЛУЧШИТЬ

### 1a. Обход авторизации

- **CRITICAL — подписка обходит per-table read-ACL.** `crates\shamir-engine\src\query\auth\session.rs:545`: `BatchOp::Subscribe → (Action::Read, Resource::Global)`, комментарий обещает «table checks at activation», но в `crates\shamir-server\src\subscriptions\` нет ни одного `authorize_access`; `subscribe_handler.rs:38-62` спавнит bridge без ACL; `bridge.rs` (live-цикл `Records/Keys`) фильтрует лишь по имени/маске/фильтру.
  Сценарий: (1) логин обычным юзером с глобальным read, но без read на `secrets`; (2) `Execute{Subscribe(secrets, deliver=Records, initial=false)}`; (3) получаешь каждый live insert/update/delete с полным содержимым. Read-ACL таблицы обойдён.
  Фикс: авторизовать каждый source в `bridge_task`.

- **HIGH/CRITICAL — WASM `db_execute` без actor-скоупа.** `crates\shamir-wasm-host\src\wasm\host_db.rs:166-190` зовёт `gateway.execute(&req_bytes)`; `FnCtx.actor` (`context.rs:299`) не прокидывается ни в один метод `DbGateway`; `db_get/insert/query` фиксируют `repo` из `HostState`, но допускают любую `table`; `db_execute` — любой repo/table/op. `meta.rs:44-48` хранит `Security::Definer/Invoker`, но не применяет.
  Сценарий: функция низкопривилегированного юзера зовёт `db_execute` с батчем на `secrets.admin_tokens` (или DROP TABLE / create-user) — хост не накладывает скоуп, всё держится на самоограничении `FacadeDbGateway`.

- **CRITICAL (control-plane) — компиляция недоверенного Rust на хосте.** `crates\shamir-wasm-host\src\compile.rs:21` — см. Топ-5 #2.

- **MEDIUM — admin create/grant/chown/retention НЕ под HMAC-гейтом (только `is_superuser`).** `crates\shamir-connect\src\server\admin.rs:51-57` (`require_superuser`) и `crates\shamir-server\src\db_handler\admin.rs`/`handler.rs:341-350`; HMAC-гейт (`check_destructive_hmacs`, `db_handler\admin.rs:137-215`) покрывает только `Drop*`/migration. `CreateUser`, `GrantRole/RevokeRole`, `Chmod/Chown/Chgrp`, `SetRetention/PurgeHistory` идут без «did-you-mean-it» HMAC.
  Сценарий: утёкший живой superuser-тикет (без знания пароля) → `GrantRole`/`Chown`/`PurgeHistory` без HMAC-подтверждения, требуемого для Drop. Асимметрия защиты.
  Фикс: расширить HMAC-список на privilege-granting/create/retention.

### 1b. Секреты — зануление и Debug (класс ШИРЕ)

Проект следует конвенции redact-Debug для секрето-несущих типов (`Session`, `TicketPlain`, `UserRecord`, `StoredKey`, `Ed25519Keypair`, `BootstrapState`, `PendingChangePwChallenge`, `ServerSecrets`). **Конвенцию нарушают** (derive-Debug + отсутствие zeroize):

- **HIGH** `BootstrapRequest` (`server\bootstrap.rs:213`) — `token` (операторский admin-секрет) + `server_key`.
- **MEDIUM-HIGH** `CreateUserInput` (`server\admin.rs:64`) — `server_key`+`stored_key`.
- **MEDIUM** `ChangePwRequest` (`server\changepw.rs:35`) — `new_server_key`/`client_proof_old`/`new_stored_key`.
- **MEDIUM** `ServerSecrets` (`server\config.rs:27`) — `server_secret`/`lockout_secret` = plain `[u8;32]`, `#[derive(Clone)]` (копии расползаются), НЕ `Zeroizing`; Debug-redact есть, но нет wipe на drop/ротации. Это самые чувствительные долгоживущие секреты крейта (server_secret питает fake_blob → его утечка = enum-oracle).
- **MEDIUM** `ServerAuthOk` (`client\handshake.rs:53`) — `session_id` (bearer) + `resumption_ticket` (утечка на клиенте).
- **LOW-MED** `ResumeRequest` (`server\resume.rs:181`) — `channel_binding_now` (TLS-exporter).
- **LOW** `PendingChangePwChallenge` (`server\session.rs:57`, названный класс) — нет zeroize; `Session.channel_binding_at_auth` (`session.rs:130`) — TLS-exporter-дериватив, не зануляется на drop; `PushEnvelope.data` (`common\push_envelope.rs:33`) — содержимое записей в derive-Debug.
- **MEDIUM (целостность секрета)** WASM `global_set` может перетереть засеянный `env.STRIPE_KEY` (см. 2/M1).

### 1c. Timing (уже корректно — подтверждаю)

`verify_client_proof`/changepw/bootstrap-token/pin используют `constant_time_eq` (`common\crypto.rs:116`, `subtle`). Per-user HMAC-кэш намеренно НЕ применён во избежание real-vs-fake канала (`crypto.rs:94-105`). Latency-pad `[50,75]мс` floor (`common\latency.rs`), fake-blob branch-equivalent (`common\fake_blob.rs`). Замечаний нет.

### 1d. Фиксация/повтор сессий и канальное связывание

- **MEDIUM-HIGH — resumption-тикет НЕ связан со ЗНАЧЕНИЕМ TLS-канала.** `crates\shamir-connect\src\server\resume.rs` (`process_resume`): `plain.channel_binding_at_auth` хранится в тикете, но **никогда** не сравнивается с `request.channel_binding_now`; проверяется лишь СИЛА binding-режима (`check_anti_downgrade`, `ticket.rs:287`). Новая сессия создаётся с текущим exporter (`resume.rs:346,360`). → украденный тикет **портативен между TLS-сессиями/сетями**. Комментарий `session.rs:128-130` («…future ticket bindings») — устаревшая/нереализованная гарантия. Асимметрия: полный SCRAM канально-связан, resume — нет.
  Фикс: сравнивать exporter-значение (или переиздавать тикет с новым каналом) при resume на exporter-профиле.
- Уже корректно: replay точного тикета блокирует monotonic per-(user,family) counter-CAS (`resume.rs:294`, strict `>`); pre-rotation тикеты отвергаются по `identity_key_version` (`rotation.rs:100`); nonce all-zero и `client==server` nonce отвергаются (`handshake.rs:141,152`); resume проходит per-subnet rate-limit (`connection\handshake.rs:523`).

---

## 2. ГДЕ ОСТОРОЖНОСТЬ УПУСКАЕТ

### 2a. DoS — неограниченный вход / slow-loris

- **HIGH** TLS-accept / WS-upgrade без таймаута + нет per-IP conn-cap (`server_launcher.rs:830/888/958/899/967`, `conn_limiter.rs`) — Топ-5 #4. Единственная тривиальная unauth-DoS.
- **MEDIUM** Неаутентифицированный WS-пир буферизует 16 MiB до логической проверки 4 KiB (`crates\shamir-transport-ws\src\server.rs:42-51`; отбраковка `>4 KiB` — `framing.rs:163` уже ПОСЛЕ буферизации). 10k пиров ≈ 160 GiB. Фикс: accept с 4 KiB `max_message_size`, расширять после `auth_ok`.
- **MEDIUM** Post-auth request-loop без idle/per-read таймаута (`request_loop.rs:219-237`): объявляешь 16 MiB длину и капаешь по байту → держишь 16 MiB + слот; resume дёшев (без Argon2) → множится.
- **MEDIUM** Backoff/lockout-sleep держит conn-слот до 30 с (`connection\handshake.rs:659-667` + `latency.rs BACKOFF_CAP_MS=30_000`) — усиливает исчерпание слотов при отсутствии per-IP cap.

### 2b. DoS — дорогие post-auth операции без троттлинга

- **HIGH** Нет post-auth rate-limiting: лимитер только до handshake (`connection\handshake.rs:523`); в request-loop лишь in-flight-семафор (глубина, не частота). Батч/`TxBegin`/`Subscribe`-штормы идут на полной скорости.
- **HIGH** Нет лимита подписок per-connection/per-user (`subscriptions\registry.rs:39`, `subscribe_handler.rs:38`) — неограниченный рост bridge-задач + broadcast-receiver'ов (×16 сессий).
- **HIGH** Reactive-подписка (`DeliverMode::Batch/Call`) игнорирует операторские query-limits: `subscriptions\reactive.rs:64-76,113-151` использует `BatchLimits::default()` вместо `self.query_limits` → амплификация (один чужой insert → N дорогих 50-query батчей под твоим actor).
- **HIGH** WASM: агрегатный CPU-fan-out. `wasm\wasm_function.rs:341-343` ставит свежий `set_fuel(1e9)` на КАЖДЫЙ Store, включая вложенные `ctx.call` (`host_call.rs:81` лимитирует только глубину 32, не число) → на один запрос ≈ (итерации)×1e9 инструкций без общего потолка; время в host-await'ах вообще не списывается с fuel. Плюс чисто-CPU гость пиннит tokio-worker на весь бюджет (нет epoch, `wasm_function.rs:319-323`) → N параллельных замораживают рантайм. Фикс: `epoch_interruption` + wall-clock дедлайн + сквозной агрегатный бюджет.
- **HIGH — Argon2-семафор защищает НЕ ТОТ путь.** Путь верификации auth **не запускает Argon2** (server SCRAM = HMAC/HKDF; подтверждено: единственный `argon2id()` в connect — клиентский `common\scram.rs:52`). Docs `server\argon2_semaphore.rs:1-16` («attacker OOM'ит параллельными auth_init») — устаревшая гарантия. Реальный серверный Argon2: `funclib\crypto.rs:54` (гость/запрос-достижимый, `A2_MAX_MEMORY_KB = 1_048_576` = **1 GiB**, без cap на число вызовов), и деривация пароля в `shamir-server\src\bootstrap.rs:122` / `db_handler\admin.rs:90`. Гость/юзер, крутящий `argon2id(pw, salt, [1048576, 16, 16, 256])`, выжигает пул `spawn_blocking` (512) + память → OOM; семафор на 64 permit этот путь НЕ гейтит.
- **MEDIUM** WASM: `StoreLimitsBuilder` задаёт только `memory_size`, без `.table_elements/.tables/.instances` (`wasm_function.rs:324-326`) → `table.grow` (дешёвый fuel) растит funcref-таблицу на сотни МБ вне 64-MiB-капа при on-demand-fallback; нет cap размера `.wasm` перед `Module::from_binary`; нет cap числа host-вызовов.
- **LOW** `MAX_SESSIONS_PER_USER` форсится O(N)-скэном под глобальным `parking_lot::Mutex` на каждый успешный логин (`server\session.rs:323-360`); `count_for_user`/`snapshot_by_user` тоже O(N) — логины сериализуются.
- **LOW** PRECIS-нормализация имени гоняется на сыром wire-строке до 255-cap (`common\username.rs:44-59`) — ограничено 4 KiB-фреймом.

### 2c. SSRF / амплификация egress (WASM)

- **HIGH** allowlist проверяется по строке host без DNS-резолва (`net_gateway.rs:162-167`: приватным помечается только литеральный IP), а `wasm\host_http.rs:145` вообще не зовёт `check_url_allowed` (защита делегирована out-of-scope `CurlNetGateway`, без ре-проверки редиректов). allowlist=`*.attacker.com`, `meta.attacker.com`→`169.254.169.254` → чтение IMDS.
- **MEDIUM** Неканонические IP (`2130706433`, `0x7f000001`, `[::ffff:a9fe:a9fe]`) обходят детектор (`net_gateway.rs:170-191`).
- **LOW** Самописный `parse_url` (`net_gateway.rs:116-159`) может расходиться с curl (parser-differential); гость задаёт любые заголовки, включая `Host:` override (`host_http.rs:39-65`).
Фикс: резолвить и проверять КАЖДЫЙ IP (и после редиректов), пиннить соединение к проверенному IP, звать `check_url_allowed` в host-пути.

### 2d. Раскрытие внутренностей в ошибках

- **LOW-MED** Строка ошибки хендлера уходит на wire дословно: `common\dispatch.rs:105,156` → `ErrorEnvelope.error`; `db_handler\handler.rs:416` (`e.to_string()`), `tx_handlers.rs:196-200`; `db_handler\admin.rs:78-88` возвращает `argon2id: {e}`. Проза движка («Database 'x' not found», storage-детали, имена внутр. типов) достигает клиента.
  Фикс: маппить на стабильные коды + generic-текст; полную прозу — только в server-log (как уже сделано для `HandshakeError::Storage`, `connection\handshake.rs:644-649`).

### 2e. Лимиты на юзера

`MAX_SESSIONS_PER_USER=16` — есть и форсится. **Нет** per-user conn-cap до auth (только глобальный), **нет** per-connection subscription-cap (2b), **нет** per-user request-rate (2b). WASM: `global_set` без гейта пишет в процесс-глобальный `GlobalVars` (`host_globals.rs:5-27`, `context.rs:141,200-210`) → неограниченный рост кучи + межтенантное отравление/подмена `env.*`-секретов.

---

## 3. УЛУЧШИТЬ (комментарии/асимметрия/логирование)

- **Устаревшие комментарии о гарантиях.** `crates\shamir-server\src\connection\handshake.rs:302-338` — длинный поток-сознания разработчика («…we use a workaround… to avoid refactoring…»), противоречащий реальному коду (который вызывает `hs.verify_proof(&proof, ctx.identity_keypair_for_verify(), …)`). Убрать. `server\argon2_semaphore.rs:1-16` — docs заявляют защиту `auth_init`-пути, который Argon2 не запускает: переформулировать (гейт для user-creation/funclib, не для verify). `server\session.rs:128-130` — «future ticket bindings» не реализовано (см. 1d).
- **Асимметрия клиент/сервер.** Политика пароля (min 12) — только клиент (SCRAM-inherent, задокументировано — ок). KDF-floor проверяется лишь при createUser/bootstrap (`validate_server_floor`, `admin.rs:96`, `bootstrap.rs:170`) — прямая запись слабого user-record в обход createUser не проверяется (low). WASM `Security::Definer/Invoker` объявлен, но не форсится (`meta.rs`).
- **Логирование секрето-несущих структур** — см. 1b (derive-Debug на `BootstrapRequest`/`CreateUserInput`/`ChangePwRequest`/`ServerSecrets`/`ServerAuthOk`/`PushEnvelope`). Grep подтвердил: сейчас connect эти структуры в `{:?}`/`tracing` не подаёт, но derive — латентная мина под любую будущую диагностику.
- **WASM детерминизм** (`wasm\wasm_engine.rs:40-46`): не отключены `wasm_simd/bulk_memory/relaxed_simd`, нет `cranelift_nan_canonicalization` — расширенная JIT-поверхность и недетерминированные NaN (важно для репликации/консенсуса). threads/gc уже вырезаны (`Cargo.toml` `default-features=false`) — хорошо.

---

## 4. УСКОРИТЬ (по дороге, вторично)

- `crates\shamir-transport-ws\src\framing.rs:119` — `ws_send_sink` аллоцирует свежий `Vec` на каждое сообщение (scratch из `framer.rs:293` игнорируется); `framing.rs:170` — вторая копия `body`→`buf` поверх tungstenite-байтов.
- `crates\shamir-server\src\connection\request_loop.rs:218` — `frame_buf = Vec::new()` пересоздаётся каждую итерацию (не переиспользуется «zero-alloc» буфер).
- `crates\shamir-connect\src\server\ticket.rs:235,249` — `wire.ciphertext.clone()` на каждый decrypt (необходимо для in-place + fallback — минор).

---

### Что подтверждено безопасным (не трогать)
DB/repo-scope confusion **отсутствует** (`execute_as`/`tx_*_as` авторизуют именно `db_name`/repo запроса под `Actor::User`); destructive-HMAC-гейт на `Drop*` корректен (per-session `session.hmac_key()`); replication deny-by-default (`replicator`/superuser + per-repo `authorize_access`); interactive-tx guards (one-tx-per-session, `owner_sid` theft-guard, db/repo-pinning, staged-cap, reaper); §7.5 инвалидция сессий; push-backpressure (`SLOW_CONSUMER_THRESHOLD`); TLS 1.3-only без downgrade, приватный ключ в `Zeroizing`, Origin-валидация fail-closed, plain-транспорт только loopback; WASM: **нет WASI** (только 10 `shamir_host`-импортов), fuel включён и сбрасывается (один Store не висит вечно), 64-MiB mem-cap, свежий Store на инвокацию (нет утечки линейной памяти между вызовами), `ctx.call` depth-limit 32, host-import доступ к памяти bounds-checked, чтение `env.*` гейтится грантами, egress default-deny.
