# Token API v1: REST и WebSocket по bearer-токену

Доступ к БД третьих программ по токену в заголовке, без SCRAM и без клиентской библиотеки. Четыре канала: HTTP, WebSocket, HTTPS, WSS.

Статус: **нормативная спецификация, реализация не начата** (задачи TOKEN-T2..T8). Дизайн-обоснование и трассировка ревью: `../../dev-artifacts/design/token-api-rest-ws.md` (revision 3). При расхождении этого документа с дизайном действует этот документ.

Версии: документ v1; формат токена `format_version = 1`; пути `/shamir/v1/...`; WS subprotocol `shamir-v1`. **AUTH_PROTOCOL.md не меняется**: токены не участвуют в SCRAM-транскрипте, enum `TransportKind` и `BindingMode` не расширяются (IMPLEMENTATION_GUIDE §9, enum extension rule).

---

## 1. Каналы

| # | Канал | Listener | Эндпоинт |
|---|---|---|---|
| 1 | HTTP + Bearer | `kind: http`, `tls: false` | `POST /shamir/v1/request`, `GET /shamir/v1/ping` |
| 2 | WebSocket | `kind: http`, `tls: false` | `GET /shamir/v1/ws` |
| 3 | HTTPS + Bearer | `kind: http`, `tls: true` | как 1 |
| 4 | WSS | `kind: http`, `tls: true` | как 2 |

«Сертификат» в каналах 3 и 4 = **серверный** TLS-сертификат. Клиентские сертификаты (mTLS) в v1 не поддерживаются.

Граница безопасности — **тип listener-а**, не путь. Listener `kind: http` обслуживает только токены и никогда не создаёт SCRAM-сессий; listener `kind: tcp|ws|unix` никогда не принимает токен. Путь `/shamir/v1/ws` (токен) отличается от `/shamir/v1` и `/shamir/v1/browser` (SCRAM, TRANSPORT_WS) и обслуживается только `kind: http`. Listener `kind: http` не отдаёт никаких маршрутов без токена: `/shamir/v1/health`, `/shamir/v1/version`, `/admin/*`, метрики на нём не существуют (404).

## 2. Формат токена

```
shk1.<id>.<secret>
```

- `id` — 16 байт из OS CSPRNG, base64url без padding: 22 символа.
- `secret` — 32 байта из OS CSPRNG, base64url без padding: 43 символа.
- Разделитель `.` (вне алфавита base64url; допустим в `b64token` RFC 6750). Общая длина **ровно 71 символ**. Разбор позиционный по фиксированным длинам.
- Алфавит base64url: `A-Za-z0-9_-`. Разбор **каноничный**: ненулевые завершающие биты последнего символа (`id`: 4 бита, `secret`: 2 бита) → токен malformed (иначе у одного токена было бы несколько строковых представлений).
- Префикс `shk1` версионирует формат и делает утёкший токен находимым сканерами секретов.
- Секрет показывается **один раз** при создании. Сервер хранит только MAC, секрет не восстановим.

## 3. Запись токена и MAC

Хранится (msgpack), ключ `id`, вторичный индекс `owner_user_id -> [id]`:

| Поле | Тип | Смысл |
|---|---|---|
| `id` | bytes(16) | идентификатор |
| `owner_user_id` | bytes(16) | `user_id` владельца; у пересозданного аккаунта он новый, токены не наследуются |
| `label` | string | уникальна среди токенов владельца, не секрет |
| `mac` | bytes(32) | `HMAC-SHA256(token_key, "shamir-api-token" \|\| format_version \|\| key_id \|\| id \|\| secret)`, `format_version` и `key_id` по одному байту |
| `key_id` | u8 | 1 в v1 |
| `format_version` | u8 | 1 |
| `created_at_ns`, `expires_at_ns` | u64 | срок **обязателен**, не больше `tokens.max_ttl_days` |
| `mode` | `read_only` \| `read_write` | |
| `channels` | `rest` \| `ws` \| `both` | на каком эндпоинте токен принимается; по умолчанию `both` |
| `databases` | `null` \| [string] | allowlist баз; `null` = все базы, доступные владельцу |
| `revoked_at_ns` | `null` \| u64 | |

Секрет высокоэнтропийный, поэтому быстрый keyed MAC корректен (Argon2 стал бы рычагом CPU против сервера). `token_key` — 32 случайных байта в `server_meta` (IMPLEMENTATION_GUIDE §1.2), создаётся при bootstrap, **в v1 не ротируется** (`key_id` и `format_version` уже входят в MAC-вход, поэтому многоключевая схема потом добавится без смены формата). Утечка `token_key` или резервной копии `server_meta` = `token revoke-all` и перевыпуск всех токенов (IMPLEMENTATION_GUIDE §5.8). Снимок одного хранилища пользователей не позволяет проверять догадки офлайн.

Состояние токена хранится и в памяти (`scc`-карта `id -> TokenState`) вместе с `OwnerState { user_id, tokens_invalid_before_ns, present }`; на горячем пути fjall не читается.

## 4. Представление токена

1. Ровно один заголовок `Authorization`, схема `Bearer` (регистр схемы не важен), один пробел, токен.
2. Токен читается **только** из этого заголовка: не из query string, не из cookie, не из тела, не из `Sec-WebSocket-Protocol`.
3. Ноль или несколько заголовков `Authorization`, иная схема, лишние пробелы, длина ≠ 71, неканоничный base64url → malformed.

## 5. Проверка

Выполняется **по заголовкам запроса**, до чтения тела и до `101`:

1. Если подсеть клиента превысила лимит неудач токен-аутентификации → `429` с `Retry-After`, проверка не выполняется.
2. Разбор (§4, §2). Ошибка → шаг 8 (неудача).
3. Поиск `id`. Если `id` неизвестен, всё равно вычисляется HMAC на фиктивных данных, чтобы unknown-id и wrong-secret занимали сопоставимое время.
4. Сравнение MAC за постоянное время.
5. Состояние: токен не отозван; срок не истёк; владелец присутствует в `OwnerState` с тем же `user_id`; `created_at_ns >= owner.tokens_invalid_before_ns`.
6. Только после успеха шагов 2-5 (секрет подтверждён): `channels` содержит эндпоинт, на котором токен предъявлен (иначе `403 token_channel_denied`); правило plaintext listener-а (§11) нарушено → `403 token_plaintext_write_denied`.
7. Строится токен-сессия (§6).
8. **Неудача шагов 2-5 даёт единый `401`** без различий в теле, заголовках и (насколько обеспечивает реализация) во времени. Неудачи пополняют **отдельный** лимитер по подсетям (экземпляр `RateLimiter`); счётчик SCRAM `LockoutStore` токеном не трогается никогда (иначе знание `id` позволяло бы заблокировать SCRAM-вход владельца). Успех не сбрасывает счётчики других подсетей.

`tokens_invalid_before_ns` повышается для пользователя при: смене пароля, смене ролей, kick, удалении (аккаунт отсутствует). При **восстановлении из резервной копии** он повышается для всех пользователей, поэтому восстановленная копия не воскрешает отозванные токены.

## 6. Принципал и права

Токен действует как его владелец, но не как администратор:

- `Actor::User(principal64(owner))`.
- Права **строятся, а не копируются**: `SessionPermissions::new(is_superuser = false, is_replicator = false, roles = роли владельца)` независимо от флагов владельца. Все привилегированные проверки читают `session.permissions`, поэтому токен superuser- или replicator-владельца для обработчика — обычный пользователь.
- Эффективные права = пересечение живых прав владельца (движок проверяет на каждом запросе) и scope токена (`mode`, `channels`, `databases`). Токен никогда не может больше владельца и по умолчанию может меньше.
- Токен-сессия — серверная `Session` с `origin = Token { token_id, scope, cancel }` (SCRAM-сессия: `origin = Scram`). `origin` — серверное поле, не поле wire. Токен-сессии живут в **отдельном** `TokenSessionStore` со своими лимитами; SCRAM `SessionStore` токен-сессий не содержит, лимит SCRAM «16 сессий на пользователя» токены не расходуют.
- Токен-сессия не получает и не предъявляет resumption-тикеты (`origin == Scram` обязателен для тикетов).

## 7. Allowlist запросов (deny-by-default)

`ScopedHandler` оборачивает обработчик БД. Запрос декодируется один раз (те же проверки лимитов и alias caps, что на SCRAM-пути), затем классифицируется по таблице. Новый вариант `DbRequest` запрещён, пока его явно не классифицируют.

| `DbRequest` | REST | WS |
|---|---|---|
| `Ping`, `Execute`, `WaitForVisibility`, `GetDdlOpStatus` | разрешён | разрешён |
| `TxBegin`, `TxExecute`, `TxCommit`, `TxRollback` | `not_supported_over_rest` | разрешён |
| `CreateCursor`, `FetchNext`, `CancelCursor` | `not_supported_over_rest` | разрешён |
| `CreateScramUser`, `SetSuperuser`, `SetReplicator`, `ChangePasswordChallenge`, `ChangePasswordVerify`, `Repl` | `token_request_denied` | `token_request_denied` |
| `CreateApiToken`, `ListApiTokens`, `RevokeApiToken`, `RevokeUserTokens` (§12) | `token_request_denied` | `token_request_denied` |
| любой иной / новый вариант | `token_request_denied` | `token_request_denied` |

Внутри `Execute` и `TxExecute` batch обходится `BatchTreeVisitor` по **всем** листьям, включая статические тела `ForEach`:

- **Admin-листья запрещены** (`BatchOp::is_admin`: DDL, пользователи, роли, права, миграции, функции, валидаторы, репликация и т.д.) с `token_request_denied`, **кроме** ровно четырёх интроспекционных операций, которые грубый admin-gate SCRAM-пути тоже пропускает (`is_coarse_admin_gate_exempt`): `List`, `AccessTree`, `DescribeTable`, `GetTableSchema`. Они по-прежнему проходят собственную авторизацию по таблице/пути. Следствие: токен не выполняет DDL и не управляет доступом, как и обычный не-superuser SCRAM-пользователь.
- `Subscribe` и `Unsubscribe`: на REST → `not_supported_over_rest` (push-канала нет, операция молча ничего бы не делала); на WS разрешены.
- Токен с `databases != null`: `List::Databases`, `List::Users`, `AccessTree` без указания базы внутри allowlist запрещены с `token_database_denied` (иначе метаданные других баз утекают через интроспекцию).
- `mode = read_only`: любой лист с `BatchOp::is_write()` запрещает batch целиком (`Call` классифицируется как запись); `TxExecute` с записью и `TxCommit` запрещены; `token_read_only`. `TxBegin` read-only токеном допускается (read-only транзакция).
- `databases != null`: поле `db` каждого запроса, несущего базу (`Execute`, `TxBegin`, `TxExecute`, `TxCommit`, `TxRollback`, `WaitForVisibility`, `GetDdlOpStatus`, `CreateCursor`), должно входить в allowlist, иначе `token_database_denied`.

Результат классификации типизирован (`Allowed` / `Denied(code)` / `NotSupported(code)`): REST-слой выбирает статус из него, не разбирая ответ обработчика повторно.

## 8. Коды ошибок

Ошибки приложения остаются единой таксономией `DbResponse::Error { code, message }` (TRANSACTION_OUTCOMES.md и `db_message.rs`). Токен добавляет коды:

| code | Когда | REST статус |
|---|---|---|
| `token_read_only` | запись при `mode = read_only` | 403 |
| `token_database_denied` | база вне allowlist | 403 |
| `token_request_denied` | запрос или лист вне allowlist | 403 |
| `token_channel_denied` | токен предъявлен на эндпоинте, не входящем в `channels` | 403 (также перед `101`) |
| `token_plaintext_write_denied` | `read_write` токен на удалённом plaintext listener-е без `allow_plaintext_write` | 403 (также перед `101`) |
| `not_supported_over_rest` | tx / cursor / Subscribe через REST | 400 |
| `invalid_request` | тело не декодируется в `DbRequest` (включая неизвестные ключи `BatchRequest`) | 400 |

Административные операции (§12) добавляют `token_label_exists`, `token_limit_reached`, `token_ttl_invalid`, `token_not_found`, `token_self_service_disabled`; для них действуют существующие `permission_denied`, `hmac_required`, `hmac_mismatch`. Ошибки, возникшие **после** декодирования внутри движка, остаются `200` с `DbResponse::Error` (в том числе `invalid_request`, выданный движком по содержимому).

Ни токен, ни секрет, ни MAC не появляются ни в одном `code`, `message`, логе, метрике или аудит-записи.

## 9. REST (каналы 1 и 3)

| Метод и путь | Смысл |
|---|---|
| `POST /shamir/v1/request` | тело: один msgpack `DbRequest`; ответ: msgpack `DbResponse` |
| `GET /shamir/v1/ping` | эквивалент `DbRequest::Ping`, ответ `DbResponse::Pong` |

`Content-Type: application/msgpack` на входе и выходе. JSON-тел в v1 нет (JSON не переносит числовые и бинарные типы движка, NUMERIC_WIRE_SEMANTICS.md).

**Аутентификация до чтения тела.** Токен проверяется по заголовкам (§5); тело читается только после успеха, под ограничениями `max_request_body_bytes` (по умолчанию потолок data-кадра, 16 MiB) **и** общего бюджета тел «в полёте» на все соединения. Дедлайны чтения заголовков, тела и общий задаются существующим механизмом `auth_init_timeout`; истечение → `408` и закрытие.

**Инварианты запроса** те же, что у SCRAM-запроса: бюджет байтов ответа (`run_with_handoff`, guard держится до записи тела), пост-аутентификационный rate limit токен-сессии (bucket кешируется на `id` токена; каждый запрос получает новую токен-сессию со свежим случайным `sid`), лимиты запросов, редактирование внутренних ошибок.

| Ситуация | Статус | Тело |
|---|---|---|
| успех, в том числе `DbResponse::Error` движка | 200 | msgpack `DbResponse` |
| malformed / неверный / просроченный / отозванный токен (§5.8) | 401 | `{"error":"unauthorized"}` (msgpack), `WWW-Authenticate: Bearer` |
| `token_read_only`, `token_database_denied`, `token_request_denied`, `token_channel_denied`, `token_plaintext_write_denied` | 403 | `DbResponse::Error` |
| `not_supported_over_rest`, `invalid_request` | 400 | `DbResponse::Error` |
| неподдерживаемый метод / путь | 405 (`Allow`) / 404 | фиксированное |
| таймаут головы или тела | 408 | фиксированное, закрытие |
| тело больше лимита | 413 | фиксированное, закрытие |
| неверный `Content-Type` | 415 | фиксированное |
| голова больше 8 KiB или больше 64 заголовков | 431 | фиксированное, закрытие |
| `Expect: 100-continue` | 417 | фиксированное |
| rate limit (подсеть или токен-сессия) | 429 | `Retry-After` |
| остановка / перегрузка | 503 | `Retry-After`, `Connection: close` |

**Framing** (только HTTP/1.1, без pipelining): запрос с одновременными `Content-Length` и `Transfer-Encoding`, повторным или нечисловым `Content-Length`, obsolete line folding, голым LF, `Transfer-Encoding` кроме `chunked` отвергается `400` с закрытием соединения; любая аномалия framing закрывает соединение. Ответы несут `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, на TLS ещё `Strict-Transport-Security`. **Заголовков CORS нет никогда**; браузерный кросс-доменный клиент вне scope v1.

## 10. WebSocket (каналы 2 и 4)

**Upgrade.** `GET /shamir/v1/ws`, `Upgrade: websocket`, `Sec-WebSocket-Protocol: shamir-v1`, `Authorization: Bearer <token>`. Токен проверяется до `101` (§5): неудача → HTTP-ответ без апгрейда (`401`/`403`/`429`/`503`). Неверный upgrade (нет заголовков, иной subprotocol) → `400`. `Origin` необязателен; если есть, должен совпасть с `browser_origin_allowlist` listener-а, пустой allowlist отвергает любой запрос с `Origin` (`403`). Исчерпание лимитов сокетов (§10.3) → `503` до `101`.

**Привязка сессии.** Сразу после `101` сервер шлёт один бинарный кадр (msgpack):

```
{ "ok": true, "sid": <32 случайных байта>, "expires_at_ns": N }
```

Дальнейшие кадры — обычные конверты `{sid, rid, req}` ↔ `{rid, res}` (TRANSPORT_TCP §6). **Единственный принимаемый соединением `sid` — выданный в первом кадре.** Кадр с иным `sid` → close `4010`. `sid` не нулевой и случаен; сессия существует только в `TokenSessionStore` этого соединения. Это закрывает подмену: токен-`sid` не действует на SCRAM-соединении (SCRAM-сервер его не знает), SCRAM-`sid` не действует на токен-соединении.

**Цикл.** Как пост-аутентификационный цикл запросов, со следующими отличиями: сессия берётся из соединения, `sid` кадра только сравнивается; перед каждым вызовом обработчика состояние токена перепроверяется (отзыв / срок / владелец, Acquire), поэтому после возврата вызова отзыва ни один запрос не авторизуется; цикл имеет ветку отмены по `cancel` токен-сессии; при отмене подписки, принадлежащие сессии, снимаются, push прекращается без каких-либо действий клиента. Только binary-кадры (text → close 1003), лимиты кадров и heartbeat как в TRANSPORT_WS §7, §8.

**Коды закрытия** (регистрируются в TRANSPORT_WS §6; `4003` занят `heartbeat_timeout`):

| Code | Имя | Когда |
|---|---|---|
| 4010 | `token_sid_mismatch` | кадр с `sid`, отличным от выданного |
| 4011 | `token_revoked` | токен отозван; владелец kick, удалён, сменил пароль или роли (повысился `tokens_invalid_before_ns`) |
| 4012 | `token_expired` | истёк `expires_at_ns` токена или `tokens.session_max_age_hours` |

Остальные коды TRANSPORT_WS §6 действуют как есть (`4000` и `4001` на токен-пути не используются: аутентификация происходит до `101`). Остановка сервера закрывает сокеты кодом `1000`.

**Время жизни.** Сессия заканчивается в `min(token.expires_at_ns, now_at_open + session_max_age)`; SCRAM `SESSION_MAX_AGE` (24 ч) не применяется. Соединение живо до отзыва, срока, закрытия пира или `heartbeat_timeout`; отдельного idle-таймера у токен-сессий в v1 нет.

**10.3 Учёт сокетов.** `tokens.max_sockets_per_token` (по умолчанию 8) и `tokens.max_sockets_per_owner` (по умолчанию 32). Токен-сокеты не расходуют лимит SCRAM-сессий владельца.

**Отзыв.** Порядок «опубликовать, затем отменить»: сначала флаг отзыва в памяти (его видят перепроверки и новые проверки), потом срабатывает `cancel` всех сессий токена; сокет закрывается кодом `4011` в том же шаге публикации. Запрос, уже прошедший перепроверку, завершается; новый не авторизуется.

## 11. Транспортная безопасность

**TLS (каналы 3 и 4).** Только TLS 1.3, без 0-RTT, ALPN `http/1.1`, тот же материал `TlsConfig`. Для токен-клиента единственная аутентификация сервера — сертификат (у него нет application-level Ed25519 pin SCRAM-клиента). Поэтому **TLS listener на не-loopback адресе отказывается стартовать с автосгенерированным сертификатом**, если у listener-а не задано `allow_self_signed: true`; в этом случае при старте печатается SHA-256 от SPKI сертификата для раздачи клиентам. Для сторонних клиентов нужен сертификат от CA.

**Plaintext (каналы 1 и 2).**

- `tls: false` обязан биндиться на loopback (`127.0.0.0/8`, `::1`); `Config::validate` отвергает остальное, как для `profile: plain` (IMPLEMENTATION_GUIDE §2.2).
- `allow_plaintext_remote: true` разрешает не-loopback bind: WARN при старте, gauge `shamir_token_listener_plaintext_remote = 1`. На таком listener-е принимаются **только `read_only` токены**, пока не задан `allow_plaintext_write: true`. Проверка идёт на шаге 6 §5 по конфигурации listener-а, **не** по адресу пира (за прокси правило по адресу пира бессмысленно).
- `trusted_proxies: [cidr...]` — прокси, чей `X-Forwarded-For` учитывается для rate limit и `max_active_connections_per_ip`; в остальных случаях заголовок не доверяется.
- Токен — bearer-секрет; по plaintext он виден каждому узлу пути. SECURITY_MODEL §4.15 говорит об этом прямо.

## 12. Выдача и администрирование (по аутентифицированной SCRAM-сессии)

Операции — новые варианты `DbRequest`; доступны **только** SCRAM-сессии (токен-сессия не может выпускать или отзывать токены). Точные имена полей фиксируются в T5 и вносятся в эту таблицу без изменения семантики.

| Операция | Поля запроса | Ответ |
|---|---|---|
| `CreateApiToken` | `label`, `ttl_secs`, `mode`, `channels` (по умолчанию `both`), `databases`, `for_user?`, `hmac?` | `{ id, token, label, owner, mode, channels, databases, created_at_ns, expires_at_ns }`; поле `token` есть только здесь |
| `ListApiTokens` | `for_user?` | список без секретов, MAC и хешей: `id`, `label`, `owner`, `mode`, `channels`, `databases`, `created_at_ns`, `expires_at_ns`, `revoked_at_ns` |
| `RevokeApiToken` | `id`, `hmac?` | подтверждение |
| `RevokeUserTokens` | `user?`, `hmac?` | число отозванных |

Правила:

1. **Самообслуживание.** Без `for_user` любой аутентифицированный пользователь создаёт, смотрит и отзывает **свои** токены, без HMAC-подтверждения (токен только сужает права, которые у пользователя уже есть). Выключается `tokens.allow_self_service: false` (тогда выдают только superuser). Лимиты: `tokens.max_per_user` активных токенов (20), `tokens.max_ttl_days` (365), `default_ttl_days` (30), уникальность `label`.
2. **Выдача администратором.** Superuser с HMAC-подтверждением (тот же механизм «did-you-mean-it», что у `CreateScramUser`, ключ выводится из session id) создаёт токен `for_user`, смотрит токены любого пользователя, отзывает любой токен, вызывает `RevokeUserTokens`. Не-superuser с `for_user` → `permission_denied`. Так выпускаются токены сервисным аккаунтам. Каноническое кодирование для HMAC добавляется в T5.
3. Пользователь не видит чужие токены. Секрет возвращается один раз по защищённому (TLS или loopback) SCRAM-каналу.
4. Смена пароля, смена ролей, kick, restore повышают `tokens_invalid_before_ns` (§5): смена пароля отзывает все токены пользователя, их нужно выпустить заново.
5. Хранилище токенов **локально для узла**; репликации токенов нет. Создание и отзыв онлайн работают на `ReadWrite` узле; на read-only реплике токены управляются офлайн-CLI `shamir-server token ...` на остановленном каталоге данных (`create`, `list`, `revoke`, `revoke-user`, `revoke-all`; секрет печатается один раз в TTY или в `--out-file` с правами 0600, в лог не попадает).

## 13. Аудит, метрики, логи

- Аудит (HMAC-цепочка, IMPLEMENTATION_GUIDE §3): `api_token_created`, `api_token_revoked`, `api_token_user_revoked`, `api_token_auth_failed`. В записи только `id` токена, никогда секрет. Неудачи malformed и unknown-id **агрегируются по подсети и сэмплируются**, не пишутся одна к одной (иначе запись на диск без аутентификации).
- Метрики (IMPLEMENTATION_GUIDE §6): `shamir_token_auth_total{channel,result}`, `shamir_token_active_ws`, `shamir_token_listener_plaintext_remote`, `shamir_token_request_duration_seconds{endpoint}`. Меток с токеном или владельцем нет.
- Редактирование: заголовок `Authorization` и любая подстрока токена вырезаются в самой ранней точке; HTTP-слой не передаёт сырой заголовок в поля логов, строки ошибок и spans. Обязателен тест редактирования логов в стиле `shamir-connect/tests/audit/integration_log_redaction.rs`, покрывающий успешные и неуспешные запросы (IMPLEMENTATION_GUIDE §4.3).

## 14. Вне scope v1

mTLS, OAuth/OIDC, JWT, cookies, CORS, JSON-тела, resource-style REST и OpenAPI, браузерные WebSocket-клиенты (браузер не может задать `Authorization` для WebSocket), административные операции через токен, транзакции / курсоры / подписки по REST, репликация токенов между узлами, квоты на токен сверх rate limit, статистика использования токена, ротация `token_key`, токен как параметр подключения в `shamir-client` и `shamir-client-ts` (T9).

## 15. Test Checklist

Unit: парсер токена (все malformed-формы, огромный вход, не-UTF8, дубль `Authorization`, путаница разделителя, неканоничные завершающие биты); MAC и проверка (dummy-путь, `key_id` и `format_version` во входе MAC); переходы срока, отзыва, состояния владельца (пересозданный аккаунт не наследует токены, restore повышает порог); классификатор scope по каждому `DbRequest` и каждому `BatchOp` (исчерпывающий, deny-by-default); валидация конфигурации (plaintext + не-loopback, opt-in, обязательность `tls`, отказ самоподписанного); правила framing HTTP.

Интеграция (реальные сокеты, loopback, эфемерные порты, Windows и Linux):

- матрица {каналы 1-4} × {valid read, valid write, read-only токен пишет, база вне allowlist, неверный секрет, неизвестный id, просрочен, отозван после подключения (WS закрывается `4011`), владелец kicked, слишком большое тело, флуд заголовков, медленная голова, нет заголовка, токен в query игнорируется, два `Authorization`};
- подмена `sid`: токен-`sid` по SCRAM-соединению отвергается, SCRAM-`sid` по токен-соединению → `4010`; токен-сессии не видны в SCRAM `SessionStore`;
- права: superuser- и replicator-токены против каждого admin `DbRequest`, каждого admin `BatchOp`, `Repl`, `ChangePassword*`, `List::{Databases,Users}`, `AccessTree`;
- подписка: после отзыва WS перестаёт получать push без действий клиента;
- гонка: отзыв при запросах в полёте, ни один запрос не авторизован после возврата отзыва;
- учёт: 16 активных токен-сокетов владельца не мешают его SCRAM-входу;
- бюджеты: флуд телами до аутентификации не превышает общий бюджет, guard ответа держится до записи тела;
- выдача: пользователь видит и отзывает только свои токены; `for_user` не-superuser отвергается; superuser выдаёт другому только с HMAC; `max_per_user`, TTL; `allow_self_service: false`; токен-сессия не вызывает ни одну токен-операцию; смена пароля отзывает токены; секрет виден ровно один раз;
- каналы: `rest`-токен на WS → `403` без `101`, `ws`-токен на REST → `token_channel_denied`, `both` работает на обоих;
- тест редактирования логов; restore повышает порог; push подписок и курсоры по WS; `not_supported_over_rest`.

Без sleep, через lifecycle-швы.

## 16. См. также

- **TRANSPORT_WS.md** (§6 коды закрытия, SCRAM-эндпоинты), **ADMIN_UI_HOSTING.md** (статика admin SPA; REST-набросок заменён этим документом)
- **SECURITY_MODEL.md** (§3 восстановление, §4.15 bearer), **IMPLEMENTATION_GUIDE.md** (§1.2 `token_key`, §2.4 listener, §3.2, §4.1, §5.8-5.9, §6)
- Дизайн: `../../dev-artifacts/design/token-api-rest-ws.md`
