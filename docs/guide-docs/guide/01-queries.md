בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 1 — Запросы: фильтры, батчи, индексы

KV-хранилище с этажа 0 — уже работает. Но данные растут: нужны выборки
по полям, диапазоны, сортировка, индексы. Этот этаж — именно об этом.

Примеры ниже используют TS-клиент `@shamir/client`. Подключение — как
на этаже 0: `const db = client.db('default')`.

## 1. Фильтры: `where`

Каждый `db.query(table)` принимает необязательный `.where(filter.*)` —
фильтр, построенный через билдер. Вот основные операции.

### Сравнения

```ts
import { filter } from '@shamir/client';

// равно
const rows = await db.query('users').where(filter.eq('status', 'active')).rows();
```

| Метод | Смысл | Пример |
|---|---|---|
| `filter.eq(f, v)` | равно | `filter.eq('status', 'active')` |
| `filter.ne(f, v)` | не равно | `filter.ne('status', 'inactive')` |
| `filter.gt(f, v)` | больше | `filter.gt('age', 18)` |
| `filter.gte(f, v)` | больше или равно | `filter.gte('salary', 50000)` |
| `filter.lt(f, v)` | меньше | `filter.lt('age', 65)` |
| `filter.lte(f, v)` | меньше или равно | `filter.lte('score', 100)` |

Значение — строка, число, булев или `null` — Shamir сам приведёт тип.

### Проверка на null / существование поля

```ts
// поле отсутствует или равно null
db.query('users').where(filter.isNull('deleted_at'))

// поле присутствует и не null
db.query('users').where(filter.isNotNull('email'))
```

`filter.isNull` — поле отсутствует или равно `null`. `filter.isNotNull` — поле
присутствует и не null.

### `in_` / `notIn` — список значений

```ts
// поле входит в список
db.query('users').where(filter.in_('status', ['active', 'pending']))

// поле не входит в список
db.query('users').where(filter.notIn('status', ['banned']))
```

### `between` — диапазон включительно с обоих концов

```ts
db.query('users').where(filter.between('age', 25, 35))
```

Эквивалентно `age >= 25 AND age <= 35`. Обе границы включительны.

### `like` / `ilike` — шаблон строки

```ts
// LIKE
db.query('users').where(filter.like('email', '%@example.com'))

// Case-insensitive LIKE
db.query('users').where(filter.ilike('email', '%@EXAMPLE.COM'))
```

`%` — любая последовательность, `_` — один символ. `ilike` —
регистронезависимый вариант.

### `exists` / `notExists` — наличие поля у записи

```ts
db.query('users').where(filter.exists('email'))
db.query('users').where(filter.notExists('temp'))
```

### Комбинирование: `and`, `or`, `not`

```ts
// AND через andWhere
db.query('users')
  .where(filter.gte('age', 30))
  .andWhere(filter.lte('age', 50))

// AND + OR — вложенные комбинаторы
db.query('users').where(
  filter.and([
    filter.gte('age', 30),
    filter.lte('age', 50),
    filter.or([
      filter.eq('city', 'NYC'),
      filter.eq('city', 'LA'),
    ]),
  ])
)

// NOT
db.query('users').where(filter.not(filter.eq('status', 'deleted')))
```

Вложенность любая. `filter.and([...])` / `filter.or([...])` принимают массив,
`filter.not(f)` — один фильтр.

### Путь к полю: строка или массив

`field` принимает **два формата**:

* **Строка** — верхнее поле: `filter.eq('id', ...)` (частый случай).
* **Массив** — вложенный путь: `filter.eq(['address', 'city'], 'NY')` →
  `record.address.city`.

Для одноэлементного пути строка и массив — эквивалентны:
`'id'` === `['id']`.

```ts
db.query('users').where(filter.eq(['address', 'city'], 'NY'))
```

### Условные значения: `$cond`/`switch_case`

`filter.cond(ifFilter, then, orElse)` — тернарный оператор поверх
`FilterValue`: если `ifFilter` истинен для строки, значением становится
`then`, иначе — `orElse`. Ветки могут сами быть `$cond` (вложенность), что
даёт switch-case поверх произвольного числа условий:

```ts
import { filter } from '@shamir/client';

// tier по score: >=100 → vip, >=50 → regular, иначе newbie
db.query('users').where(
  filter.eq(
    filter.switchCase(
      [
        [filter.gte('score', 100), 'vip'],
        [filter.gte('score', 50), 'regular'],
      ],
      'newbie',
    ),
    'vip',
  ),
)

// эквивалентный ручной cond()
filter.cond(
  filter.gte('score', 100),
  'vip',
  filter.cond(filter.gte('score', 50), 'regular', 'newbie'),
)
```

`filter.switchCase(cases, defaultValue)` — сахар над `cond()`: список пар
`[условие, значение]` (первое истинное условие побеждает) плюс
`defaultValue`, свёрнутый в цепочку вложенных `$cond` справа налево — так
4+ ветки не требуют ручной вложенности скобок.

`filter.expr(op, args)` — арифметика/строки/логика/сравнение как значение
(`$expr`), а не как условие фильтра:

```ts
// age + score как значение (используется, например, внутри $cond-ветки
// или как аргумент filter.gte/eq)
filter.expr('add', [filter.ref('age'), filter.ref('score')])
```

**Где работает.** `$cond`/`$expr` вычисляются в `where`-фильтрах и в
аргументах `$fn` (`filter.fn(name, args)`). Они **сегодня не работают**
как значение `SET` в `update`/`upsert` — билдер типов `QueryValue`
(write-стороны) и `FilterValue` (read-стороны) разошлись, и `$cond`/`$expr`
не компонуются в write-значения. Это известное ограничение, отслеживается
как **#641** — не баг эвалюации, а незакрытый разрыв между двумя
системами типов; используйте `$cond`/`$expr` только в фильтрах и `$fn`
до его закрытия.

**Известный баг: `$query`-реф внутри `$cond`-ветки в батчах.** Если
ветка `$cond` (в `then`/`else`) ссылается на результат другого запроса
того же батча через `$query`/`filter.queryRef`, планировщик батча
(`BatchPlanner`) сегодня **не рекурсирует** в `$cond`/`$expr`/`$fn` при
извлечении зависимостей между шагами батча — зависимость на
`$query`-реф внутри `$cond`-ветки молча теряется (silent data loss), а
не падает с ошибкой. Отслеживается как
**#642**. **Не полагайтесь** на этот паттерн (условная ветка,
ссылающаяся на результат другого шага батча) до фикса #642 — либо
избегайте `$query`-рефов внутри `$cond`-веток в батчах, либо явно
проверяйте, что зависимость учтена планировщиком.

**Предупреждение о производительности.** `$cond`/`$expr` заметно
дороже плоского сравнения на каждой вычисляемой строке — резолвер
сегодня перекомпилирует условие фильтра (`compile_filter`) на каждую
эвалюацию, а не один раз на запрос. Измеренные числа (микробенч
`crates/shamir-engine/benches/cond_expr_eval.rs`, 1000 записей/итерация,
QUICK-калибровка):

```text
cond_expr_eval/flat_literal_1000          135249 iters      7231.95 ns/op
cond_expr_eval/cond_2branch_1000            4797 iters    213004.36 ns/op
cond_expr_eval/cond_nested_3level_1000      1335 iters    829010.94 ns/op
cond_expr_eval/expr_add_two_fields_1000      784 iters   1378120.92 ns/op
```

Т.е. 2-ветвевой `$cond` — примерно **29x** дороже плоского литерала,
3-уровневый вложенный `$cond` (switch-case) — примерно **115x**, а
`$expr` с арифметикой над двумя полями — примерно **190x**. Причина и
план устранения — **#643**. Используйте `$cond`/`$expr` для
нечастых/некритичных по латентности фильтров и вычисляемых значений, а
не как замену вторичному индексу или как значение в горячем цикле
per-row сравнений.

## 2. Мульти-запросные батчи

Несколько операций — один round-trip. Алиасы — ключи в `.add(alias, ...)`;
результаты вернутся в `resp.results[alias]`.

### Независимые операции

```ts
import { Query, write } from '@shamir/client';

const resp = await db.batch()
  .add('users',  db.query('users'))
  .add('orders', db.query('orders'))
  .add('seed',   write.insert('users', [{ name: 'Alice', score: 100 }]))
  .run();

resp.results.users.records;
resp.results.orders.records;
```

Чтения и записи вперемешку. Движок корректно упорядочит выполнение.

### Скрытие промежуточных результатов

`.add(alias, op, { returnResult: false })` — операция выполнится, но результат
не вернётся. Удобно для промежуточных записей:

```ts
const resp = await db.batch()
  .add('setup', write.insert('users', [{ name: 'Alice' }]), { returnResult: false })
  .add('read',  db.query('users'))
  .run();

resp.results.read.records; // ['Alice']
// resp.results.setup — отсутствует
```

### Зависимые запросы: `filter.queryRef`

Один запрос может ссылаться на результат другого через `filter.queryRef('@alias', path)`.

```ts
const resp = await db.batch()
  .add('user', db.query('users').where(filter.eq('name', 'alice')))
  .add('user_orders', db.query('orders').where(
    filter.eq('user_id', filter.queryRef('@user', '[0].id'))
  ))
  .run();

resp.results.user_orders.records; // заказы alice
resp.execution_plan;               // [['user'], ['user_orders']] — два этапа
```

`'[0].id'` — взять поле `id` из первой записи результата `user`. Планировщик
автоматически выстроит этапы: `user` → `user_orders`.

Краткая запись `path` поддерживает навигацию по результату:
`[0].field`, `[].field` (все элементы), `.count` / `.length`.

### `$query`/`queryRef` vs `after` — какое правило выбирать

Одной фразой: **`queryRef` создаёт ребро зависимости сам по себе** (движок
видит, что `user_orders` читает данные `user`, и automatически ставит его в
следующий этап) — **`after` нужен только там, где нет потока данных**, но
порядок всё равно важен. Классический случай — DDL перед DML: `create_table`
не возвращает ничего, что `insert` мог бы прочитать через `queryRef`, так что
единственный способ сказать «сначала создай таблицу» — явный `after`.

```ts
import { ddl, write, filter } from '@shamir/client';

const resp = await db.batch()
  .add('schema', ddl.createTable('users', { if_not_exists: true }))
  .add('seed', write.insert('users', [{ name: 'Alice', score: 100 }]), {
    after: ['schema'], // порядок only — insert не читает результат create_table
  })
  .add('scored', db.query('users').where(
    filter.gt('score', filter.queryRef('@seed', '[0].score'))
  ))
  .run();
```

Три этапа: `schema` → `seed` → `scored`. Ребро `seed` → `schema` пришло из
`after` (явный, `EdgeKind::Explicit`); ребро `scored` → `seed` пришло из
`queryRef` (`EdgeKind::DataFlow`) — движок обнаружил его сам, без всякого
`after`.

**Важно:** `after`-зависимость — это ТОЛЬКО порядок исполнения, не доступ к
данным. `after: ['seed']` без `queryRef` гарантирует, что `seed` выполнится
раньше, но ничего не даёт `scored` прочитать из результата `seed` — для этого
нужен `filter.queryRef`. Если нужно и то, и другое (порядок И данные) —
хватит одного `queryRef`: он сам одновременно и упорядочивает, и передаёт
данные. `after` в таком случае избыточен.

Для отладки `resp.execution_plan` (этапы) теперь сопровождается
`resp.edge_provenance` — картой `alias -> dep_alias -> "explicit" |
"data_flow" | "both"`, показывающей, каким именно механизмом появилось
каждое ребро зависимости. Поле опущено на wire, если зависимостей нет.

Батчи **не** исполняются параллельно потоками ОС внутри этапа — этапы
`execution_plan` остаются логической группировкой независимых запросов,
исполнение последовательное (см.
`docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md`).

### Условное исполнение: `when`/`switch`

Каждый `.add(alias, op, { when: <Filter> })` — операция выполняется, только
если `when` вычисляется в `true`. Без `when` (или `when` отсутствует) —
поведение как раньше, op выполняется всегда. Это НЕ то же самое, что
`$cond`/`switch_case` из §1 «Условные значения» — там условие выбирает
одно из двух ЗНАЧЕНИЙ внутри уже выполняющегося запроса; `when` решает,
выполнится ли САМА операция целиком (INSERT/UPDATE/DELETE/DDL/Call/
под-батч) — см. ADR
`docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`, Decision 1.

> ⚠️ **Известное ограничение (задача #651, НЕ исправлена).** `when`-условия
> на основе СРАВНЕНИЯ ПОЛЕЙ (`filter.eq`/`filter.gt`/`filter.gte`/
> `filter.lt`/`filter.lte`/`filter.ne`/`filter.fieldEq` и т.п.) СЕГОДНЯ НЕ
> РАБОТАЮТ КОРРЕКТНО — они компилируются против пустой синтетической записи
> через одноразовый scratch-интернер, который не может разрешить путь к
> полю НИ ДЛЯ ОДНОГО имени поля. В результате КАЖДОЕ такое сравнение
> схлопывается в фиксированный результат (`Gt`/`Gte`/`Lt`/`Lte`/`Eq`/`Ne` →
> всегда `false`, т.е. операция всегда пропускается) — **независимо от
> реальных данных**. Канонический сценарий «спиши со счёта, если баланс >=
> суммы» (`filter.gte('balance', filter.queryRef('@account', '[0].balance'))`)
> СЕГОДНЯ НЕ РАБОТАЕТ и молча даёт неверный результат без единой ошибки.
>
> **Единственный сегодня надёжный `when`-паттерн** — `filter.isNull(field)`
> / `filter.isNotNull(field)` против заведомо отсутствующего в схеме поля
> (проверка наличия `$query`-рефа как guard'а, не сравнение значений).
> **НЕ используйте `when` для полевых числовых/строковых сравнений до
> фикса #651.**

Базовый рабочий пример (`IsNull`/`IsNotNull`-guard, как в
`crates/shamir-client/tests/batch_when_e2e.rs`, Epic03/E) — вместо
недоступного сегодня «сравни баланс с суммой» ветвление проводится через
заведомо `true`/`false` guard, выбранный на стороне вызывающего кода:

```ts
import { write, filter } from '@shamir/client';

const resp = await db.batch()
  .add('debit', write.insert('ledger', [{ kind: 'debit', amount: 40 }]), {
    // "never_present_field" отсутствует в схеме -> IsNull всегда true
    when: filter.isNull('never_present_field'),
  })
  .add('decline', write.insert('ledger', [{ kind: 'decline', amount: 40 }]), {
    // IsNotNull того же отсутствующего поля -> всегда false -> decline пропускается
    when: filter.isNotNull('never_present_field'),
  })
  .run();

resp.results.debit.skipped;   // false — выполнился
resp.results.decline.skipped; // true  — пропущен
```

### `switchCase` — несколько взаимоисключающих веток

`batch.switchCase([...cases], defaultCase)` — синтаксический сахар:
генерирует N `QueryEntry` с комплементарными `when`-фильтрами (`case1` →
`AND(NOT case1, case2)` → ... → `default = NOT any_case`), гарантируя, что
выполнится РОВНО одна ветка. Сегодня практически применим только с
`isNull`/`isNotNull`-guard'ами (см. предупреждение выше). Сценарий с тремя
ветками ЖЕЛАЕМОГО (но пока НЕработающего до фикса #651) вида «выбери ветку
по сравнению значения» выглядел бы так — этот пример **намеренно
использует ` filter.eq`/`filter.gte`, СЕГОДНЯ НЕ РАБОТАЕТ**, приведён
только как целевой синтаксис после фикса #651:

```ts
// ⚠️ ЖЕЛАЕМЫЙ синтаксис — СЕГОДНЯ НЕ РАБОТАЕТ (см. #651 выше).
// filter.eq/filter.gte внутри `when` всегда схлопываются в false.
const handles = db.batch().switchCase(
  [
    { alias: 'gold',   op: write.insert('tier', [{ name: 'gold' }]),   condition: filter.gte('score', 90) },
    { alias: 'silver', op: write.insert('tier', [{ name: 'silver' }]), condition: filter.gte('score', 50) },
  ],
  { alias: 'bronze', op: write.insert('tier', [{ name: 'bronze' }]) },
);
```

Работающий сегодня трёхветочный пример использует `isNull`/`isNotNull`
guard'ы вместо сравнения значений — см. `switch_three_branches_executes_exactly_one_over_real_wire`
в `crates/shamir-client/tests/batch_when_e2e.rs`.

### Каскадный skip

Если алиас `A` пропущен (`when` дал `false`), и алиас `B` имеет
`DataFlow`/`Both`-зависимость от `A` (реальная ссылка через `queryRef`, или
`A` упомянут в `when` самого `B`) — `B` тоже пропускается автоматически,
статус `skipped`, БЕЗ ошибки «cannot resolve». Это соответствует обычной
if/else-интуиции: код внутри непройденной ветки (включая всё, что читает
её вывод) просто не выполняется.

**Чисто `after`-зависимость НЕ каскадируется** — `after` это порядок
исполнения, не доступ к данным (см. §«`$query`/`queryRef` vs `after`»
выше). Если `B after A` и `A` пропущен, `B` всё равно выполнится (если у
`B` нет собственного `when` и нет `DataFlow`/`Both`-ребра на `A`) — просто
без гарантии порядка, поскольку упорядочивать больше не с чем. Подробности
— ADR `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`,
Decision 2.

### `skipped`-статус в ответе

`resp.results[alias].skipped` — булево поле (по умолчанию `false`,
опущено на wire когда `false`). Три разных состояния алиаса не путать:

- **выполнен, есть данные**: `skipped: false`, `records: [...]`.
- **выполнен, 0 совпадений**: `skipped: false`, `records: []`,
  `stats` присутствует (например `records_scanned > 0`).
- **пропущен `when`-условием (свой или каскад)**: `skipped: true`,
  `records: []`, `stats`/`pagination`/`explain` отсутствуют. Алиас всё
  равно фигурирует в `resp.execution_plan`/`resp.edge_provenance`
  (статические артефакты плана, не зависят от runtime-решения о skip).
- **отфильтрован `returnResult: false` / `returnOnly`**: алиас вообще
  ОТСУТСТВУЕТ в `resp.results` — не путать с `skipped: true`. Это два
  независимых механизма: `when` решает, выполнится ли операция;
  `returnResult`/`returnOnly` решает, попадёт ли её результат в ответ.

## 3. Вторичные индексы

Без индекса каждый `where` сканирует таблицу целиком (O(n)). Индекс
ускоряет конкретные паттерны доступа. Создаётся через `ddl.createIndex`:

### Обычный (hash) индекс

Ускоряет `filter.eq`-поиск по полю:

```ts
import { ddl } from '@shamir/client';

await db.run(ddl.createIndex('name_idx', 'users', [['name']]));
```

После этого `filter.eq('name', 'Bob')` пойдёт через индекс — O(log n)
вместо полного скана.

### Уникальный индекс

То же, что обычный, плюс constraint: дубль по полю → ошибка.

```ts
await db.run(ddl.createIndex('email_idx', 'users', [['email']], { unique: true }));
```

### Sorted-индекс (для диапазонов, сортировки, MIN/MAX)

Ключевое отличие: **значения хранятся упорядоченно**. Это даёт O(log n)
для:

* диапазонов: `between`, `gt`/`gte`, `lt`/`lte` по одному полю;
* `orderByAsc` / `orderByDesc` + `limit` — первые/последние K без полной сортировки;
* `select.min` / `select.max` — O(1) из начала/конца индекса.

```ts
await db.run(ddl.createIndex('score_idx', 'users', [['score']], { sorted: true }));
```

> `unique: true` + `sorted: true` одновременно — **запрещено**.
> Sorted-индекс — только одно скалярное поле.

### Какой индекс для какого запроса

| Паттерн запроса | Нужный индекс |
|---|---|
| `filter.eq('email', …)` | обычный или `unique` по `email` |
| `filter.between('age', …)` | `sorted` по `age` |
| `orderByDesc('score')` + `limit(10)` | `sorted` по `score` |
| `select.min('score')`, `select.max('score')` | `sorted` по `score` |

## 4. Диапазоны, сортировка, лимиты

### BETWEEN + sorted-индекс

```ts
const rows = await db.query('users')
  .where(filter.between('age', 30, 35))
  .rows();
```

При наличии sorted-индекса по `age` — сканирование от 30 до 35,
O(log n + K), где K — число попавших записей.

### ORDER BY + LIMIT

```ts
const rows = await db.query('users')
  .orderByDesc('score')
  .limit(10)
  .offset(0)
  .rows();
```

* `orderByAsc(field)` / `orderByDesc(field)` — сортировка по одному полю.
* При sorted-индексе по `score` движок возьмёт 10 записей прямо
  из индекса — без сортировки всей таблицы.

### MIN / MAX

```ts
import { select } from '@shamir/client';

const qr = await db.query('users')
  .select([
    select.min('score', { alias: 'lo' }),
    select.max('score', { alias: 'hi' }),
  ])
  .ex();

const { lo, hi } = qr.records[0];
```

С sorted-индексом — O(1): берётся первая (min) и/или последняя (max)
запись.

### Постраничная навигация

```ts
// LimitOffset
const rows = await db.query('users')
  .limit(25)
  .offset(25) // вторая страница
  .rows();

// 1-based page helper
const rows2 = await db.query('users').page(2, 25).rows();

// С подсчётом общего числа записей
const qr = await db.query('users')
  .where(filter.gte('score', 50))
  .limit(25)
  .offset(0)
  .countTotal()
  .ex();

const total = qr.pagination?.total_count; // нужно для пагинации UI
```

`countTotal()` — вернуть общее число записей в ответе (нужно для
пагинации UI). Результат — в `qr.pagination.total_count`.

## Что важно знать уже сейчас (дозированно)

* **Индекс — overlay.** Данные живут в MVCC-сторе; индекс лишь
  ускоряет доступ. При crash он восстанавливается из WAL.
* **`Query.from` и `Query.withRepo`.** `db.query('users')` → таблица
  `users` в репозитории `main` (по умолчанию). Если нужна таблица
  из другого репозитория: `Query.withRepo('hot', 'sessions')`.
  Подробности — [этаж 3](./03-storage.md).
* **`select` по умолчанию — `SELECT *`.** Опускай его, пока не нужны
  агрегаты или проекции.
* **FTS, vector, functional-индексы** — отдельный зоопарк, им посвящён
  [этаж 6](./06-search.md). Здесь мы не касаемся `filter.fts`,
  `filter.vectorSimilarity` и `filter.computed`.

## Куда дальше

| Упёрся в… | Поднимайся на |
|---|---|
| «данные терять нельзя, нужны транзакции» | [Этаж 2 — Durability](./02-durability.md) |
| «несколько хранилищ, бэкап, миграции» | [Этаж 3 — Хранилища](./03-storage.md) |
| «нужен полнотекстовый или векторный поиск» | [Этаж 6 — Поиск](./06-search.md) |
