בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 0 — Quickstart: KV-хранилище за 5 минут

Цель: запустить сервер, завести аккаунт, создать таблицу в хранилище по
умолчанию и складывать/читать значения по ключу. Это всё — дальше работает
как обычное key-value хранилище. Ничего из верхних этажей пока не нужно.

## 1. Запусти сервер (один бинарник)

```bash
shamir-server --config db.ktav --bootstrap-password "change-me-admin"
```

* Сервер сам создаёт при первом старте: data-каталог, self-signed TLS-серт,
  durable хранилище `default` → репозиторий `main`, и bootstrap-админа
  `admin` с указанным паролем.
* Без `--bootstrap-password` сервер сгенерирует случайный токен, **один раз**
  напечатает его в лог (WARN) и положит в `data_dir/bootstrap_token.txt`.

Минимальный `db.ktav` указывает лишь `data_dir`, один TCP-листенер и пути к
серту/ключу — полный пример конфига см. на [этаже 7](./07-operations.md).

## 2. Подключись клиентом

Высокоуровневый клиент сам проводит TLS 1.3 + SCRAM-Argon2id:

```rust
use shamir_client::{BatchRequest, Client, ConnectOptions};
use zeroize::Zeroizing;

let client = Client::connect(ConnectOptions {
    addr: "127.0.0.1:7000".parse()?,
    server_name: "localhost".into(),       // совпадает с self-signed сертом
    username: "admin".into(),
    password: Zeroizing::new(b"change-me-admin".to_vec()),
    accept_new_host: true,                  // trust-on-first-use; пин сохрани для следующих коннектов
    trusted_pin: None,
})
.await?;
```

## 3. Создай таблицу в хранилище по умолчанию

Хранилище `default` и репозиторий `main` **уже существуют** (durable, созданы
на старте) — отдельно их заводить не нужно. Создаём только таблицу:

```rust
let mk: BatchRequest = serde_json::from_value(json!({
    "id": "mk",
    "queries": {
        "t": { "create_table": "kv", "repo": "main" }
    }
}))?;
client.execute("default", mk).await?;
```

## 4. PUT / GET по ключу

Запросы шлются **батчами** (можно несколько за один round-trip). `set` —
upsert по ключу (PUT); `from` — чтение (GET).

```rust
// PUT
let put: BatchRequest = serde_json::from_value(json!({
    "id": "put",
    "queries": {
        "p": {
            "set": "kv",
            "key":   { "id": "user:42" },
            "value": { "id": "user:42", "name": "Алиса", "score": 7 }
        }
    }
}))?;
client.execute("default", put).await?;

// GET (по фильтру на ключ)
let get: BatchRequest = serde_json::from_value(json!({
    "id": "get",
    "queries": {
        "g": {
            "from": "kv",
            "where": { "op": "eq", "field": "id", "value": "user:42" }
        }
    }
}))?;
let resp = client.execute("default", get).await?;
let rows = &resp.results["g"].records;
assert_eq!(rows[0]["name"], "Алиса");
```

Всё. Это рабочее, durable (переживает рестарт) KV-хранилище.

## Что важно знать уже сейчас (дозированно)

* **Записи — это документы** (вложенный JSON/MessagePack), не только плоские
  пары. Ключ — это поле(я), по которым ты адресуешь запись.
* **`field` — это путь к полю.** Верхнее поле можно писать строкой —
  `"field": "id"` — это эквивалент пути из одного сегмента `["id"]`. Для
  вложенного документа указываешь путь массивом: `["address", "city"]` →
  `record.address.city`. Строка и одноэлементный массив — одно и то же.
* **Durability по умолчанию — `buffered`**: подтверждение приходит быстро,
  данные доливаются на диск фоном за миллисекунды и гарантированно — на
  штатной остановке сервера. Для «терять нельзя даже при потере питания»
  есть `durability: "synced"` — это [этаж 2](./02-durability.md).
* **Один аккаунт = bootstrap-админ.** Заводить обычных пользователей, группы
  и раздавать права — [этаж 4](./04-access.md). Пока админ может всё.

## Куда дальше

| Упёрся в… | Поднимайся на |
|---|---|
| «KV мало, нужны выборки/индексы» | [Этаж 1 — Запросы](./01-queries.md) |
| «эти данные терять нельзя» | [Этаж 2 — Durability](./02-durability.md) |
| «нужно несколько хранилищ / бэкап» | [Этаж 3 — Хранилища](./03-storage.md) |
| «пользователей много, нужны права» | [Этаж 4 — Доступ](./04-access.md) |
