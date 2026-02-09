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
    # paths can be:
    # - Single string for field index: "email"
    # - Multiple strings for composite index: "category", "subcategory"
    # - Empty array for full value index: []
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

Index paths define which fields are indexed. The `paths` field is a `Vec<String>` where:

- **Single string** = index on a specific field (e.g., `"email"`)
- **Multiple strings** = composite index on multiple fields (e.g., `"category"`, `"subcategory"`)
- **Empty array `[]`** = index the entire value

### Dot Notation for Nested Fields

For nested objects, use dot notation (`.`) to reference fields:

```yaml
# Single field index (flat object)
paths:
  - email

# Nested field index (user.name references "name" field in "user" object)
paths:
  - user.name
  - user.profile.age

# Composite index with nested fields
paths:
  - user.email
  - order.date
```

**Important:** The `.` character separates nested field levels. When indexing `"user.name"`, the database looks for a `user` object containing a `name` field.

### Examples

**Single Field Index:**
```yaml
email_idx:
  paths:
    - email  # Indexes the "email" field only
```

**Composite Index:**
```yaml
category_subcategory_idx:
  paths:
    - category      # First field in composite key
    - subcategory   # Second field in composite key
```

**Full Value Index:**
```yaml
full_value_idx:
  paths: []  # Indexes the entire Map/Set value
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
