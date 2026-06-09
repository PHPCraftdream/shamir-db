בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 6 — Поиск: полнотекстовый, векторный / embeddings

**Когда подниматься:** нужен поиск / AI-фичи.

До этого этажа запросы к ShamirDB — точные совпадения (`filter.eq`), диапазоны
(`filter.between`), шаблоны (`filter.like`). Но реальные продукты требуют:
«найди документы со словом *rust* и *world*» или «найди записи, похожие
на этот вектор». Этот этаж — про два специализированных вида поиска:
**FTS** (full-text search) и **vector similarity** (ANN через HNSW).

## 1. FTS-индекс: создание

FTS-индекс строится по текстовому полю и ускоряет поиск по словам
(токенам). Создаётся через `ddl.createIndex` с `index_type: 'fts'`:

```ts
import { ddl, Batch } from '@shamir/client';

await Batch.create('mk-fts')
  .add('idx', ddl.createIndex('body_fts', 'posts', [['body']], {
    index_type:    'fts',
    fts_tokenizer: 'whitespace',
  }))
  .execute(client, 'my_app');
```

Ключевые опции:

|| Поле | Описание | Значения |
||---|---|---|
|| `index_type` | тип индекса | `'fts'` |
|| `fts_tokenizer` | алгоритм токенизации | `'whitespace'`, `'unicode'`, `'stemmed_en'`, `'ngram3'`, … |
|| `fts_language` | подсказка языка (reserved) | строка, пока не влияет на `whitespace` |

### Токенайзеры

|| Токенайзер | Значение | Как работает |
||---|---|---|
| **Whitespace** | `'whitespace'` | Разбивает по пробелам → lowercase. Быстро, просто. |
| **Unicode** | `'unicode'` | Unicode-aware границы слов (буквы/цифры vs punctuation). |
| **Ngram** | `'ngram2'` … `'ngram9'` | Скользящее окно по символам. Для CJK-языков и substring-search. |
| **Full (stemming)** | `'stemmed_en'`, `'stemmed_ru'`, … | Полный pipeline: whitespace → lowercase → **stopwords** → **Snowball stemming**. 17 языков. |

Пример stemmed-индекса:

```ts
await Batch.create('mk-stemmed')
  .add('idx', ddl.createIndex('body_stemmed', 'articles', [['content']], {
    index_type:    'fts',
    fts_tokenizer: 'stemmed_en',
  }))
  .execute(client, 'my_app');
```

При stemmed-токенизации:
* слово `"running"` → stem `"run"` → токен хешируется → posting entry.
* стоп-слова (`"the"`, `"a"`, `"is"`, …) удаляются.
* запрос `"was running"` → токены `["run"]` (was — стоп-слово).

### Поддерживаемые языки стемминга

English, Russian, Arabic, Danish, Dutch, Finnish, French, German,
Greek, Hungarian, Italian, Norwegian, Portuguese, Romanian, Spanish,
Swedish, Tamil, Turkish. Стоп-словые списки кураторованы для EN и RU.

## 2. FTS-запрос

### AND-режим (все токены должны быть)

```ts
import { filter } from '@shamir/client';

const rows = await db.query('posts')
  .where(filter.fts('body', 'hello world', 'and'))
  .rows();
```

Строка `"hello world"` токенизируется тем же токенайзером, что и индекс,
и движок ищет записи, содержащие **оба** токена.

Ответ с BM25-ранжированием (FTS-индекс использует `index2_ranked`):

```json
{
  "results": {
    "r": {
      "records": [
        { "body": "hello rust world", "$score": 1.82 }
      ],
      "stats": { "index_used": "index2_ranked" }
    }
  }
}
```

`$score` — BM25-оценка (Okapi BM25, k1=1.2, b=0.75). Выше = релевантнее.

### OR-режим (хотя бы один токен)

```ts
const rows = await db.query('posts')
  .where(filter.fts('body', 'apple banana', 'or'))
  .rows();
```

Вернёт записи с `"apple"` ИЛИ `"banana"`.

### Brute-force fallback

Если FTS-индекс **не** создан, `filter.fts` всё равно работает — движок
сканирует таблицу и проверяет каждую запись. Это медленно (O(n)), но
работает для прототипирования. При появлении индекса — автоматически
переключается на индексный путь.

## 3. BM25-ранжирование

FTS-индекс использует алгоритм **Okapi BM25** для ранжирования:

* **TF** (term frequency) — сколько раз токен встречается в документе.
* **IDF** (inverse document frequency) — редкость токена по корпусу.
* **Doc length normalisation** — более короткие документы получают бонус.

Статистика (`doc_count`, `sum_doc_len`) обновляется атомарно при каждой
записи. Результаты отсортированы по убыванию `$score`.

## 4. Vector-индекс: создание

Для поиска «ближайших соседей» по embeddings — HNSW-индекс:

```ts
await Batch.create('mk-vec')
  .add('idx', ddl.createIndex('vec_idx', 'docs', [['embedding']], {
    index_type:    'vector',
    vector_dim:    128,
    vector_metric: 'cosine',
  }))
  .execute(client, 'my_app');
```

|| Поле | Описание | Значения |
||---|---|---|
|| `index_type` | тип индекса | `'vector'` |
|| `vector_dim` | размерность вектора | число (default: 384) |
|| `vector_metric` | метрика расстояния | `'cosine'` (default), `'l2'`, `'dot'` |

### Метрики

|| Метрика | Формула | Когда |
||---|---|---|
|| `cosine` | косинусное сходство | embeddings (NLP, vision) — наиболее частый |
|| `l2` | евклидово расстояние | пространственные координаты, физические величины |
|| `dot` | скалярное произведение | преднормализованные векторы |

Вычисления используют **SIMD**: AVX-512F (16-lane FMA), AVX2+FMA
(8-lane), NEON (aarch64), scalar fallback.

## 5. Vector-запрос: top-k ближайших

```ts
import { filter } from '@shamir/client';

const rows = await db.query('docs')
  .where(filter.vectorSimilarity('embedding', [0.95, 0.1, 0.0], 5))
  .rows();
```

* Второй аргумент — вектор-запрос (массив float), той же размерности, что `vector_dim`.
* Третий аргумент `k` — сколько ближайших соседей вернуть.

`$score` в результатах — similarity score (чем выше, тем ближе). Для `l2` это
negative distance (0 = идентичные).

### Как это работает внутри

* **Маленькие индексы** (≤ 256 записей) — brute-force exact KNN
  (точный, но O(n)).
* **Большие индексы** — HNSW (approximate nearest neighbor):
  * `ef_construction: 200`, `M: 16` — параметры графа.
  * `ef_search: 50` — качество поиска (выше = точнее, но медленнее).
  * Мягкое удаление (tombstones) — delete не перестраивает граф.
* Top-k ограничен 10 000 (защита от DoS).

## 6. Полный пример: FTS + vector вместе

```ts
import { ddl, write, filter, Batch } from '@shamir/client';

// Создание таблицы + двух индексов
await Batch.create('setup')
  .add('mk-table', ddl.createTable('articles', { repo: 'main' }))
  .add('fts-idx', ddl.createIndex('title_fts', 'articles', [['title']], {
    index_type:    'fts',
    fts_tokenizer: 'stemmed_en',
  }))
  .add('vec-idx', ddl.createIndex('embedding_idx', 'articles', [['embedding']], {
    index_type:    'vector',
    vector_dim:    3,
    vector_metric: 'cosine',
  }))
  .execute(client, 'my_app');

// Вставка данных
await Batch.create('insert')
  .add('a1', write.insert('articles', [
    { title: 'Rust programming language', embedding: [1.0, 0.0, 0.0] },
  ]))
  .add('a2', write.insert('articles', [
    { title: 'Python for data science', embedding: [0.0, 1.0, 0.0] },
  ]))
  .execute(client, 'my_app');

// Поиск по тексту
const textRows = await db.query('articles')
  .where(filter.fts('title', 'programming', 'and'))
  .rows();

// Поиск по вектору
const vecRows = await db.query('articles')
  .where(filter.vectorSimilarity('embedding', [0.9, 0.1, 0.0], 2))
  .rows();
```

## 7. Транзакции и FTS/Vector

Индексы FTS и Vector — **проекции** (overlay), как и обычные индексы.
Внутри транзакции:

* Вставленные записи немедленно видны векторному поиску **внутри tx**
  (staged vectors).
* При коммите — векторы промотируются в HNSW-граф.
* При rollback — staged vectors отбрасываются.

<!-- TODO: verify staged vector merge under concurrent tx commit — see vector_backend.rs TxContext -->

Crash recovery: FTS и Vector-индексы восстанавливаются из WAL при старте
(перестройка posting lists / HNSW-графа из durable-данных).

## 8. Functional-индекс (бонус)

Третий вид `index2` — functional: индекс по **результату функции** над
полем. Пример: case-insensitive поиск по email:

```ts
await Batch.create('mk-func-idx')
  .add('idx', ddl.createIndex('email_lower', 'users', [['email']], {
    index_type:   'functional',
    functional_op: 'lower',
  }))
  .execute(client, 'my_app');
```

Запрос через computed-фильтр:

```ts
const rows = await db.query('users')
  .where(filter.computed('lower', 'email', 'eq', 'alice@foo.com'))
  .rows();
```

Движок применяет `lower(email)` к каждой записи (или через индекс, если
он есть) и сравнивает результат с `"alice@foo.com"`.

## Что важно знать уже сейчас (дозированно)

* **FTS без индекса — работает, но медленно.** Создай индекс, как только
  данных становится больше нескольких сотен записей.
* **HNSW — approximate.** Recall@10 на 1K векторах — ~95–99% (проверено
  тестами). Для exact KNN на малых данных — автоматический brute-force.
* **Размерность — fixed.** `vector_dim` задаётся при создании индекса и
  не меняется. Все записи должны иметь вектор той же размерности.
* **FTS и vector — как обычные индексы.** Восстанавливаются из WAL,
  переживают crash, поддерживают migration между репозиториями.
* **`$score` — не persist.** Это runtime-оценка, вычисленная при запросе.
  Не хранится в записи.

## Куда дальше

||| Упёрся в… | Поднимайся на |
||---|---|---|
|| «выкатываю в прод, нужны метрики и сервис» | [Этаж 7 — Эксплуатация](./07-operations.md) |
|| «нужна децентрализация / P2P» | [Этаж 8 — Interconnect](./08-interconnect.md) |
