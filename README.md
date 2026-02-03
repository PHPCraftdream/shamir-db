# S.H.A.M.I.R. Database

**S**ecure, **H**igh-performance, **A**synchronous, **M**odular, **I**nterconnected, **R**epository

A modern, modular embedded database written in Rust with pluggable storage backends, key interning, and async streaming.

## 🎯 Project Status

**Version:** 0.1.0 (Alpha)

**Current Features:**
- ✅ 6 storage backend abstraction (Sled, Redb, Fjall, Nebari, Persy, Canopy)
- ✅ Key interning system (strings → u64 for memory efficiency)
- ✅ Async streaming with batch generators (PHP-style)
- ✅ High-level Table engine with UserValue/InnerValue transformations
- ✅ Comprehensive test coverage (91+ tests)

**Planned Features:**
- 🔜 Internal indexes for fast lookups
- 🔜 Secondary indexes
- 🔜 Query planner
- 🔜 SQL-like query language

## 🏗️ Architecture

```
src/
├── api/           # Public API layer (REST, gRPC, etc.)
├── codecs/        # Serialization/deserialization
├── core/          # Core abstractions (Interner, Transform)
├── db/            # Database layer
│   ├── engine/    # Table engine with interning
│   └── storage/   # Storage backend implementations
└── types/         # Type definitions (Value, RecordId, etc.)
```

## 🚀 Quick Start

```rust
use shamir_db::db::storage::storage_sled::SledRepo;
use shamir_db::db::engine::Table;
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
# Run all tests
cargo test

# Run specific storage tests
cargo test test_sled
cargo test test_redb

# Run streaming tests
cargo test iter_stream
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
