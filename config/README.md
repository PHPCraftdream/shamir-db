# Configuration Guide

## Overview

SHAMIR DB configuration is stored in YAML format and defines repositories, tables, and indexes.

## File Location

Default config location: `config/database.yaml`

## Example Configuration

```yaml
data_dir: "./data"
repos:
  default:
    storage_type: Redb
    ram_cached: true
    tables:
      users:
        indexes:
          email_idx:
            paths:
              - email
        indexes_unique:
          user_id_idx:
            paths:
              - id
```

## Configuration Structure

### DbConfig

```yaml
data_dir: "/path/to/data"
repos:
  <repo_name>: RepoConfig
```

### RepoConfig

```yaml
storage_type: Canopy | Fjall | Cached | Memory | Nebari | Persy | Redb | Sled
ram_cached: true | false
tables:
  <table_name>: TableConfig
```

### TableConfig

```yaml
indexes:
  <index_name>:
    paths:
      - field.path
      - nested.field.path
indexes_unique:
  <unique_index_name>:
    paths:
      - field.path
```

## Storage Engines

- **Canopy**: LSM-based storage engine
- **Fjall**: High-performance key-value store
- **Cached**: In-memory caching layer
- **Memory**: Pure in-memory storage
- **Nebari**: B+ tree based storage
- **Persy**: Multi-mode persistent storage
- **Redb**: Embedded key-value store with transactions
- **Sled**: Modern embedded database

## Index Paths

Index paths use dot notation for nested fields:

```yaml
# Simple field
paths:
  - email

# Nested field
paths:
  - user.profile.age

# Multi-field index
paths:
  - category
  - subcategory
```

## Validation Rules

1. `data_dir` must be specified
2. At least one repository must exist
3. Each repository must have at least one table
4. Each table must have at least one index (regular or unique)
5. Each index must have at least one path

## Loading Configuration

```rust
use shamir_db::db::engine::dispatcher::ConfigLoader;

// Load from file
let config = ConfigLoader::load_from_file("config/database.yaml")?;

// Save to file
ConfigLoader::save_to_file("config/database.yaml", &config)?;
```

## Hot Reload

Configuration can be updated at runtime:

1. Load new config via Web API
2. Update Dispatcher with new repo/table configuration
3. Save to file atomically (temp + rename)

See `config/database.yaml.example` for a complete example.
