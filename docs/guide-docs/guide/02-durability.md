בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 2 — Durability и транзакции

## Migration note

For query protocol v4 changes and required client/server upgrades, see [PR1c migration](./PR1C-MIGRATION.md).

На этажах 0–1 мы не касались durability — всё работало из коробки.
По умолчанию `buffered` подтверждает запись после локального write в WAL
(страничный кеш ОС), а не после одной лишь записи в MemBuffer. Для файлового
WAL это переживает падение процесса (`kill -9`), но не гарантирует сохранение
хвоста при потере питания. Фоновый fsync имеет номинальный таймер 250 мс —
**не жёсткий максимум**: задержки планировщика/накопителя и ошибки storage
могут увеличить окно. Штатная остановка пытается долить данные; проверяйте
результат shutdown и ошибки хранения.

Этот этаж — о том, как явно управлять надёжностью: от «synced-на-каждый
коммит» до транзакционных батчей с изоляцией.

## 1. Durability: `buffered` (дефолт) и `synced`

`durability.wal_commit` задаёт серверный default для всех открываемых и
создаваемых репозиториев. `synced` на сервере нельзя ослабить клиентским
`buffered`. Батч может усилить durability через `.durability('synced')`:

```ts
import { write } from '@shamir/client';

const db = client.db('default');

await db.batch()
  .add('p', write.upsert('orders', { id: 'ORD-1' }, { id: 'ORD-1', amount: 9990, status: 'new' }))
  .durability('synced')
  .run();
```

| Уровень | Что происходит на файловом WAL | Переживает |
|---|---|---|
| `buffered` (default) | ack после локального WAL write | падение процесса; хвост до успешного fsync может исчезнуть при потере питания |
| `synced` | ack после coalesced WAL fsync | падение процесса и потерю питания, если storage честно выполняет fsync |

`synced` синхронизирует WAL с байтами записи, а не требует синхронной
материализации всех проекций. Конкурентные запросы объединяют fsync
(coalesced group commit); задержка зависит от накопителя.

### Когда что

* **`buffered`** (дефолт) — допустима потеря ещё не fsync-нутого хвоста
  при потере питания. Номинальные 250 мс фонового таймера не задают
  максимальный RPO; storage errors могут задержать успешную синхронизацию.
* **`synced`** — деньги, заказы, критичные ключи. Осознанный opt-in: «терять
  нельзя даже при потере питания».

### Серверная политика (PR1)

```ktav
durability: {
    wal_commit: buffered
    wal_retention_max_bytes: 1073741824
    on_wal_retention_exceeded: refuse_writes
    replication_serve: durable
}
```

Это defaults при отсутствующем блоке/полях. Неизвестные ключи ВНУТРИ блока
и неверные значения enum отвергаются при parsing. Общего детектора опечаток
имени верхнеуровневого блока нет. `wal_commit`: `buffered|synced`;
`on_wal_retention_exceeded`: `refuse_writes|declare_gap`. Retention budget
(1 GiB default) должен быть не меньше **2 × effective WAL segment size**,
включая `SHAMIR_WAL_SEGMENT_MAX_BYTES`. Проверка выполняется до init.
**До PR2 retention только parsed/validated: ни cap, ни overflow action ещё
не enforced.** `declare_gap` выдаёт предупреждение при boot, не включает
эвикцию. `replication_serve: completed_reseed` отвергается с причиной:
`completed_reseed is not available yet (planned: epoch bump + universal fencing)`.

На in-memory/hybrid WAL нет persistence: commit tier игнорируется,
предупреждение выдаётся один раз при открытии WAL. `synced` не превращает
такие репозитории в долговечные.

### Durability и replication lag

`R` — **непрерывный префикс recoverability**, а не максимальная версия,
когда-либо fsync-нутая. Durable Pull отдаёт не выше
`min(confirmed, W, R)` (confirmed — подтверждение journal, W — settled bound
коммитов в чтении). При нехватке R pull создаёт coalesced sync demand и
ограниченно ждёт продвижения recoverability; при ошибке/таймауте остаётся
старый безопасный bound. Это не обязательное ожидание следующего 250 мс tick
и не ожидание follower на commit path. Локальный `buffered` ack может быть
выше R: хвост ещё нельзя безопасно отдать follower с durable bookmark.

`index_visibility` выбирает видимость индексов: `sync` ждёт синхронного обновления, `async` допускает отложенную видимость. Эти варианты отдельны от durability: запрос durability принимает только `buffered` или `synced`. Старые примеры с предыдущий параметр асинхронного обновления индексов следует мигрировать на `index_visibility: async`; см. [заметку PR1c](./PR1C-MIGRATION.md).


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
`"tx_conflict"`. Ограничение: phantom-вставки конкурентных транзакций в
диапазон потокового скана не детектируются
(см. [`KNOWN_LIMITATIONS.md`](../KNOWN_LIMITATIONS.md) §1).

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

`dropTable`, `dropDb`, `dropRepo`, `dropIndex`, `dropUser`, `dropGroup` —
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

**Без hmac-тега** работают: read-операции, `create_table`, `create_db`,
`create_repo`, `create_index`. HMAC требуется для всех `drop_*`, а также для
admin-операций доступа (`chmod`/`chown`/`chgrp`, группы, `grantRole`/
`revokeRole`, `createUser`) и миграций; в TS-билдерах это первый аргумент `client`.

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

После **успешного clean shutdown** файлового репозитория подтверждённые
записи долиты; unclean shutdown или ошибки storage не дают этой гарантии.
In-memory/hybrid данные не сохраняются при перезапуске.

<!-- TODO: verify shutdown deadline config and signal names on each OS — see GRACEFUL_SHUTDOWN.md -->

## Что важно знать уже сейчас (дозированно)

* **Durability ≠ материализация.** На файловом WAL `synced` ждёт успешного fsync.
  Вторичные индексы, HNSW-граф — проекции, восстанавливаемые из WAL при
  crash recovery. См. сквозной принцип в README.
* **Транзакция — один репозиторий.** Кросс-репо 2PC не поддерживается:
  если батч ссылается на таблицы из разных репо → ошибка
  `tx_cross_repo_not_supported`. Разноси такие операции в отдельные
  батчи.
* **`buffered`: power-loss окно не имеет жёсткого максимума 250 мс.**
  Это номинальный таймер; ошибки storage и задержки могут растянуть окно.
  Для критичных данных выбирайте файловое хранилище и `synced`.

## Куда дальше

|| Упёрся в… | Поднимайся на |
|---|---|---|
| «несколько хранилищ, бэкап, миграции» | [Этаж 3 — Хранилища](./03-storage.md) |
| «пользователей много, нужны права» | [Этаж 4 — Доступ](./04-access.md) |
