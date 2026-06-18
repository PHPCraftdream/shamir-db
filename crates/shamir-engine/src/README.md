# `shamir-engine` crate

The runtime engine for ShamirDB: database/repo/table managers, the index
subsystem, and the SDBQL query layer (filter / read / write / batch / admin /
auth). DTOs live in `shamir-query-types`; this crate owns the executable
behaviour (parsing, planning, execution, interner-aware filter compilation,
index maintenance).

## Crate layout

```
shamir-engine/src/
├── lib.rs
├── README.md
├── LAYERS.md                 # short note: где живёт интернер и почему
├── db_instance/              # DbInstance — repos within one DB
├── repo/                     # RepoInstance + BoxRepo + BoxRepoFactory (7 backends)
├── table/                    # TableManager + Table (low-level CRUD) + interner / counter / read+write executors
├── index/                    # IndexManager + IndexInfo + IndexDefinition + IndexRecordKey
└── query/                    # SDBQL query layer
    ├── batch/                # BatchPlanner + execute_batch + $query reference resolver
    ├── read/                 # parser + exec pipeline (DTOs re-exported from shamir-query-types)
    ├── write/                # re-exports + execution glue (DTOs in shamir-query-types)
    ├── admin/                # DDL re-exports
    ├── auth/                 # SessionPermissions + auth DTOs
    ├── filter/               # compile_filter + eval_context + FilterCallback
    └── common/               # shared parsers (filter / order / agg / pagination)
```

Hierarchy of managers (bottom of `shamir-db` ↓):

```
ShamirDb (in shamir-db crate)
  └── DbInstance               manages repos within one database
       └── RepoInstance        wraps one BoxRepo (storage backend) + table configs
            └── TableManager   one table: data + interner + indexes + query exec
                 └── IndexManager   regular + unique indexes (atomic flags for O(1) probe)
```

## Storage model per table

Each `TableManager` owns two underlying `Store`s in its repo:

```
__data__{table_name}   user records (InnerValue payload, RecordId keys)
__info__{table_name}   interner state + index metadata (system records)
```

`RecordId::system("internals")` / `RecordId::system("inter_max")` /
`RecordId::system("indexes")` / `RecordId::system("indexes_unique")` are the
well-known keys in the info store.

## Storage backends

Repos plug into one of seven embedded engines via `BoxRepoFactory`:

```rust
pub enum BoxRepoFactory {
    InMemory(_),
    #[cfg(feature = "sled")]   Sled(_),
    #[cfg(feature = "redb")]   Redb(_),
    #[cfg(feature = "fjall")]  Fjall(_),
    #[cfg(feature = "nebari")] Nebari(_),
    #[cfg(feature = "persy")]  Persy(_),
    #[cfg(feature = "canopy")] Canopy(_),
}
```

Each non-memory backend is gated by its cargo feature.

## TableManager surface

CRUD (all routed through interning + index maintenance):

```rust
table.read(&read_query, &filter_ctx).await?;
table.execute_insert(&insert_op).await?;
table.execute_update(&update_op, &filter_ctx).await?;
table.execute_set(&set_op).await?;       // upsert
table.execute_delete(&delete_op, &filter_ctx).await?;
```

Index management:

```rust
table.create_index("email_idx", &["email"]).await?;
table.create_unique_index("username_idx", &["username"]).await?;
table.drop_index("email_idx").await?;
table.drop_unique_index("username_idx").await?;
table.has_indexes(); // O(1), atomic flag
```

The interner is loaded lazily from `__info__` via `OnceCell` and persisted
after every write that introduced a new key.

## Query layer

All requests funnel through `BatchRequest`:

```rust
let response: BatchResponse = execute_batch(
    &batch_request,
    &table_resolver,
    Some(&admin_executor),
).await?;
```

The planner extracts `$query` references, validates them against declared
aliases, sorts dependencies into parallel stages, and runs each stage
concurrently. Reads use index scans automatically when `where` is `Eq` / `In`
on an indexed field; otherwise full scan.

For the `BatchRequest`/`QueryValue` query format see
[`query/batch/README.md`](query/batch/README.md) and
[`query/README.md`](query/README.md).

## Concurrency

Every manager is cheaply cloneable (`Arc` internals). Concurrent reads and
writes on the same table are supported; the interner is `DashMap`-backed and
`OnceCell` guarantees single initialization.

## Errors

Surfaces `DbError` from `shamir-storage`:

- `DbError::NotFound` — missing key/table/repo/db
- `DbError::DuplicateKey` — unique-index conflict on insert/update
- `DbError::Storage` — backend I/O error
- `DbError::UniqueIndexCreationFailed(name, dup_count, sample)` — building a
  unique index over an existing table that already contains duplicates
