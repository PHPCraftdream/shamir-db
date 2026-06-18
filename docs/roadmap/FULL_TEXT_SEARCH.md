# Full-Text Search — Design

Status: **planned, no code yet.** Pre-flight design doc.

Same structure as the other roadmap docs — Russian conversational
explanation up top, formal English design contract below.

---

## Кратко по-русски

### Зачем

Сейчас единственный способ найти документ по содержимому — точное
сравнение через `Filter::Eq` или `Filter::In`. Запросов вида «дай мне
все документы где в описании есть слово `embedded`» движок не умеет.
Это базовая ожидаемая фича — без неё product-search и admin-UI
поиск работают как «select all + filter в JS».

### Что это технически

Семь функциональных кусков, каждый — отдельное архитектурное решение:

1. **Tokenization** — текст → последовательность токенов
2. **Normalization** — lowercase, Unicode NFC, accent stripping
3. **Stemming** — `running / runs / ran → run`
4. **Stop-words** — фильтрация шума («и», «на», «the», «a»)
5. **Posting lists** — `token → отсортированный список doc_ids`
6. **Query language** — boolean (AND/OR/NOT), phrase, prefix
7. **Scoring** — BM25 / TF-IDF, ранжирование по релевантности

Не каждый нужен сразу. Phase 0 — самое необходимое; продвинутое —
отдельные phase'ы по мере спроса.

### Как ложится в архитектуру

Ровно как remaining index types — **новый `IndexKind::FullText`**
рядом с `Regular` / `Unique`. Тот же `IndexManager`, тот же `Store`
trait под капотом, новая семантика на write/read.

Storage layout — добавляются два префикса в `info_store`:

```
__fts_idx__<table>__<idx>__<token>     — posting list per token
__fts_meta__<table>__<idx>             — total docs + doc-lengths (для BM25)
```

Posting list per token — отсортированный `Vec<RecordId>` (16
bytes/entry). На старте — bincoded sorted vec; для частотных
токенов оптимизация (delta+varint) — Phase 1+.

### Где будет больно

Три неочевидных места.

**1. Write amplification.** Документ с 100 токенами = 100 posting
list updates на каждый INSERT. На UPDATE — diff между старым и
новым набором токенов. Mitigation:

- Batching внутри одной `execute()` — собрать все per-token
  updates, дедупнуть, применить одним проходом.
- Async indexing опционально — INSERT возвращает быстро,
  индексация в фоновой очереди, eventual consistency. Управляется
  per-index флагом (как `auto_embed_strict` в других дизайнах —
  тот же паттерн).

**2. Размер индекса.** FTS индекс реально 2-5× от размера исходных
текстов. С positions для phrase search — ещё +50%. Если текстовая
таблица 10 GB, готовьтесь к 30-50 GB FTS. Mitigation:

- Indexing подмножества полей, не всех
- Stop-words убирают ~30-40 % объёма для популярных языков
- Phrase positions опционально (per-index config)

**3. Tokenization — выбор навсегда.** Решение, которое привязывает
индекс к языку/конвенции. Сменить tokenizer = пересчитать весь
индекс через `recompute_fts_index` admin op.

| Tokenizer | Покрытие | Подходит | Не подходит |
|---|---|---|---|
| **Whitespace + lowercase** | ASCII-only | quick prototype | Cyrillic / Asian |
| **Unicode word boundaries** | весь Unicode (через `unicode-segmentation`) | универсал | typo-tolerance |
| **N-gram (3-gram)** | substring + typo-tolerance | партномера, медкоды | normal text (взрыв размера) |
| **Language-specific (snowball)** | максимум качества | mono-lingual prod | multi-lingual |

Мы — русскоязычный проект; **Unicode tokenization обязательно**.
ASCII-only сразу вылетает.

### Своя реализация vs Tantivy

[Tantivy](https://github.com/quickwit-oss/tantivy) — production-grade
FTS на Rust, ядро Quickwit (multi-billion-doc analytics). Огромный
плюс: instant production-quality. Минусы:

- ~5 MB к binary
- **Свой storage layer** — не использует наш `Store` trait, хочет
  владеть директорией с mmap-файлами
- Schema-based — каждый документ должен соответствовать declared
  schema
- Индексные сегменты с merge политикой (LSM-сложности)

Архитектурный mismatch — Tantivy хочет быть БД-внутри-БД. Можно
прикрутить, но это рассинхронизация: данные у нас, FTS-индекс у
него, sync через write hooks.

**Своя реализация:**
- Phase 0 (boolean только) — ~3-5 дней
- Phase 1 (+ BM25 scoring) — ещё ~3-5 дней
- Phase 2 (+ phrase / positions) — ещё ~5-7 дней
- Production-quality на уровне tantivy — недостижимо без месяцев

**Прагматика:** своя реализация для интеграции с нашим Store /
IndexManager. Tantivy как опциональный backend
(`feature = "fts-tantivy"`) если кому-то понадобится enterprise-grade
— отдельная сессия.

### Phasing

| Phase | Что входит | Время |
|---|---|---|
| **0** | Boolean FTS, Unicode tokenizer, без scoring | ~3-5 дней |
| **1** | + BM25 scoring + ORDER BY relevance + field boosting | ~3-5 дней |
| **2** | + Phrase queries (positions) | ~5-7 дней |
| **3** | + Stemming + multi-language | ~2-3 дня |

Phase 0 + 1 разумно делать одним spike-комитом — без scoring «search»
по UX не работает (результаты в произвольном порядке).

Phase 2 (phrase) и 3 (stemming) — отдельные плановые работы.

### Что НЕ делаем (явно)

- **Fuzzy / typo-tolerance через edit distance** (FST-based) —
  отдельный концерн, требует другой структуры индекса.
- **Synonyms dictionaries** — клиент пусть expand'ит запрос сам.
- **Highlighting** (возврат `<em>matched</em>` фрагментов) —
  отдельная задача поверх FTS results.
- **Spelling correction** — нужен symspell или vocabulary; future.
- **Multi-language detection per document** — слишком много магии.

---

## Architecture

### New types in `shamir-query-types`

```rust
// shamir-query-types::admin
pub struct CreateFtsIndexOp {
    pub create_fts_index: String,         // index name
    pub table: String,
    pub fields: Vec<FieldPath>,           // multi-field supported
    pub repo: String,
    pub config: FtsConfig,
}

pub struct FtsConfig {
    /// Default: TokenizerKind::Unicode.
    pub tokenizer: TokenizerKind,
    /// Default: true.
    pub case_insensitive: bool,
    /// Default: empty. Optional pre-built lists per language can be
    /// referenced by name (e.g. "russian", "english") — server
    /// resolves to a built-in word list.
    pub stop_words: StopWordsConfig,
    /// Phase 2: store positions for phrase queries. Doubles index
    /// size; off by default.
    #[serde(default)]
    pub store_positions: bool,
    /// Phase 1: store term frequencies for BM25. On by default once
    /// Phase 1 ships.
    #[serde(default = "default_true")]
    pub store_term_frequency: bool,
    /// Phase 3: optional snowball stemmer.
    #[serde(default)]
    pub stemmer: Option<StemmerKind>,
    /// Per-field boost for BM25. Default 1.0 each.
    #[serde(default)]
    pub field_boosts: TMap<String, f32>,
}

pub enum TokenizerKind {
    Whitespace,
    Unicode,                 // default
    Ngram { n: u8 },         // 3 typical
}

pub enum StopWordsConfig {
    None,
    BuiltIn(String),         // "russian" | "english" | "german" | ...
    Custom(Vec<String>),
}

pub enum StemmerKind {
    Russian,
    English,
    German,
    French,
    // ... whatever rust-stemmers supports
}
```

### New filter variant

```rust
// shamir-query-types::filter
pub enum FtsMatchMode {
    /// All terms must appear (in any order, anywhere in the field).
    AllTerms,
    /// Any term match — equivalent to OR of single-term searches.
    AnyTerms,
    /// Phrase — terms must appear consecutively in order. Requires
    /// `store_positions = true` on the index. Phase 2.
    Phrase,
}

// added to Filter enum
Filter::FtsMatch {
    field: FieldPath,            // must match one of the indexed fields
    query: String,               // raw user query — server tokenizes
    #[serde(default)]
    mode: FtsMatchMode,          // default = AllTerms
}
```

Wire shape (msgpack — wire form; clients build this via the query builder):

```msgpack
{ "create_fts_index": "by_body",
  "table": "docs",
  "fields": [["title"], ["body"]],
  "config": {
    "tokenizer": "unicode",
    "case_insensitive": true,
    "stop_words": { "kind": "built_in", "name": "russian" },
    "field_boosts": { "title": 2.0, "body": 1.0 }
  } }

{ "from": "docs",
  "where": { "op": "fts_match",
             "field": ["body"],
             "query": "rust embedded",
             "mode": "all_terms" } }
```

When this ships, the TS client will expose:

```ts
import { ddl, filter, Batch } from '@shamir/client';

// DDL
await Batch.create('mk-fts')
  .add('idx', ddl.createIndex('by_body', 'docs', [['title'], ['body']], {
    index_type: 'fts',
    fts_tokenizer: 'unicode',
  }))
  .execute(client, 'my_app');

// Query — filter.fts already exists in the current builder
const rows = await db.query('docs')
  .where(filter.fts('body', 'rust embedded', 'and'))
  .rows();
```

### Storage layout

```
__data__<table>                          — record bytes (existing)
__info__<table>                          — interner + counters + index meta (existing)
__idx__<table>__<idx>                    — regular / unique posting (existing)
__fts_idx__<table>__<idx>__<token>       — posting list per token        (NEW)
__fts_meta__<table>__<idx>               — config + total_docs + doc_len (NEW)
```

Per-token posting list:

```rust
// Phase 0
struct PostingList {
    docs: Vec<RecordId>,                 // sorted ascending
}

// Phase 1 — adds term frequency (TF) + per-field hits for boosting
struct PostingListV1 {
    entries: Vec<PostingEntry>,
}
struct PostingEntry {
    doc_id: RecordId,
    /// One entry per field that contains the token in this doc.
    /// Most docs hit only 1-2 fields → small.
    field_hits: SmallVec<[FieldHit; 2]>,
}
struct FieldHit {
    /// Index into the FtsConfig.fields array
    field_idx: u8,
    /// Number of occurrences of the token in this field
    term_freq: u32,
    /// Phase 2: positions. None when store_positions = false.
    positions: Option<Vec<u32>>,
}
```

Per-index meta (read at index open, kept in memory):

```rust
struct FtsIndexMeta {
    config: FtsConfig,
    total_docs: u64,
    /// Sum of all doc lengths per field (for BM25 avgdl).
    total_field_len: Vec<u64>,
    /// avgdl per field = total_field_len[i] / total_docs
}

// Per-record: doc length per field, kept in __fts_meta__<table>__<idx>__lens__<record_id>
struct DocLengths {
    per_field: SmallVec<[u32; 4]>,
}
```

### Write-path integration

Hooks into `IndexManager::on_record_created/updated/deleted` (the
same touchpoints `Regular` / `Unique` indexes use today):

```rust
// pseudocode
async fn on_record_created(&self, record_id: RecordId, value: &InnerValue, tx: Option<&mut TxContext>) {
    for fts_idx in self.fts_indexes_for(value) {
        let mut updates: HashMap<Token, FieldHit> = HashMap::new();
        for (field_idx, path) in fts_idx.config.fields.iter().enumerate() {
            let text = resolve_field_as_string(value, path)?;
            let tokens = tokenize(&text, &fts_idx.config);
            for (pos, tok) in tokens.enumerate() {
                let entry = updates.entry(tok).or_insert_with(|| FieldHit {
                    field_idx: field_idx as u8,
                    term_freq: 0,
                    positions: fts_idx.config.store_positions.then(Vec::new),
                });
                entry.term_freq += 1;
                if let Some(p) = &mut entry.positions { p.push(pos as u32); }
            }
        }
        // One batched store update per posting list — minimises
        // info_store write count.
        for (token, field_hit) in updates {
            let key = fts_posting_key(table, idx_name, &token);
            self.upsert_posting(key, record_id, field_hit, tx).await?;
        }
        self.update_doc_lengths(record_id, /* ... */).await?;
    }
}
```

`on_record_deleted` reverses each posting (remove `record_id` from
the sorted list). `on_record_updated` does delete-then-insert in the
simple case; later optimisation can compute the diff.

### Read-path integration — Phase 0 (boolean)

Plug into `try_plan_index_scan` (the same planner the read path
already uses for `Regular` indexes). On `Filter::FtsMatch`:

1. Tokenize the query string the same way the index was built
   (same tokenizer, same case rules, same stop-words).
2. For each token, fetch the posting list.
3. Combine according to `mode`:
   - `AllTerms` — set intersection (Vec merge — both lists sorted)
   - `AnyTerms` — set union
4. Result: `BTreeSet<RecordId>` — same shape `Regular` index lookup
   produces, so the rest of the read pipeline is unchanged.

If a residual filter exists (e.g. `fts_match AND age >= 18`), the
read planner already handles it — the residual gets compiled and
evaluated per candidate, exactly like for regular indexes.

### Read-path integration — Phase 1 (BM25 scoring)

When the query is `ORDER BY _score DESC LIMIT K`:

1. Tokenize query.
2. Walk posting lists, computing per-doc BM25:

```
score(d, q) = Σ over t in q:
    boost(t.field) * IDF(t) * TF_norm(t, d)

IDF(t)      = log((N - df(t) + 0.5) / (df(t) + 0.5) + 1)
TF_norm     = (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl/avgdl))

constants k1 = 1.2, b = 0.75   (BM25 defaults)
```

3. Maintain a top-K min-heap as we walk; final result is
   `Vec<(RecordId, score)>` sorted desc.

For the no-LIMIT case fall back to scoring all matches and sorting.

The new sort direction `_score` only makes sense in queries with an
`fts_match` filter — engine validates at parse time.

### Field boosting

In FtsConfig: `field_boosts: { "title": 2.0, "body": 1.0 }`. Title
matches contribute 2× to the BM25 score. Common requirement.

### Multi-field — one index covers many fields

A single FTS index can cover multiple fields:

```
{ "create_fts_index": "by_body",
  "fields": [["title"], ["body"], ["tags", "name"]] }
```

Per-token posting list stores per-field hits (`FieldHit { field_idx,
term_freq, positions? }`). Single index supports search across all
covered fields with field-aware scoring. Cleaner than maintaining
N separate indexes.

### Validation rules

- `Filter::FtsMatch.field` must match exactly one of the index's
  `fields` array. Multi-field search is via separate `OR` of
  `FtsMatch` clauses — keeps the per-clause semantics simple.
- `mode = Phrase` requires `store_positions = true` on the index;
  otherwise validation error.
- Switching tokenizer / stemmer / case-insensitivity = explicit
  `recompute_fts_index` admin op (analogous to other recompute ops).
- Stop-word changes require recompute too (otherwise old indexed
  docs have stop-tokens, new queries strip them — mismatched).

### Operations

Admin ops added to `BatchOp`:

```rust
RecomputeFtsIndex {
    recompute_fts_index: String,   // index name
    table: String,
    repo: String,
    rate_limit_records_per_sec: Option<u32>,
}

DropFtsIndex { drop_fts_index: String, table: String, repo: String }

ListFtsIndexes { /* table, repo */ }   // returns config + size stats
```

Observability metrics:

- `shamir_fts_index_size_bytes{index}` — per-index storage cost
- `shamir_fts_token_count{index}` — distinct token count
- `shamir_fts_query_latency_seconds{index, mode}` — p50/p99 by op
- `shamir_fts_write_amplification_ratio{index}` — postings-touched
  per record-touched

### `_score` virtual column

When an `FtsMatch` filter is present, each result record gets a
synthetic `_score` field. `ORDER BY` and `SELECT` can reference it
(wire form; clients build this via the query builder):

```msgpack
{ "from": "docs",
  "where": { "op": "fts_match", "field": ["body"], "query": "rust embedded" },
  "order_by": { "items": [{ "field": ["_score"], "direction": "desc" }] },
  "select": { "items": [
    { "type": "field", "path": ["title"] },
    { "type": "field", "path": ["_score"] }
  ]},
  "pagination": { "mode": "LimitOffset", "limit": 10, "offset": 0 } }
```

Without an FTS filter `_score` doesn't exist; referencing it errors.

---

## Phasing — concrete deliverables

### Phase 0 — boolean FTS

- `IndexKind::FullText` + `FtsConfig` + `TokenizerKind::{Whitespace, Unicode}`
- `Filter::FtsMatch { field, query, mode: AllTerms | AnyTerms }`
- Per-token posting list (sorted `Vec<RecordId>` only, no TF/positions)
- Synchronous index update on every write (write amplification
  accepted on day one)
- Stop-word config (built-in lists for `russian`, `english`)
- Tests: unit + e2e through node SDK
- Microbench: `bench_fts_search` added to `engine_perf.rs`

**~3-5 дней.**

### Phase 1 — BM25 scoring + relevance

- Posting list entries gain `field_idx + term_freq` (per-field hits)
- `__fts_meta__<table>__<idx>` stores `total_docs` and per-field
  `total_field_len`
- Per-record `DocLengths` written alongside each indexed record
- BM25 scoring at query time, top-K via min-heap
- `_score` virtual column, `ORDER BY _score DESC LIMIT N`
- Field boosting via `FtsConfig.field_boosts`
- Bench cases: `bench_fts_search_top10`, `bench_fts_score_full`

**~3-5 дней.** Worth bundling with Phase 0 in a single spike — without
scoring "search" UX is broken.

### Phase 2 — phrase queries

- Positions stored per (token, doc, field) when `store_positions = true`
- `mode = Phrase` matcher walks positions to find consecutive runs
- Index-level config — opt-in, doubles posting size

**~5-7 дней.**

### Phase 3 — stemming + multi-language

- `rust-stemmers` crate (Snowball)
- Per-index `stemmer: Option<StemmerKind>`
- Stop-word lists per language (built-in `RUSSIAN`, `ENGLISH`,
  `GERMAN`, `FRENCH`, ...)
- Tokenization pipeline: tokenize → lowercase → strip stops → stem

**~2-3 дня.**

---

## Open questions — what to decide before code

1. **Default tokenizer.** I vote `Unicode`. `Whitespace` stays as a
   faster opt-in for ASCII-only data.

2. **Stop-words by default — yes or no?** Без них индекс +30% по
   объёму. Со словарями — куча языков, неочевидный default. Compromise:
   default = `None`; opt-in via `built_in: "russian"` / `"english"`
   с заранее заготовленными списками.

3. **Sync vs async index update.** Sync (consistency) by default;
   async (eventual, with `__pending_fts__` retry journal) opt-in
   per index.

4. **Phase 0 alone, or 0 + 1 together?** Без BM25 «search» по UX
   почти бесполезен (произвольный порядок). Лучше делать оба phase
   как один spike, ~6-10 дней суммарно.

5. **Phrase positions — где?** Я бы делал отдельной phase 2 за
   `store_positions` флагом (default off). Удваивает индекс — должно
   быть осознанным выбором.

6. **Multi-field — один индекс на N полей?** Да: один индекс с
   per-field posting hits. Поддерживает field boosting, проще для
   пользователя, экономнее по storage чем N отдельных индексов.

7. **Field boosting в Phase 1?** Стандартная фича (title^2 + body^1).
   ~1 день работы. Включить.

8. **Per-language pre-built stop-word lists** — где их брать? Проще
   всего embed как `&'static [&'static str]` from a known set
   (snowball / nltk lists). ~10 KB на язык в binary.

---

## Order of work — Phase 0 + 1 spike

1. `TokenizerKind` + `Tokenizer` trait + `WhitespaceTokenizer` +
   `UnicodeTokenizer` (via `unicode-segmentation` crate). Tests for
   token boundaries on Cyrillic / mixed scripts. **1 day**

2. `FtsConfig` + `CreateFtsIndexOp` + `Filter::FtsMatch` DTOs in
   `shamir-query-types`. **2 hours**

3. `IndexKind::FullText` registration in `IndexManager`; storage
   layout for `__fts_idx__` and `__fts_meta__`. **1 day**

4. Posting list encoding (Phase 0: sorted `Vec<RecordId>` bincoded).
   `upsert`/`remove` operations on a posting list. **1 day**

5. Write-path hooks: `on_record_created/updated/deleted` extended
   to update FTS postings + doc-lengths in batched fashion. **1-2 days**

6. Read-path hooks: `try_plan_fts_match` integrated alongside
   `try_plan_index_scan`. Set intersection / union for AllTerms /
   AnyTerms modes. **1 day**

7. Stop-word built-in lists for `russian` + `english` (embedded
   in binary as `&'static [&'static str]`). **2 hours**

8. **Phase 1 starts here.** Posting list V1 with `field_hits` +
   per-field TF; `__fts_meta__` with `total_docs` and per-field
   `avgdl`; `DocLengths` per record. **1-2 days**

9. BM25 scoring at query time + top-K min-heap; `_score` virtual
   column; `ORDER BY _score` validation. **1-2 days**

10. Field boosting via `FtsConfig.field_boosts`. **0.5 day**

11. `RecomputeFtsIndex` admin op (full-table scan + rebuild). **0.5 day**

12. Microbenches in `crates/shamir-db/benches/engine_perf.rs`:
    `fts_search_simple`, `fts_search_with_score_top10`,
    `fts_index_insert_amp`. **0.5 day**

13. e2e tests `tests/e2e/tests/15-fts.test.js`: setup → seed varied
    text → search by single term → AllTerms / AnyTerms / boosting /
    `_score` ordering. **1 day**

14. Docs: this file marked "implemented", `LOGIC_FLOW.md` updated,
    root README capability list updated. **0.5 day**

**Phase 0 + 1 total: ~9-12 days of focused work.**

---

## Future phases (separate sprints)

- **Phase 2 — phrase queries** with positions. Requires posting
  V2 with per-field position arrays. Index size +50-100 %.
- **Phase 3 — stemming + multi-language** via `rust-stemmers` (Snowball).
- **Phase 4 — fuzzy / typo-tolerance** via FST + edit distance. Big
  separate concern.
- **Phase 5 — highlighting** — return `<em>matched</em>` fragments
  alongside records. Depends on positions, so after Phase 2.
- **Tantivy backend** (`feature = "fts-tantivy"`) for users who
  need enterprise-scale FTS. Optional, parallel to our own impl.
