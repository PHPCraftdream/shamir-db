# S.H.A.M.I.R. Database

**S**ecure, **H**igh-performance, **A**synchronous, **M**odular, **I**nterconnected, **R**epository

A modern, modular embedded database written in Rust with pluggable storage backends, key interning, and async streaming.

## 🎯 Project Status

**Version:** 0.1.0 (Alpha)

**Current Features:**
- ✅ 6 storage backend abstraction (Sled, Redb, Fjall, Nebari, Persy, Canopy) — feature-gated
- ✅ Key interning system (strings → u64 for memory efficiency)
- ✅ Async streaming with batch generators
- ✅ High-level Table engine with UserValue/InnerValue transformations
- ✅ Multi-database / multi-repo system store with durable metadata
- ✅ JSON-based Batch query API: WHERE / SELECT / GROUP BY / ORDER BY / LIMIT / pagination
- ✅ Cross-query references via `{"$query": "@alias[].field"}`
- ✅ Admin DDL (Create/Drop Db / Repo / Table / Index, List)
- ✅ Auth ops (Create/Drop User / Role, Grant / Revoke)
- ✅ Secondary indexes + query planner
- ✅ Wire protocol: TLS 1.3 + SCRAM-Argon2id + Ed25519 channel binding
- ✅ Transports: TCP, WebSocket (native + browser)
- ✅ Session resumption tickets (AES-256-GCM, anti-downgrade, multi-device families)
- ✅ Audit log with HMAC chain
- ✅ ACID Transactions: single-batch Snapshot Isolation (SI) + Serializable (SSI), crash recovery via WAL V2
- ✅ MVCC versioned reads, history store GC, max-tx-lifetime enforcement
- ✅ 1452+ workspace tests

**Planned Features (see [docs/roadmap/ROADMAP.md](docs/roadmap/ROADMAP.md)):**
- 🔜 Browser WASM client (Argon2id in Web Worker)
- 🔜 SQL-like query frontend (today: structured JSON queries)
- 🔜 QUIC transport
- 🔜 Post-quantum hybrid handshake

## 🏗️ Architecture

Cargo workspace, layered from foundation upward:

```
crates/
├── shamir-types/         # Value model, identifiers, codecs, interner
├── shamir-storage/       # Store/Repo traits + 7 backend impls (feature-gated)
├── shamir-engine/        # Table engine + query language + batch executor
├── shamir-query-types/   # Query DTOs (filter, read, write, batch — wire-shareable)
├── shamir-db/            # Top-level facade: SystemStore, ShamirDb::execute(batch)
├── shamir-connect/       # Wire protocol: SCRAM-Argon2id + envelopes + session
├── shamir-transport-tcp/ # TLS 1.3 over TCP
├── shamir-transport-ws/  # WebSocket (native WSS + browser WSS)
├── shamir-transport-udp/ # UDP framing (experimental)
└── shamir-server/        # ServerLauncher: bootstrap, listeners, RBAC, audit
```

Documentation:
- **[docs/architecture/LOGIC_FLOW.md](docs/architecture/LOGIC_FLOW.md)** — view from above: how a request travels through the crates
- **[docs/architecture/ARCHITECTURE.md](docs/architecture/ARCHITECTURE.md)** — DB internals (storage, types, indexes)
- **[docs/client-server-protocol-spec/](docs/client-server-protocol-spec/)** — wire protocol (auth, session, transports)
- **[docs/roadmap/](docs/roadmap/)** — production hardening, server plan, feature roadmap
- **[docs/ops/](docs/ops/)** — capacity planning, perf tuning

## 🚀 Quick Start

```rust
use shamir_db::storage::storage_sled::SledRepo;
use shamir_db::engine::Table;
use shamir_db::types::value::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open database
    let repo = SledRepo::new("./my_db")?;

    // Get/create table
    let table = repo.table_get("users")?;

    // Insert record
    let user = Value::Object(map![
        ("name".into(), Value::Str("Alice".into())),
        ("age".into(), Value::Int(30))
    ]);
    let id = table.insert(user).await?;

    // Stream all records (memory-efficient!)
    let mut stream = table.list_stream(100); // 100 records per batch
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        for (id, record) in batch {
            println!("{}: {:?}", id, record);
        }
    }

    Ok(())
}
```

## 📦 Storage Backends

| Backend | Status | Notes |
|---------|--------|-------|
| **Sled** | ✅ Stable | Pure Rust, battle-tested |
| **Redb** | ✅ Stable | Modern, MVCC-based |
| **Fjall** | ✅ Stable | LSM-tree, high write throughput |
| **Nebari** | ✅ Stable | BlueDB successor |
| **Persy** | ✅ Stable | ACID transactions |
| **Canopy** | ✅ Stable | B+-tree, LZ4 compression |

## 🧪 Testing

```bash
# Full workspace test sweep (1178+ tests, ~90s)
bash scripts/test-all.sh

# Specific crate
cargo test -p shamir-engine
cargo test -p shamir-server
```

## 📊 Performance

- **Interning**: ~70% memory reduction for string-heavy data
- **Streaming**: Constant memory usage regardless of dataset size
- **Batch size**: Tunable for optimal throughput

## 🤝 Contributing

This is an alpha-stage research project. Architecture decisions are still evolving.

## 📝 License

[Your License Here]

---

**Made with ❤️ in Rust**
