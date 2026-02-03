# API Layer

**Status:** 🚧 Under Construction

This module will provide external APIs for S.H.A.M.I.R. database.

## Planned Components

### Network Protocols
- **REST API** (HTTP/JSON)
- **gRPC** (high-performance RPC)
- **WebSocket** (real-time subscriptions)

### Operations
```rust
// Planned API structure

// Tables
POST   /tables                 → Create table
GET    /tables                 → List tables
GET    /tables/{name}          → Get table info
DELETE /tables/{name}          → Drop table

// Records
POST   /tables/{name}/records  → Insert record
GET    /tables/{name}/records/{id} → Get record
PUT    /tables/{name}/records/{id} → Update record
DELETE /tables/{name}/records/{id} → Delete record
GET    /tables/{name}/records  → List/stream records

// Queries (future)
POST   /tables/{name}/query    → Execute query
```

### Request/Response Examples

**Insert Record:**
```http
POST /tables/users/records
Content-Type: application/json

{
  "name": "Alice",
  "age": 30,
  "email": "alice@example.com"
}

Response: 201 Created
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created_at": "2026-02-03T18:00:00Z"
}
```

**Stream Records:**
```http
GET /tables/users/records?batch_size=100
Accept: application/json-stream

Response: 200 OK
Content-Type: application/json-stream

{"id": "...", "data": {...}}
{"id": "...", "data": {...}}
...
```

## Implementation Status

- [ ] REST API (Actix-web/Axum)
- [ ] gRPC (Tonic)
- [ ] WebSocket (Tokio-tungstenite)
- [ ] Authentication/Authorization
- [ ] Rate limiting
- [ ] Request validation

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
- `400` - Bad request
- `404` - Not found
- `500` - Internal error
- `503` - Service unavailable

### Concurrency
- Per-request table clones
- Shared connection pool
- Request timeout configuration

## Integration with Table Engine

```rust
// Pseudo-code for REST handler

async fn insert_record(Path(table_name): Path<String>,
                        Json(value): Json<UserValue>,
                        repo: State<Arc<SledRepo>>)
    -> Result<Json<InsertResponse>, StatusCode>
{
    let table = repo.table_get(&table_name)?;

    let id = table.insert(value).await?;

    Ok(Json(InsertResponse { id }))
}

async fn stream_records(Path(table_name): Path<String>,
                        Query(params): Query<StreamParams>,
                        repo: State<Arc<SledRepo>>)
    -> Response<Body>
{
    let table = repo.table_get(&table_name)?;
    let stream = table.list_stream(params.batch_size);

    // Convert to SSE stream
    let sse_stream = stream.map(|batch| {
        match batch {
            Ok(records) => {
                for (id, value) in records {
                    yield sse_event(id, value);
                }
            }
            Err(e) => yield sse_error(e),
        }
    });

    Response::new(Body::wrap_stream(sse_stream))
}
```

## Timeline

**Phase 1** (Current):
- ✅ Core database engine
- ✅ Storage abstraction
- ✅ Table API

**Phase 2** (Next):
- [ ] REST API
- [ ] Basic CRUD operations
- [ ] Streaming support

**Phase 3** (Future):
- [ ] gRPC
- [ ] WebSocket subscriptions
- [ ] Query language
- [ ] Authentication

## Contributing

API layer implementation is open for contribution once core features stabilize.
