בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 0 — Quickstart: KV-хранилище за 5 минут

Цель: запустить сервер, завести аккаунт, создать таблицу в хранилище по
умолчанию и складывать/читать значения по ключу. Это всё — дальше работает
как обычное key-value хранилище. Ничего из верхних этажей пока не нужно.

## 1. Запусти сервер (один бинарник)

Готовых бинарников пока нет — собери из исходников:

```bash
cargo build --release -p shamir-server
PW='change-me-admin'
printf '%s\n' "$PW" | ./target/release/shamir-server --config db.ktav --bootstrap-password-stdin
```

* Сервер сам создаёт при первом старте: data-каталог, self-signed TLS-серт,
  durable хранилище `default` → репозиторий `main`, и bootstrap-админа
  `admin` с указанным паролем.
* Без `--bootstrap-password` сервер сгенерирует одноразовый токен, запишет
  его в `data_dir/bootstrap_token.txt` (или в `--bootstrap-token-path`) и
  выведет в лог (WARN) только **путь** к файлу. После первого входа токен
  перестаёт действовать — сразу заведи постоянного администратора.

TS-клиент подключается по WebSocket к browser-профилю
(`/shamir/v1/browser`, `tls_no_export`), поэтому минимальный `db.ktav`
для этого quickstart такой (комментарии в Ktav — только целой строкой, `##`;
на Windows подставь абсолютный путь с прямыми слэшами):

```ktav
data_dir: /tmp/shamir-quickstart

kdf_defaults: {
    memory_kb: 19456
    time: 2
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: ws
        addr: 127.0.0.1:7333
        path: /shamir/v1/browser
        profile: tls_no_export
        browser_origin_allowlist: [
            https://127.0.0.1
        ]
    }
]

tls: {
    cert_path: /tmp/shamir-quickstart/cert.pem
    key_path:  /tmp/shamir-quickstart/key.pem
}
```

Отсутствующие cert/key сервер сгенерирует сам (self-signed). Полный пример —
`deploy/server.example.ktav` и [этаж 7](./07-operations.md).

## 2. Подключись клиентом

Пакет `@shamir/client` пока не опубликован в npm — собери его из репозитория
и подключи локально (клиент сам проводит TLS 1.3 + SCRAM-Argon2id):

```bash
cd crates/shamir-client-ts && npm ci && npm run build
# в своём проекте:
npm i /path/to/shamir-db/crates/shamir-client-ts
```

```ts
import { connect, filter, write, ddl } from '@shamir/client';

const client = await connect({
  host: '127.0.0.1',
  port: 7333,
  username: 'admin',
  password: 'change-me-admin',
  tls: { rejectUnauthorized: false }, // только для локальной разработки: TS-клиент не проверяет идентичность сервера
  origin: 'https://127.0.0.1',
});

// Получаем bound-handle на нужную базу — дальше используем его
const db = client.db('default');
```

## 3. Создай таблицу в хранилище по умолчанию

Хранилище `default` и репозиторий `main` **уже существуют** (durable, созданы
на старте) — отдельно их заводить не нужно. Создаём только таблицу:

```ts
await db.run(ddl.createTable('kv', { repo: 'main' }));
```

## 4. PUT / GET по ключу

`write.upsert` — upsert по ключу (PUT); `db.query` — чтение (GET).

```ts
// PUT
await db.run(write.upsert('kv', { id: 'user:42' }, { id: 'user:42', name: 'Алиса', score: 7 }));

// GET (по фильтру на ключ)
const rows = await db.query('kv').where(filter.eq('id', 'user:42')).rows();
console.log(rows[0].name); // 'Алиса'

await client.close();
```

Всё. Это рабочее KV-хранилище, переживающее штатный рестарт (гарантии при аварии — ниже и на этаже 2).

## Что важно знать уже сейчас (дозированно)

* **Записи — это документы** (MessagePack, вложенные структуры), не только плоские
  пары. Ключ — это поле(я), по которым ты адресуешь запись.
* **`field` — это путь к полю.** Верхнее поле можно писать строкой —
  `filter.eq('id', …)` — это эквивалент пути из одного сегмента `['id']`. Для
  вложенного документа указываешь путь массивом: `['address', 'city']` →
  `record.address.city`. Строка и одноэлементный массив — одно и то же.
* **Durability по умолчанию — `buffered`**: подтверждение приходит быстро,
  данные доливаются на диск фоном (окно до ~500 мс) и гарантированно — на
  штатной остановке сервера. При `kill -9`, OOM или потере питания
  подтверждённые записи из этого окна могут пропасть. Для «терять нельзя»
  есть `durability: "synced"` — это [этаж 2](./02-durability.md).
* **Один аккаунт = bootstrap-админ.** Заводить обычных пользователей, группы
  и раздавать права — [этаж 4](./04-access.md). Пока админ может всё.

## Куда дальше

| Упёрся в… | Поднимайся на |
|---|---|
| «KV мало, нужны выборки/индексы» | [Этаж 1 — Запросы](./01-queries.md) |
| «эти данные терять нельзя» | [Этаж 2 — Durability](./02-durability.md) |
| «нужно несколько хранилищ» | [Этаж 3 — Хранилища](./03-storage.md) |
| «нужен бэкап» | [Этаж 7 — Эксплуатация](./07-operations.md) |
| «пользователей много, нужны права» | [Этаж 4 — Доступ](./04-access.md) |
