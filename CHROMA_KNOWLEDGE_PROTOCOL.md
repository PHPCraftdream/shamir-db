# Chroma Knowledge Base Protocol

## 📋 Document Addition Protocol

### Rule #1: ID Uniqueness Check (MANDATORY)

Before adding any document, **ALWAYS** check if ID already exists:

```rust
// Step 1: Check ID existence
existing = chroma_get_documents(
    collection_name="shamir_project",
    ids=["proposed-id"]
)

// Step 2: If exists, skip or update
if existing:
    // OPTION A: Skip (default)
    log("Document with ID 'proposed-id' already exists, skipping")

    // OPTION B: Update (if content changed)
    if document_content_different(existing[0], new_document):
        chroma_update_documents(
            collection_name="shamir_project",
            ids=["proposed-id"],
            documents=[new_content],
            metadatas=[new_metadata]
        )
    return

// Step 3: Add only if ID doesn't exist
chroma_add_documents(
    collection_name="shamir_project",
    documents=[new_document],
    ids=["proposed-id"],
    metadatas=[metadata]
)
```

### Rule #2: Semantic Duplicate Check (RECOMMENDED)

Before adding new content, search for semantically similar documents:

```rust
// Step 1: Search for similar documents
similar = chroma_query_documents(
    collection_name="shamir_project",
    query_texts=["summary of document content"],
    n_results=3
)

// Step 2: Check distance (lower = more similar)
for doc in similar['documents']:
    if doc['distance'] < 0.3:  // Very similar (same topic)
        // Check if it's truly duplicate or just related
        if content_is_duplicate(new_doc, doc):
            log("Semantic duplicate found with distance: {doc['distance']}")
            return  // Skip adding
        else:
            // Update existing document instead
            chroma_update_documents(...)
            return
```

### Rule #3: ID Naming Convention (MANDATORY)

Use structured IDs following this pattern:

```rust
// Format: {module}-{entity}-{number}-{variant}

// Examples:
"db-storage-layer-001-overview"      // Module + entity + number + description
"types-value-001-enum-definition"     // Module + type + number + variant
"core-interner-002-persistence"      // Module + component + number + aspect
"api-commands-001-put-get"          // Module + feature + number + scope

// Increment number for new documents on same entity
// Use variant for different aspects of same entity
```

### Rule #4: Metadata Standardization (MANDATORY)

All documents MUST include these metadata fields:

```rust
metadatas = [{
    "category": "overview|types|architecture|api|protocol",  // Required
    "component": "value|interner|storage|table|dispatcher",    // Required
    "module": "types|core|db|codecs|api",                    // Required
    "priority": "high|medium|low",                            // Required
    "file": "src/path/to/file.rs",                           // Recommended
    "created_at": "2025-02-11",                              // Recommended
    "version": "1.0"                                         // Recommended for updates
}]
```

### Rule #5: Document Versioning (RECOMMENDED)

When updating existing knowledge:

```rust
// Step 1: Check if document exists
existing = chroma_get_documents(collection_name, ids=[id])

// Step 2: Compare versions/content
if existing:
    old_version = existing[0]['metadata'].get('version', '1.0')
    new_version = increment_version(old_version)

    // Step 3: Update with new version
    chroma_update_documents(
        collection_name="shamir_project",
        ids=[id],
        documents=[updated_content],
        metadatas=[{
            **old_metadata,
            "version": new_version,
            "updated_at": "2025-02-11"
        }]
    )
```

## 🔄 Adding Knowledge Workflow

### Before Adding

1. **Search Chroma FIRST** for similar documents
2. **Check ID existence** with `chroma_get_documents(ids=[...])`
3. **Search semantically** with `chroma_query_documents`
4. **Decide**: Add new OR Update existing OR Skip

### Adding New Document

```rust
id = generate_id(module, entity, aspect)

// Check if exists
if chroma_get_documents(collection_name="shamir_project", ids=[id]):
    log(f"Document {id} already exists")
    return

// Search semantically
similar = chroma_query_documents(
    "shamir_project",
    [summary],
    n_results=2
)

// Check for duplicates
if similar['distances'][0] < 0.3:
    log("Semantic duplicate found")
    return

// Add with standard metadata
chroma_add_documents(
    collection_name="shamir_project",
    documents=[content],
    ids=[id],
    metadatas=[{
        "category": "types",
        "component": "value",
        "module": "types",
        "priority": "high",
        "file": "src/types/value.rs",
        "created_at": today()
    }]
)
```

### Updating Existing Document

```rust
// Get existing
existing = chroma_get_documents(collection_name, ids=[id])

// Check if content changed
if content_same(existing[0]['document'], new_content):
    return  // No update needed

// Update
chroma_update_documents(
    collection_name="shamir_project",
    ids=[id],
    documents=[new_content],
    metadatas=[{
        **existing[0]['metadata'],
        "version": increment_version(existing[0]['metadata']['version']),
        "updated_at": today()
    }]
)
```

## 🎯 Best Practices

### DO ✅
- Always check ID existence before adding
- Search for semantically similar documents
- Use structured IDs (module-entity-number-aspect)
- Include all required metadata fields
- Update existing documents instead of duplicating
- Use semantic search to find related knowledge

### DON'T ❌
- Never add documents without ID check
- Never add duplicate content with different IDs
- Never skip metadata fields
- Never assume document doesn't exist

## 📊 Current Collection State

```
Collection: shamir_project
Document Count: 118
ID Format: Mixed (need standardization)
Metadata: Partially standardized
Duplicates: Some detected
```

## 🔧 Migration Plan

### Phase 1: Clean Duplicates
1. Identify duplicates by semantic search
2. Remove redundant documents
3. Keep most recent/complete version

### Phase 2: Standardize IDs
1. Generate new structured IDs
2. Update document IDs (chroma doesn't support rename, will delete and re-add)
3. Update all references

### Phase 3: Complete Metadata
1. Add missing fields to all documents
2. Add created_at timestamps
3. Add version numbers

### Phase 4: Enforce Protocol
1. All future additions follow rules
2. Automated checks in workflow
3. Regular duplicate detection scans
