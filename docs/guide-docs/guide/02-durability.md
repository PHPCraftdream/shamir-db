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

`durability` задаётся на уровне батча через `.durability('synced')`:

```ts
import { write } from '@shamir/client';

const db = client.db('default');

await db.batch()
  .add('p', write.upsert('orders', { id: 'ORD-1' }, { id: 'ORD-1', amount: 9990, status: 'new' }))
  .durability('synced')
  .run();
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

`.transactional()` оборачивает весь батч в MVCC-транзакцию.
Все операции видят консистентный срез (snapshot) данных, коммит — атомарный.

### Snapshot Isolation (SI)

```ts
import { write } from '@shamir/client';

const db = client.db('default');

const resp = await db.batch()
  .add('ins', write.insert('items', [{ name: 'widget', qty: 10 }]))
  .transactional()
  .run();

resp.transaction?.status;          // 'committed'
resp.transaction?.tx_id;           // number
resp.transaction?.commit_version;  // number
```

`materialized: true` — проекции (data-store, индексы) успели
построиться до ответа. Если `false` — коммит durable, но какие-то
проекции отложены до recovery (данные появятся после рестарта).

### Кросс-таблицная атомарность

```ts
const resp = await db.batch()
  .add('ins_items', write.insert('items', [{ name: 'cross-item' }]))
  .add('ins_logs',  write.insert('logs',  [{ event: 'item_created' }]))
  .transactional()
  .run();

resp.transaction?.status; // 'committed'
```

Обе таблицы — один репозиторий → одна транзакция. Либо обе вставки
закоммичены, либо ни одна.

### Serializable Snapshot Isolation (SSI)

```ts
const resp = await db.batch()
  .add('ins', write.insert('items', [{ name: 'ssi-item' }]))
  .transactional('serializable')
  .run();

resp.transaction?.status; // 'committed'
```

`transactional()` — дефолт (SI). `transactional('serializable')` добавляет
валидацию read-set'а на коммите: если конкурентная транзакция успела
изменить данные, которые ты читал, — твой коммит abort с причиной
`"tx_conflict"`.

## 3. Нетранзакционные батчи — по-прежнему работают

Без `.transactional()` — автокоммит каждой операции. Поле `transaction`
в ответе отсутствует:

```ts
const resp = await db.batch()
  .add('ins', write.insert('items', [{ name: 'plain-item' }]))
  .run();

resp.transaction; // undefined
```

## 4. Деструктивные операции и HMAC-gate

`dropTable`, `dropDb`, `dropIndex`, `dropUser`, `dropRole` —
деструктивные операции. Они требуют **HMAC-тег** — не для аутентификации
(транспорт уже TLS 1.3 + SCRAM), а как подтверждение намерения:
«ты точно уверен?».

### Как работает

1. Ключ = `SHA256("shamir-db hmac key v1\0" || session_id)`.
2. Канонический вход — null-byte-separated байты, например:
   `drop_table\0<db>\0<repo>\0<table>`.
3. HMAC-SHA256 → hex-тег → поле `"hmac"` в операции.

TS-клиент скрывает механику — передай подключённый `client` как `signer`:

```ts
import { ddl, Batch } from '@shamir/client';

// ddl.dropTable(signer, dbInUse, repo, table)
const resp = await Batch.create('drop')
  .add('d', ddl.dropTable(client, 'default', 'main', 'old_table'))
  .execute(client, 'default');

// Или через bound-handle:
const qr = await db.dropTable('main', 'old_table');
qr.records[0]; // { dropped_table: 'old_table', existed: true }
```

### Три состояния

| Запрос | Результат |
|---|---|
| Без HMAC | ошибка `hmac_required` |
| HMAC неверный | ошибка `hmac_mismatch` |
| HMAC верный (через `ddl.*` / `db.dropTable(...)`) | успех, `records: [{ dropped_table: "...", existed: true }]` |

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
