# Embeddings & Vector Search — Design

Status: **planned, no code yet.** Pre-flight design doc.

Companion to `TRANSACTIONS.md` / `TRANSACTIONS_IMPL.md` style — Russian
conversational explanation up top, formal English design contract
below.

---

## Кратко по-русски

### Зачем

Современные приложения хотят семантический поиск — «найди мне
документы похожие по смыслу на этот запрос», а не только LIKE-match
по подстроке. Это делается через **embedding-векторы**: текст
прогоняется через нейросеть-эмбеддер, она возвращает вектор из
сотен-тысяч float'ов, и семантически близкие тексты дают
геометрически близкие векторы. Поиск = ANN (Approximate Nearest
Neighbors) в этом многомерном пространстве.

ShamirDB сейчас этого не умеет.

### Как ложится в архитектуру

Главное архитектурное решение, к которому пришли в обсуждении:
**ShamirDB не считает embeddings.** Вычисление embedding'а —
ML-модель с весами от десятков MB до гигабайт, тащить это в
embedded-БД ломает всю простоту. Вместо этого:

- **ShamirDB определяет контракт** (vector storage + ANN index +
  опциональный embedder-plugin protocol).
- **Embedding-провайдеры подключаются** по индустриальному стандарту
  (OpenAI-compatible `/v1/embeddings`), которым уже говорят все
  релевантные runtime'ы: OpenAI, Voyage, Cohere, Ollama, TEI, vLLM,
  любой 30-строчный собственный FastAPI.

Это инверсия: ShamirDB — passive consumer, плагины активные. Та же
философия что у `Store` trait для backend'ов хранения.

### Два уровня

**Уровень 0 — pure vector storage (всегда работает).**

Клиент вычислил embedding (где угодно — OpenAI, локальный
sentence-transformers, кастом), прислал готовый `Vec<f32>`. ShamirDB
хранит, индексирует через HNSW, делает ANN-поиск. Не знает откуда
вектор и не интересуется. Этот уровень покрывает 100% базовых нужд.

**Уровень 1 — optional embedder plugin (для удобства).**

Опционально. В конфиге сервера декларируется один или несколько
embedder'ов (по сути — endpoint + model + dim). Index может быть
помечен `auto_embed: { embedder: "v1", from_field: "text" }`. На
INSERT/UPDATE сервер автоматически прогоняет указанное поле через
embedder и пишет результат в vector-поле.

Опт-ин на уровне индекса. Базовый INSERT на полях без auto_embed
никогда не тормозится.

### Стандарт — OpenAI Embeddings API

OpenAI задал формат `/v1/embeddings`, и его сейчас поддерживают
все. Один HTTP-протокол → все варианты бесплатно покрываются:

| Провайдер | Стоимость | Где запускается |
|---|---|---|
| OpenAI text-embedding-3-small/large | $0.02 / 1M tokens | cloud |
| Voyage AI (recommended by Anthropic) | $0.06 / 1M | cloud |
| Cohere embed-v3 | $0.10 / 1M (slightly different protocol) | cloud |
| Ollama локально | $0 | localhost:11434 |
| TEI (HuggingFace text-embeddings-inference) | $0 | self-hosted |
| vLLM с embedding-моделью | $0 | self-hosted |
| свой Python сервер | $0 | 30 строк FastAPI |

Не нужно в наш Rust crate тащить ML runtime, ONNX, веса моделей.
Один `reqwest`-клиент покрывает всё.

### Что про latency и стоимость

Возражение, которое возникало в `TRANSACTIONS.md` против "LLM-в-insert":
latency, cost, internet dependency, недетерминизм. Для embeddings
оно тоже применимо, **но смягчается**:

- Embeddings на 1-2 порядка дешевле и быстрее completion'а
  (text-embedding-3-small = $0.02/1M vs Haiku $0.80/1M; ~50ms vs
  ~1s latency).
- Локальный Ollama даёт **бесплатно и ~30ms** на CPU для MiniLM.
- Это **явный opt-in** через флаг `auto_embed` на конкретный индекс.
  Кто не хочет — не включает.

Так что для embeddings подход допустим (а для LLM-completion в
INSERT — нет).

### Batching — обязательно

OpenAI/Voyage принимают до 1000-2000 текстов за один call.
ShamirDB-сервер должен накапливать queue auto-embed запросов и
вызывать embedder батчем, иначе на 1000 INSERT'ах получается 1000
последовательных HTTP-call'ов = катастрофа.

Аккумулятор: micro-batch с timeout (e.g. 50ms или 256 items).

---

## Architecture

### Layer 0 — pure vector storage (always available)

Three new public bits:

```rust
// shamir-query-types::admin
CreateVectorIndexOp {
    create_vector_index: String,   // index name
    table: String,
    field: FieldPath,              // path to the vector field
    dim: usize,                    // hard-validated on every write
    metric: VectorMetric,          // Cosine | DotProduct | EuclideanL2
    repo: String,
    // optional convenience — see Layer 1
    auto_embed: Option<AutoEmbedConfig>,
}

// shamir-query-types::filter  (new variant of Filter)
Filter::VectorSearch {
    field: FieldPath,
    vector: Vec<f32>,
    k: usize,
    metric: Option<VectorMetric>,   // override; default = index's metric
}

// shamir-query-types::filter
pub enum VectorMetric {
    Cosine,
    DotProduct,
    EuclideanL2,
}
```

Wire shape (msgpack — wire form; clients build this via the query builder):

```msgpack
{ "create_vector_index": "by_embedding",
  "table": "docs", "field": ["embedding"],
  "dim": 768, "metric": "cosine" }

{ "from": "docs",
  "where": { "op": "vector_search", "field": ["embedding"],
             "vector": [0.12, -0.04, ...], "k": 10 } }
```

When this ships, the TS client will expose:

```ts
import { ddl, filter, Batch } from '@shamir/client';

// DDL — ddl.createIndex already accepts index_type/vector_dim/vector_metric
await Batch.create('mk-vec')
  .add('idx', ddl.createIndex('by_embedding', 'docs', [['embedding']], {
    index_type:    'vector',
    vector_dim:    768,
    vector_metric: 'cosine',
  }))
  .execute(client, 'my_app');

// Query — filter.vectorSimilarity already exists in the current builder
const rows = await db.query('docs')
  .where(filter.vectorSimilarity('embedding', [0.12, -0.04, /* ... */], 10))
  .rows();
```

Server-side: a new `IndexKind::Vector` alongside `Regular` / `Unique`
in the existing `IndexManager`. Storage backend choice for the HNSW
graph:

- **Phase 0**: simple in-memory HNSW via `hnsw_rs` crate. Lost on
  restart, rebuilt by full-table scan during `init`. Acceptable for
  tables ≤ 100K docs.
- **Phase 1**: persisted HNSW (custom binary layout in
  `__vec_idx__<table>__<idx>`) so restart is O(open) not O(rebuild).
- **Phase 2**: tiered — recent inserts in mutable in-memory layer,
  older entries in immutable persisted layer, periodic merge.

Pure storage protocol: every record carries `embedding: Vec<f32>` (or
nested in any field path). Client INSERTs the vector explicitly.
DB validates `vector.len() == index.dim` on every write.

### Layer 1 — optional embedder plugin

In-server trait, all built-in implementations fit one HTTP-shaped
abstraction:

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    fn name(&self) -> &str;
    fn dim(&self) -> usize;
    fn metric(&self) -> VectorMetric;

    /// Batch is critical — providers accept up to 1000-2000 inputs
    /// per call and bill the same as one big call vs many small.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

pub enum EmbedError {
    Transport(String),    // network / 5xx
    Auth(String),         // bad API key
    RateLimit { retry_after_ms: Option<u64> },
    Invalid { reason: String },  // 4xx, model rejected input
    Timeout,
}
```

Built-in implementations (all in `shamir-server::embedders` module):

- **`OpenAiCompatibleEmbedder`** — covers OpenAI, Voyage, Ollama,
  TEI, vLLM, own FastAPI. Just configure `endpoint` + `model` +
  optional `api_key`.
- **`OllamaEmbedder`** — convenience wrapper; same as
  OpenAiCompatible with `endpoint=localhost:11434/api/embeddings`
  default.
- **(future)** `OnnxEmbedder` behind feature flag — local ONNX
  runtime via `ort` crate. Heavy (~30MB binary increase), opt-in.
- **(future)** `BundledMiniLMEmbedder` behind feature flag — embeds
  the 22MB `all-MiniLM-L6-v2` weights into the binary via `candle-rs`.
  For zero-config setup.

### Server config (`.ktav`)

```ktav
embedders: [
    {
        name: "v1"
        protocol: openai_embeddings
        endpoint: "http://localhost:11434/v1/embeddings"
        model: "nomic-embed-text"
        dim: 768
        metric: cosine
        timeout_ms: 5000
        max_concurrent: 16
        # batching parameters
        batch_max_size: 256
        batch_max_wait_ms: 50
    }
    {
        name: "openai"
        protocol: openai_embeddings
        endpoint: "https://api.openai.com/v1/embeddings"
        model: "text-embedding-3-small"
        dim: 1536
        metric: cosine
        api_key_env: OPENAI_API_KEY
    }
]
```

Each embedder gets:
- a name (referenced from `auto_embed` configs)
- a dim (validated against index dims)
- batching parameters (size cap + max wait time)
- credentials (env-var name to read from, never in config text)

### Auto-embed-on-change

When an index is configured with `auto_embed` (wire form; clients build this via the query builder):

```msgpack
{
  "create_vector_index": "by_text",
  "table": "docs",
  "field": ["embedding"],
  "dim": 768,
  "metric": "cosine",
  "auto_embed": {
    "embedder": "v1",
    "from_field": ["text"]
  }
}
```

Server behaviour on `INSERT { text: "...", ... }`:

1. Detect that this table has an `auto_embed` index covering `text`.
2. Add the text + record-id to the named embedder's pending queue.
3. Either flush the queue when it hits `batch_max_size` OR
   `batch_max_wait_ms` elapses.
4. On batch result: for each (record_id, vector) pair, write the
   vector into the record's `embedding` field via a follow-up
   internal write, and update the HNSW index.

Two failure modes the user must choose between (config flag):

- **`auto_embed_strict: true`** — INSERT blocks until the embedder
  call completes. Slow, but the record is queryable immediately.
- **`auto_embed_strict: false`** (default) — INSERT returns immediately;
  the vector arrives asynchronously. If the embedder is unavailable,
  the system retries with backoff and the record is searchable
  later (eventually consistent).

In strict=false mode, an internal `__pending_embed__` journal is kept
so retries survive restart.

### Validation rules

- `dim` field on index is mandatory and matched on every write. A
  vector of wrong length is `validation` error.
- `metric` is set at index creation, immutable.
- An index can have AT MOST ONE `auto_embed` config; multi-source
  auto-embed (concat of several text fields) is a future extension
  via `from_fields: [["title"], ["body"]]` with a `joiner: "\n\n"`.
- Renaming an embedder in config is allowed only if `dim` and
  `metric` stay the same.
- Migrating to a different model = explicit `recompute_vectors`
  admin op (see Operations).

### Operations

Admin ops added to `BatchOp` enum:

```rust
RecomputeVectors {
    recompute_vectors: String,   // index name
    table: String,
    repo: String,
    embedder: String,            // which embedder to use (must dim-match index)
    rate_limit_per_sec: Option<u32>,
}

ListEmbedders   // returns configured embedders + their dim/metric/usage stats
EmbedderStats   // tokens used, requests/sec, errors per embedder

// Pause / resume auto-embed for an index — useful when external API
// is rate-limited or being upgraded
PauseAutoEmbed { table, repo, index }
ResumeAutoEmbed { table, repo, index }
```

The observability HTTP server (`/healthz` etc.) gains:

- `/embedders` — msgpack list of configured embedders + last 1m stats
  (calls, tokens, errors, p99 latency).
- Prometheus metrics: `shamir_embedder_calls_total`,
  `shamir_embedder_tokens_total`, `shamir_embedder_errors_total`,
  `shamir_embedder_latency_seconds`.

### Storage layout

```
__data__<table>             — record bytes (existing)
__info__<table>             — interner / counter / index metadata (existing)
__idx__<table>__<idx>       — regular / unique index entries (existing)
__vec_idx__<table>__<idx>   — HNSW graph nodes + serialised vectors  (NEW)
__pending_embed__<table>    — async auto-embed queue + retry state    (NEW)
```

The HNSW graph stores `(record_id, vector_bytes)` and the graph
adjacency layers. Format: each layer = sorted list of
`(node_id → [neighbour_id; M])`. Standard HNSW parameters: M=16,
ef_construction=200, ef_search defaults to 64 (overridable per
query).

---

## Out of scope (explicitly)

- **Embedding LLM prompts / RAG pipelines.** That's app-level. We
  give them the vector storage + search; assembling RAG is their job.
- **Hybrid (FTS + vector) reranking.** Mentioned as related work
  below — separate doc.
- **Cross-encoder rerankers.** Same — different abstraction.
- **Computing embeddings without a network call when a model is
  small enough to run in-process.** Possible later via
  `BundledMiniLMEmbedder` feature flag, but not the first cut.
- **Multi-vector / late-interaction (ColBERT-style).** Different
  index structure entirely; future.
- **Filtered ANN (`vector_search` + `where city='NYC'`).** Hard
  problem with HNSW. Phase 1 will only support post-filter
  (search top-K, then drop those that don't match the where).
  Pre-filter or co-filter is a research-level extension.

---

## Open questions / things to decide before code

1. **Default behaviour on embedder failure during INSERT**
   (strict vs async) — both have ergonomic and safety implications.
   Default async is friendlier but masks problems.
2. **Cost telemetry granularity** — bill per (index, day)?
   per-(client, day)? both?
3. **Vector compression** — float16 / int8 quantization. 2-4×
   storage win, ~1% recall cost. Decide whether to support out of
   the box or as future opt-in.
4. **Backwards compat when changing embedder dim** — `recompute_vectors`
   admin op covers it but we need to decide if reads against the
   old vectors should error or silently return zero results during
   the transition.
5. **Should ShamirDB log full text being embedded?** Privacy
   concern: by default no, but operators may want it for debugging.
   Off by default, opt-in via config.

---

## Related future work (separate docs)

- **Full-text search (FTS).** Inverted index, BM25 scoring. Same
  philosophy: own simple implementation up front, optionally
  back with `tantivy` for production. Sketched in earlier
  conversation; document pending.
- **Hybrid search.** Combining FTS + vector results via
  reciprocal rank fusion (RRF) or learned linear combination.
  Requires both subsystems to exist first.
- **Change subscriptions API.** Parallel design: server emits
  events on insert/update/delete; clients subscribe; LLM-pipelines
  / audit / materialised views all consume the stream. Gives a
  clean foundation for things like "summarise this record with
  Haiku" without putting LLM into the write path.

---

## Order of work (when we pick this up)

1. **`VectorMetric` + `Filter::VectorSearch` + `CreateVectorIndexOp`
   in `shamir-query-types`** (DTOs, no logic) — 2 ч
2. **`IndexKind::Vector`** in `IndexManager` — 1 day
3. **In-memory HNSW** wired through `hnsw_rs` crate, full-rebuild
   on `init` from a scan — 2-3 days
4. **`vector_search` planner integration** — when the read planner
   sees a `VectorSearch` filter against an indexed field, route
   through HNSW.knn(k) — 1 day
5. **Persisted HNSW** (Phase 1 — survives restart) — 3-5 days
6. **`Embedder` trait + `OpenAiCompatibleEmbedder` impl** — 2 days
7. **Embedder config in `.ktav`** + named-embedder registry — 1 day
8. **Auto-embed-on-change** with batch accumulator + retries — 3-4 days
9. **Admin ops** (RecomputeVectors, ListEmbedders, Pause/Resume) — 1 day
10. **Observability hooks** (`/embedders` endpoint, Prometheus metrics) — 1 day
11. **e2e test file `tests/e2e/tests/14-vectors.test.js`** with a
    real Ollama subprocess — 2 days
12. **Docs**: this file marked "implemented", LOGIC_FLOW updated,
    root README capability list updated — 0.5 day

**Total: ~3-4 недели сфокусированной работы.** Phase 0 + Phase 1
(in-memory + persisted HNSW + OpenAI-compatible embedder + auto-embed).
Quantization, hybrid search, filtered ANN — отдельные спринты.
