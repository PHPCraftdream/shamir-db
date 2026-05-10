# API Layer

**Status:** Under Construction

This module will provide external APIs for S.H.A.M.I.R. database.

## Current Architecture

All database operations are accessed through the **Batch API** — a unified JSON-based interface.

### Entry Point

```rust
use shamir_db::ShamirDb;
use shamir_db::SystemStoreConfig;
use shamir_db::query::BatchRequest;

// Initialize database
let db = ShamirDb::init(SystemStoreConfig::InMemory).await?;

// Create a database
db.create_db("myapp").await;

// Execute batch request
let request: BatchRequest = serde_json::from_str(json_str)?;
let response = db.execute("myapp", &request).await?;
```

### Batch Request Format

Every request goes through `BatchRequest` with a mandatory `id` field:

```json
{
  "id": "req-001",
  "queries": {
    "users": {
      "from": "users",
      "where": { "op": "eq", "field": ["status"], "value": "active" }
    }
  }
}
```

**Important:** Field paths are arrays (`["user", "address", "city"]`), not dot-separated strings.

### Table References

Tables are referenced via `TableRef { repo, table }`:

```json
// Simple: default repo "main"
"from": "users"

// With repo qualifier
"from": ["hot", "sessions"]
```

### Operations via Batch API

| Operation | JSON Key | Description |
|-----------|----------|-------------|
| Read (SELECT) | `from` | Query records |
| Insert | `insert_into` | Insert records |
| Update | `update` | Update records |
| Set (Upsert) | `set` | Update or create |
| Delete | `delete_from` | Delete records |
| Create DB | `create_db` | Create database |
| Drop DB | `drop_db` | Drop database |
| Create Repo | `create_repo` | Create repository |
| Drop Repo | `drop_repo` | Drop repository |
| Create Table | `create_table` | Create table |
| Drop Table | `drop_table` | Drop table |
| Create Index | `create_index` | Create index |
| Drop Index | `drop_index` | Drop index |
| List | `list` | List databases/repos/tables/indexes |

### Response Format

```json
{
  "id": "req-001",
  "results": {
    "users": {
      "records": [...],
      "stats": { "records_scanned": 100, "records_returned": 5, "execution_time_us": 1234 }
    }
  },
  "execution_plan": [["users"]],
  "execution_time_us": 1500
}
```

## Planned Components

### Network Protocols
- **REST API** (Axum)
- **gRPC** (Tonic)
- **WebSocket** (real-time subscriptions)

### REST API Design

```
POST   /batch                 -> Execute batch request
POST   /query                 -> Execute single query (convenience)
```

All operations go through the Batch API, so REST endpoints are thin wrappers.

## Implementation Status

- [x] Batch API (core engine)
- [x] Read queries (SELECT with filters, ordering, pagination)
- [x] Write operations (Insert, Update, Set, Delete)
- [x] Admin/DDL operations (create/drop db/repo/table/index, list)
- [x] SystemStore for persistent metadata
- [x] ShamirDb::init(SystemStoreConfig) entry point
- [ ] REST API (Axum)
- [ ] gRPC (Tonic)
- [ ] WebSocket (Tokio-tungstenite)
- [ ] Authentication/Authorization (designed, see auth/README.md)
- [ ] Rate limiting
- [ ] Request validation middleware

## Design Considerations

### Async Streaming
Will utilize the underlying `iter_stream()` for memory-efficient responses:
- Server-Sent Events (SSE) for streaming
- Chunked transfer encoding
- Backpressure support

### Error Handling
HTTP status mapping:
- `200` - Success
- `201` - Created
- `400` - Bad request (BatchError)
- `404` - Not found
- `500` - Internal error
- `503` - Service unavailable

### Concurrency
- Per-request table resolution via `TableResolver` trait
- `AdminExecutor` trait for DDL operations
- Request timeout configuration

## Integration with Engine

```rust
// ShamirDb manages the full hierarchy:
// ShamirDb
//   -> SystemStore (persistent metadata)
//   -> DbInstance (per database)
//     -> RepoInstance (per repository)
//       -> TableManager (per table)

let shamir = ShamirDb::init(SystemStoreConfig::Redb("./data".into())).await?;
let db = shamir.create_db("production").await;

// Execute batch
let response = shamir.execute("production", &request).await?;
```

## Timeline

**Phase 1** (Complete):
- [x] Core database engine
- [x] Storage abstraction (7 engines + cached wrapper)
- [x] Table API with interning
- [x] Batch query system with read/write/admin ops
- [x] SystemStore for metadata persistence
- [x] Filter evaluation (all operators implemented)

**Phase 2** (Next):
- [ ] REST API
- [ ] Basic HTTP endpoints
- [ ] Streaming support

**Phase 3** (Future):
- [ ] gRPC
- [ ] WebSocket subscriptions
- [ ] Authentication middleware
