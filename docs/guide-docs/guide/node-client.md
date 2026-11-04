בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Node.js-клиент (`shamir-client`, napi)

Как подключиться к серверу из Node.js нативным биндингом `shamir-client` (крейт `crates/shamir-client-node`). Какие способы входа и транспорты вообще есть — в [`client-matrix.md`](./client-matrix.md); про токены и вход по паролю на стороне сервера — в [`token-api.md`](./token-api.md).

Запросы собираются билдерами из `@shamir/client` (`ddl`, `write`, `Query`, `filter`), а не руками: клиент принимает готовый объект батча.

```js
const { ddl, write, Query, filter } = require('@shamir/client');

const batch = (id, queries) => ({ id, queries });
```

## Сессия (SCRAM): TCP+TLS

```js
const { ShamirClient } = require('shamir-client');

const client = await ShamirClient.connect({
  host: '127.0.0.1',
  port: 7331,
  serverName: 'localhost',
  username: 'alice',
  password: 'secret',
  acceptNewHost: true, // TOFU: запомнить пин сервера при первом подключении
});

await client.execute('default', batch('mk', { mk: ddl.createDb('shop') }));
await client.execute('shop', batch('t', {
  mr: ddl.createRepo('main'),
  tb: ddl.createTable('items', { repo: 'main' }),
}));
await client.execute('shop', batch('w', {
  w: write.upsert('items', { id: 'a' }, { id: 'a', qty: 1 }),
}));
const resp = await client.execute('shop', batch('r', {
  r: Query.from('items').where(filter.eq('id', 'a')).build(),
}));
console.log(resp.results.r.records);

await client.close();
```

`client.authKind()` возвращает `'scram'` для сессии и `'token'` для токенной сессии.

## WebSocket, plain loopback, IPC

Параметры у всех вариантов такие же, как у `connect`; различается только слушатель, к которому идёт подключение.

```js
// Сессия по WebSocket (слушатель ws, profile: tls_exporter).
const ws = await ShamirClient.connectWs({
  host: '127.0.0.1', port: 7333, serverName: 'localhost',
  username: 'alice', password: 'secret', acceptNewHost: true,
});

// TCP без TLS: только loopback.
const plain = await ShamirClient.connectPlain({
  host: '127.0.0.1', port: 7332,
  username: 'alice', password: 'secret', acceptNewHost: true,
});

// Локальный IPC: путь к Unix socket или имя Named Pipe.
const local = await ShamirClient.connectLocal({
  addr: process.platform === 'win32' ? '\\\\.\\pipe\\shamir' : '/run/shamir.sock',
  username: 'alice', password: 'secret', acceptNewHost: true,
});
```

- `connectPlain` **только для loopback**: для любого другого адреса он отвечает ошибкой, в тексте которой есть `loopback`, и не открывает сокет.
- `connectWs` и `connectPlain` подключаются к **первому** адресу, в который разрешилось имя хоста; перебора адресов нет.

## Resume по билету

После обычного входа клиент хранит билет возобновления. Он позволяет открыть новую сессию без пароля. Билет одноразовый — после использования сервер выдаёт следующий, поэтому для каждого resume берите билет свежей сессии.

```js
const first = await ShamirClient.connect({ /* ... как выше ... */ });
const ticket = first.resumptionTicket();      // Buffer | null
const trustedPin = first.serverPubKeyPin();   // Buffer, 32 байта

// TCP+TLS
const a = await ShamirClient.resume({
  host: '127.0.0.1', port: 7331, serverName: 'localhost', ticket, trustedPin,
});

// WebSocket (билет, полученный по TCP или по WS, подходит к обоим)
const b = await ShamirClient.resumeWs({
  host: '127.0.0.1', port: 7333, serverName: 'localhost', ticket, trustedPin,
});
```

Неверный пин или испорченный билет — ошибка; текст ошибки не содержит ни билета, ни пина.

## Токены

Токен выпускает суперпользователь из сессии SCRAM:

```js
const issued = await client.createApiToken({
  label: 'ci-bot',
  ttlSecs: 3600,
  mode: 'read_write',   // или 'read_only'
  channels: 'both',     // 'rest' | 'ws' | 'both'
});
// issued.token — строка вида shk1.<id>.<secret>; показывается один раз
```

### REST: `ShamirTokenHttpClient`

```js
const { ShamirTokenHttpClient } = require('shamir-client');

const http = await ShamirTokenHttpClient.connect({
  host: 'db.example.com', port: 7340,
  serverName: 'db.example.com',
  caPem: fs.readFileSync('ca.pem', 'utf8'), // доверять этому сертификату
  token: issued.token,
});
await http.ping();
await http.execute('shop', batch('r', { r: Query.from('items').build() }));
await http.close();
```

### WebSocket: `connectTokenWs`

```js
const tokenWs = await ShamirClient.connectTokenWs({
  host: 'db.example.com', port: 7340, serverName: 'db.example.com',
  caPem, token: issued.token,
});
console.log(tokenWs.authKind()); // 'token'
```

Токенная сессия **не умеет админские вызовы**: `createScramUser`, `createApiToken`, `repl`, `setReplicator` завершаются `ShamirDbError` с кодом `token_request_denied`. Токен с `channels: 'rest'` по WebSocket не принимается. `serverPubKeyPin()` у такой сессии — 32 нулевых байта: пина сервера в токенном обмене нет.

### Опции адреса: `caPem` и `plainLoopback`

Они общие для `ShamirTokenHttpClient.connect`, `connectTokenWs` и `login`:

- `caPem` — PEM-строка сертификата (или CA), которому доверять при TLS.
- `plainLoopback: true` — подключиться без TLS. Разрешено только для loopback-адреса; для остальных запрос отклоняется.

## Вход по паролю → токен

```js
const { login } = require('shamir-client');

const t = await login({
  host: '127.0.0.1', port: 7340, serverName: 'localhost', caPem,
  username: 'alice', password: 'secret',
  label: 'laptop', mode: 'read_only', channels: 'rest',
});
// t.token, t.id (Buffer, 16 байт), t.owner, t.expiresAtNs, ...
const http = await ShamirTokenHttpClient.connect({
  host: '127.0.0.1', port: 7340, serverName: 'localhost', caPem, token: t.token,
});
```

Сервер запрещает вход по паролю для суперпользователя и для аккаунтов репликатора (`login_account_denied`); `read_write` токен выдаётся, только если сервер разрешил его в настройках токенов. Слушатель должен включать вход: `login: { enabled: true }`.

## Ошибки и секреты

- Ошибки уровня базы приходят как `ShamirDbError` с полем `.code`; ошибки транспорта и входа — обычный `Error`.
- Тексты ошибок не содержат пароль, токен, билет и пин.
- **Строку или `Buffer` в JavaScript нельзя гарантированно затереть**: пароль, токен и билет остаются в памяти процесса, пока их не соберёт сборщик мусора, и могут попасть в дамп. Держите их как можно короче и не логируйте. Это ограничение среды JavaScript, а не биндинга.
