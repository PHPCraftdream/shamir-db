בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 2 — Durability и транзакции

**Когда подниматься:** данные терять нельзя.

На этажах 0–1 мы не касались durability — всё работало из коробки.
По умолчанию записи подтверждаются быстро (попадают в in-memory буфер и
сбрасываются на диск фоном за миллисекунды). Штатная остановка сервера
доливает всё. Но при жёстком обрыве (потеря питания, `kill -9`) последние
~500 мс буфера могут не доехать.

Этот этаж — о том, как явно управлять надёжностью: от «synced-на-каждый
коммит» до транзакционных батчей с изоляцией.

## 1. Durability: `buffered` (дефолт) и `synced`

Поле `durability` на уровне батча:

```json
{
  "id": "safe-write",
  "durability": "synced",
  "queries": {
    "p": {
      "set": "orders",
      "key": { "id": "ORD-1" },
      "value": { "id": "ORD-1", "amount": 9990, "status": "new" }
    }
  }
}
```

|| Уровень | Что происходит | Переживает |
|---|---|---|---|
| `"buffered"` (или без поля) | ack после попадания в MemBuffer | graceful restart; теряет ≤окно (~500 мс) при жёстком обрыве |
| `"synced"` | ack **после fsync** WAL-маркера | потерю питания, `kill -9`, «конец света» |

`synced` фсинкает только маленький последовательный WAL-маркер — **не**
дорогую разбросанную материализацию данных. Поэтому он быстр.

### Когда что

* **`buffered`** (дефолт) — подавляющее большинство данных. Сервер на Rust:
  нет segfault, нет UB-вылетов; паника разматывает стек и прогоняет `Drop`.
  На ИБП почти никогда не гаснет моментально.
* **`synced`** — деньги, заказы, критичные ключи. Осознанный opt-in: «терять
  нельзя даже при потере питания».

## 2. Транзакционные батчи

Флаг `transactional: true` оборачивает весь батч в MVCC-транзакцию.
Все операции видят консистентный срез (snapshot) данных, коммит — атомарный.

### Snapshot Isolation (SI)

```json
{
  "id": "tx-si",
  "transactional": true,
  "queries": {
    "ins": {
      "insert_into": "items",
      "values": [{ "name": "widget", "qty": 10 }]
    }
  }
}
```

Ответ содержит блок `transaction`:

```json
{
  "id": "tx-si",
  "results": {
    "ins": { "records": [{ "name": "widget", "qty": 10 }] }
  },
  "execution_plan": [["ins"]],
  "execution_time_us": 340,
  "transaction": {
    "tx_id": 1,
    "status": "committed",
    "snapshot_version": 100,
    "commit_version": 101,
    "materialized": true
  }
}
```

`materialized: true` — проекции (data-store, индексы) успели
построиться до ответа. Если `false` — коммит durable, но какие-то
проекции отложены до recovery (данные появятся после рестарта).

### Кросс-таблицная атомарность

```json
{
  "id": "tx-cross",
  "transactional": true,
  "queries": {
    "ins_items": {
      "insert_into": "items",
      "values": [{ "name": "cross-item" }]
    },
    "ins_logs": {
      "insert_into": "logs",
      "values": [{ "event": "item_created" }]
    }
  }
}
```

Обе таблицы — один репозиторий → одна транзакция. Либо обе вставки
закоммичены, либо ни одна.

### Serializable Snapshot Isolation (SSI)

```json
{
  "id": "tx-ssi",
  "transactional": true,
  "isolation": "serializable",
  "queries": {
    "ins": {
      "insert_into": "items",
      "values": [{ "name": "ssi-item" }]
    }
  }
}
```

`"isolation": "snapshot"` — дефолт (SI). `"serializable"` добавляет
валидацию read-set'а на коммите: если конкурентная транзакция успела
изменить данные, которые ты читал, — твой коммит abort с причиной
`"tx_conflict"`.

## 3. Нетранзакционные батчи — по-прежнему работают

Без `transactional` (или `transactional: false`) — автокоммит каждой
операции. Блок `transaction` в ответе отсутствует:

```json
{
  "id": "non-tx",
  "queries": {
    "ins": {
      "insert_into": "items",
      "values": [{ "name": "plain-item" }]
    }
  }
}
```

## 4. Деструктивные операции и HMAC-gate

`drop_table`, `drop_db`, `drop_index`, `drop_user`, `drop_role` —
деструктивные операции. Они требуют **HMAC-тег** — не для аутентификации
(транспорт уже TLS 1.3 + SCRAM), а как подтверждение намерения:
«ты точно уверен?».

### Как работает

1. Ключ = `SHA256("shamir-db hmac key v1\0" || session_id)`.
2. Канонический вход — null-byte-separated байты, например:
   `drop_table\0<db>\0<repo>\0<table>`.
3. HMAC-SHA256 → hex-тег → поле `"hmac"` в операции.

Клиентская библиотека скрывает механику — вызови helpers:

```javascript
// JS (napi helper)
const hmac = require('./helpers/hmac');

await client.execute(db, {
  id: 1,
  queries: { d: hmac.drop_table_op(client, db, 'main', 'items') },
});
```

### Три состояния

|| Запрос | Результат |
|---|---|---|
| HMAC отсутствует | `{ "drop_table": "items", "repo": "main" }` | ошибка `hmac_required` |
| HMAC неверный | `{ "drop_table": "items", "repo": "main", "hmac": "aa…aa" }` | ошибка `hmac_mismatch` |
| HMAC верный | сгенерирован helper'ом | успех, `records: [{ dropped_table: "items", existed: true }]` |

**Без hmac-тега** работают: все read-операции, `create_table`, `create_db`,
`create_index`. HMAC — только для `drop_*`.

### Тег привязан к цели

HMAC вычисляется по конкретному db + repo + table + index. Подставить
тег от таблицы A к таблице B → `hmac_mismatch`. Подделать без `session_id`
— невозможно.

## 5. Graceful shutdown

Сервер умеет штатно останавливаться:

1. Прекратить приём новых соединений.
2. Дождаться in-flight запросов (в пределах дедлайна).
3. Долить все MemBuffer'ы на диск → fsync.
4. Освободить файловые блокировки.

После shutdown любой подтверждённый (`acked`) write — durable. Файловые
блокировки сняты — можно сразу перезапустить.

<!-- TODO: verify shutdown deadline config and signal names on each OS — see GRACEFUL_SHUTDOWN.md -->

## Что важно знать уже сейчас (дозированно)

* **Durability ≠ материализация.** `synced` гарантированно пишет WAL-маркер.
  Вторичные индексы, HNSW-граф — проекции, восстанавливаемые из WAL при
  crash recovery. См. сквозной принцип в README.
* **Транзакция — один репозиторий.** Кросс-репо 2PC не поддерживается:
  если батч ссылается на таблицы из разных репо → ошибка
  `tx_cross_repo_not_supported`. Разноси такие операции в отдельные
  батчи.
* **`buffered` — честен при нормальной эксплуатации.** Rust-процесс
  не падает от типовых ошибок; graceful drain доливает буферы; ИБП
  — стандарт продакшена. `synced` — осознанный opt-in для критичных
  данных.

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «несколько хранилищ, бэкап, миграции» | [Этаж 3 — Хранилища](./03-storage.md) |
| «пользователей много, нужны права» | [Этаж 4 — Доступ](./04-access.md) |
