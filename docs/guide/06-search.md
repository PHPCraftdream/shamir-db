בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 6 — Поиск: полнотекстовый, векторный / embeddings

**Когда подниматься:** нужен поиск / AI-фичи.

До этого этажа запросы к ShamirDB — точные совпадения (`eq`), диапазоны
(`between`), шаблоны (`like`). Но реальные продукты требуют:
«найди документы со словом *rust* и *world*» или «найди записи, похожие
на этот вектор». Этот этаж — про два специализированных вида поиска:
**FTS** (full-text search) и **vector similarity** (ANN через HNSW).

## 1. FTS-индекс: создание

FTS-индекс строится по текстовому полю и ускоряет поиск по словам
(токенам). Создаётся как обычный индекс — батчем:

```json
{
  "id": "mk-fts",
  "queries": {
    "idx": {
      "create_index": "body_fts",
      "table": "posts",
      "fields": [["body"]],
      "index_type": "fts",
      "fts_tokenizer": "whitespace"
    }
  }
}
```

Ключевые поля:

|| Поле | Описание | Значения |
||---|---|---|
|| `index_type` | тип индекса | `"fts"` |
|| `fts_tokenizer` | алгоритм токенизации | `"whitespace"`, `"unicode"`, `"stemmed_en"`, `"ngram3"`, … |
|| `fts_language` | подсказка языка (reserved) | строка, пока не влияет на `whitespace` |

### Токенайзеры

|| Токенайзер | DSL-имя | Как работает |
||---|---|---|
| **Whitespace** | `"whitespace"` | Разбивает по пробелам → lowercase. Быстро, просто. |
| **Unicode** | `"unicode"` | Unicode-aware границы слов (буквы/цифры vs punctuation). |
| **Ngram** | `"ngram2"` … `"ngram9"` | Скользящее окно по символам. Для CJK-языков и substring-search. |
| **Full (stemming)** | `"stemmed_en"`, `"stemmed_ru"`, … | Полный pipeline: whitespace → lowercase → **stopwords** → **Snowball stemming**. 17 языков. |

Пример stemmed-индекса:

```json
{
  "create_index": "body_stemmed",
  "table": "articles",
  "fields": [["content"]],
  "index_type": "fts",
  "fts_tokenizer": "stemmed_en"
}
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

```json
{
  "from": "posts",
  "where": {
    "op": "fts",
    "field": ["body"],
    "query": "hello world",
    "mode": "and"
  }
}
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

```json
{
  "from": "posts",
  "where": {
    "op": "fts",
    "field": ["body"],
    "query": "apple banana",
    "mode": "or"
  }
}
```

Вернёт записи с `"apple"` ИЛИ `"banana"`.

### Brute-force fallback

Если FTS-индекс **не** создан, `op: "fts"` всё равно работает — движок
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

```json
{
  "id": "mk-vec",
  "queries": {
    "idx": {
      "create_index": "vec_idx",
      "table": "docs",
      "fields": [["embedding"]],
      "index_type": "vector",
      "vector_dim": 128,
      "vector_metric": "cosine"
    }
  }
}
```

|| Поле | Описание | Значения |
||---|---|---|
|| `index_type` | тип индекса | `"vector"` |
|| `vector_dim` | размерность вектора | число (default: 384) |
|| `vector_metric` | метрика расстояния | `"cosine"` (default), `"l2"`, `"dot"` |

### Метрики

|| Метрика | Формула | Когда |
||---|---|---|
|| `cosine` | косинусное сходство | embeddings (NLP, vision) — наиболее частый |
|| `l2` | евклидово расстояние | пространственные координаты, физические величины |
|| `dot` | скалярное произведение | преднормализованные векторы |

Вычисления используют **SIMD**: AVX-512F (16-lane FMA), AVX2+FMA
(8-lane), NEON (aarch64), scalar fallback.

## 5. Vector-запрос: top-k ближайших

```json
{
  "from": "docs",
  "where": {
    "op": "vector_similarity",
    "field": ["embedding"],
    "query": [0.95, 0.1, 0.0],
    "k": 5
  }
}
```

* `query` — вектор (массив float), той же размерности, что `vector_dim`.
* `k` — сколько ближайших соседей вернуть.

Ответ:

```json
{
  "results": {
    "r": {
      "records": [
        { "embedding": [1.0, 0.0, 0.0], "label": "x", "$score": 0.998 },
        { "embedding": [0.95, 0.1, 0.0], "label": "x_near", "$score": 0.999 }
      ],
      "stats": { "index_used": "index2_ranked" }
    }
  }
}
```

`$score` — similarity score (чем выше, тем ближе). Для `l2` это
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

```json
{
  "id": "setup",
  "queries": {
    "mk-table": { "create_table": "articles", "repo": "main" },
    "fts-idx": {
      "create_index": "title_fts",
      "table": "articles",
      "fields": [["title"]],
      "index_type": "fts",
      "fts_tokenizer": "stemmed_en"
    },
    "vec-idx": {
      "create_index": "embedding_idx",
      "table": "articles",
      "fields": [["embedding"]],
      "index_type": "vector",
      "vector_dim": 3,
      "vector_metric": "cosine"
    }
  }
}
```

```json
{
  "id": "insert",
  "queries": {
    "a1": {
      "insert_into": "articles",
      "values": [{ "title": "Rust programming language", "embedding": [1.0, 0.0, 0.0] }]
    },
    "a2": {
      "insert_into": "articles",
      "values": [{ "title": "Python for data science", "embedding": [0.0, 1.0, 0.0] }]
    }
  }
}
```

Поиск по тексту:

```json
{
  "id": "search-text",
  "queries": {
    "q": {
      "from": "articles",
      "where": { "op": "fts", "field": ["title"], "query": "programming", "mode": "and" }
    }
  }
}
```

Поиск по вектору:

```json
{
  "id": "search-vec",
  "queries": {
    "q": {
      "from": "articles",
      "where": {
        "op": "vector_similarity",
        "field": ["embedding"],
        "query": [0.9, 0.1, 0.0],
        "k": 2
      }
    }
  }
}
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

```json
{
  "create_index": "email_lower",
  "table": "users",
  "fields": [["email"]],
  "index_type": "functional",
  "functional_op": "lower"
}
```

Запрос через computed-фильтр:

```json
{
  "from": "users",
  "where": {
    "op": "computed",
    "expr_op": "lower",
    "field": ["email"],
    "cmp": "eq",
    "value": "alice@foo.com"
  }
}
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
