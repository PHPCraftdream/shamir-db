# 📘 Chroma MCP Guide

## What is Chroma MCP?

**Chroma MCP** is a server implementing the MCP (Model Context Protocol) that provides access to **Chroma** - a vector database for storing and semantically searching documents.

**Capabilities:**
- Create and manage document collections
- Semantic search (vector search by meaning)
- Metadata filtering
- Store vector representations of text for LLM

---

## 📋 Available MCP Tools

All tools are prefixed with `mcp_chroma-db_`.

### Collection Management

| Tool | Description |
|-------|-------------|
| `chroma_create_collection` | Create a new collection |
| `chroma_list_collections` | Get list of all collections |
| `chroma_get_collection_info` | Get collection information |
| `chroma_get_collection_count` | Document count in collection |
| `chroma_modify_collection` | Change collection name or metadata |
| `chroma_delete_collection` | Delete a collection |
| `chroma_peek_collection` | Preview sample documents |
| `chroma_fork_collection` | Create collection copy |

### Document Operations

| Tool | Description |
|-------|-------------|
| `chroma_add_documents` | Add documents to collection |
| `chroma_query_documents` | Semantic document search |
| `chroma_get_documents` | Get documents by ID |
| `chroma_update_documents` | Update document content/metadata |
| `chroma_delete_documents` | Delete documents from collection |

---

## 🚀 Quick Start

### 1. Create Collection

```rust
// Create collection with description
chroma_create_collection(
    "shamir_knowledge_base",
    metadata={
        "project": "S.H.A.M.I.R.",
        "purpose": "architecture documentation"
    }
)
```

### 2. Add Documents

```rust
// Add documents with metadata
chroma_add_documents(
    collection_name="shamir_knowledge_base",
    documents=[
        "Interning system converts strings to u64 IDs",
        "Storage backend abstraction supports 6 engines",
        "DashMap provides lock-free concurrent operations"
    ],
    ids=["interning-001", "storage-001", "concurrency-001"],
    metadatas=[
        {"category": "interner", "priority": "high"},
        {"category": "storage", "priority": "high"},
        {"category": "concurrency", "priority": "high"}
    ]
)
```

### 3. Semantic Search

```rust
// Search by question meaning
chroma_query_documents(
    collection_name="shamir_knowledge_base",
    query_texts=["How does the interning system work?"],
    n_results=3
)

// Chroma automatically creates embeddings and finds relevant documents
```

### 4. Filter by Metadata

```rust
// Search only documents from specific category
chroma_query_documents(
    collection_name="shamir_knowledge_base",
    query_texts=["storage engines"],
    n_results=5,
    where={"category": "storage"}  // Metadata filter
)
```

### 5. Get Documents by ID

```rust
// Get specific documents
chroma_get_documents(
    collection_name="shamir_knowledge_base",
    ids=["interning-001", "storage-001"]
)
```

---

## 💡 Advanced Examples

### Combined Filtering

```rust
// Search by meaning AND multiple metadata criteria
chroma_query_documents(
    collection_name="shamir_knowledge_base",
    query_texts=["concurrency patterns"],
    where={
        "$and": [
            {"category": "concurrency"},
            {"priority": "high"}
        ]
    },
    n_results=3
)
```

### Document Content Filtering

```rust
// Search documents containing specific words
chroma_query_documents(
    collection_name="shamir_knowledge_base",
    query_texts=["async streaming"],
    where_document={
        "$contains": "spawn_blocking"
    },
    n_results=2
)
```

### Update Documents

```rust
// Update document metadata
chroma_update_documents(
    collection_name="shamir_knowledge_base",
    ids=["interning-001"],
    metadatas=[
        {"category": "interner", "priority": "critical", "updated_at": "2025-02-11"}
    ]
)
```

### Pagination for Retrieval

```rust
// Get documents with pagination
chroma_get_documents(
    collection_name="shamir_knowledge_base",
    where={"category": "storage"},
    limit=5,
    offset=10  // Skip first 10
)
```

---

## 📊 View Collection State

```rust
// Get general information
info = chroma_get_collection_info("shamir_knowledge_base")
// Returns: name, count, collection metadata

// Get document count
count = chroma_get_collection_count("shamir_knowledge_base")
// Returns: number (e.g., 42)

// Preview samples
samples = chroma_peek_collection("shamir_knowledge_base", limit=3)
// Returns: 3 random documents from collection
```

---

## 🎯 Best Practices

### 1. Metadata Structure

```rust
// Good approach: structured metadata
metadatas=[
    {
        "category": "interner",        // Category
        "component": "core",           // Component
        "priority": "high",           // Priority
        "module": "types",            // Module
        "file": "src/core/interner.rs"   // Source file
    }
]
```

### 2. ID Naming Scheme

```rust
// Use descriptive IDs
ids=[
    "arch-001-overview",      // Category-number-name
    "feat-001-interner",      // Type-number-name
    "doc-2025-02-11-001"  // Date-number
]
```

### 3. Document Size

```rust
// Optimal size: 200-500 words
documents=[
    "Interning converts frequently occurring strings to compact u64 IDs..." // ✅ Good
    // ❌ Bad: too long documents (>1000 words)
]
```

### 4. Batch Processing

```rust
// Add documents in batches (10-50 at a time)
chroma_add_documents(
    collection_name="shamir_knowledge_base",
    documents=large_batch,  // up to 50 documents
    ids=corresponding_ids,
    metadatas=corresponding_metadata
)
```

### 5. Filter Before Search

```rust
// Narrow search scope first with filtering
chroma_query_documents(
    collection_name="shamir_knowledge_base",
    query_texts=["storage"],  // General query
    where={"category": "backend"},  // Filter first
    n_results=5
)
```

---

## 🔧 Troubleshooting

### Problem: Documents Not Found

**Solution:**
```rust
// 1. Check if documents were added
count = chroma_get_collection_info("my_collection")

// 2. Try broader queries
chroma_query_documents("my_collection", ["storage"], n_results=10)

// 3. Verify metadata is correct
chroma_get_documents("my_collection", ids=["doc-001"])
```

### Problem: Slow Search

**Solution:**
```rust
// 1. Use filtering to narrow scope
where={"category": "storage"}

// 2. Limit result count
n_results=5

// 3. Reduce document size
// Documents under 300 words search faster
```

### Problem: Duplicate Documents

**Solution:**
```rust
// 1. Use unique IDs
ids=["doc-" + str(uuid.uuid4())]

// 2. Check before adding
existing = chroma_get_documents("my_collection", ids=["doc-001"])
if not existing:
    chroma_add_documents(...)
```

---

## 🤖 Knowledge Protocol for AI Assistants

## 🔍 QUERY PROTOCOL (MANDATORY)

**Rule #1: ALWAYS query in English ONLY**

English queries achieve distances 0.5-1.0 (excellent relevance), while Russian queries achieve 1.1-1.9 (poor relevance). This is due to embeddings being trained primarily on English text.

**Rule #2: Filter by metadata when possible**

Use the `where` parameter with component/module/category/file to narrow search scope and improve relevance.

**Rule #3: Handle null results gracefully**

Some documents may be incomplete or corrupted. Always check for null documents in results.

---

## 📚 KNOWLEDGE ACQUISITION WORKFLOW (MANDATORY)

**Step 1: Search Chroma MCP FIRST**

Before analyzing code, always search the knowledge base:
```rust
chroma_query_documents(
    collection_name="shamir_project",
    query_texts=["topic in English"],
    n_results=3
)
```

**Step 2: IF found**

- Use knowledge from Chroma
- Cite the source document ID and file path
- Verify the information is still current

**Step 3: IF NOT found or incomplete**

a. **Analyze codebase** to understand the topic
   - Read relevant source files
   - Check dependencies and implementations
   - Verify behavior through tests if available

b. **Synthesize findings** into clear, concise documentation
   - Focus on key concepts and patterns
   - Include code examples if relevant
   - Note trade-offs and design decisions

c. **Add new knowledge to Chroma** with proper metadata
   ```rust
   chroma_add_documents(
       collection_name="shamir_project",
       documents=["Synthesized knowledge..."],
       ids=["unique-descriptive-id"],
       metadatas=[{
           "category": "...",
           "component": "...",
           "module": "...",
           "priority": "high",
           "file": "src/..."
       }]
   )
   ```

d. **Cite the addition:** "Added to knowledge base" after discovery

**Step 4: NEVER assume knowledge exists**

Always verify through Chroma first. Don't rely on memory or assumptions about the codebase.

---

## 📚 Usage Examples for S.H.A.M.I.R.

### Architecture Knowledge Base

```rust
// Create collection for architecture
chroma_create_collection(
    "shamir_architecture",
    metadata={"project": "S.H.A.M.I.R.", "type": "architecture"}
)

// Add architecture sections
chroma_add_documents(
    "shamir_architecture",
    documents=[
        "Storage Backend Abstraction Layer with 8 engines...",
        "Interning System for String to u64 compression...",
        "Async Streaming with Rust Streams..."
    ],
    ids=["storage-backend", "interner-system", "async-streaming"],
    metadatas=[
        {"category": "storage", "priority": "high", "component": "storage_engine"},
        {"category": "core", "priority": "high", "component": "interner"},
        {"category": "async", "priority": "medium", "component": "streaming"}
    ]
)
```

### Project Information Retrieval

```rust
// Question: "Which storage backends are supported?"
chroma_query_documents(
    "shamir_architecture",
    ["Which storage backends are supported?"],
    n_results=3
)
// Returns: storage-backend (most relevant)
```

### Assistant Learning

```rust
// Add problem solutions to knowledge base
chroma_add_documents(
    "shamir_solutions",
    documents=[
        "For concurrency issues, use DashMap instead of Mutex",
        "To prevent blocking tokio runtime, use spawn_blocking"
    ],
    ids=["solution-concurrency-001", "solution-async-001"],
    metadatas=[
        {"category": "solutions", "problem": "concurrency", "priority": "high"},
        {"category": "solutions", "problem": "blocking", "priority": "high"}
    ]
)

// Assistant will search solutions in knowledge base
chroma_query_documents(
    "shamir_solutions",
    ["How to fix blocking tokio runtime?"],
    n_results=2
)
```

### Code Change Tracking

```rust
// Add changes to history
chroma_add_documents(
    "shamir_changelog",
    documents=[
        "Added new storage backend: Canopy with LZ4 compression",
        "Migrated interner to use variable-sized keys"
    ],
    ids=["change-2025-02-11-001", "change-2025-02-11-002"],
    metadatas=[
        {"date": "2025-02-11", "type": "feature"},
        {"date": "2025-02-11", "type": "refactor"}
    ]
)

// Search changes for specific date
chroma_query_documents(
    "shamir_changelog",
    ["recent changes"],
    where={"date": "2025-02-11"},
    n_results=10
)
```

---

## 🎓 Summary

**Key Points:**

1. **Semantic Search** - Chroma understands meaning, not just keywords
2. **Metadata** - Powerful tool for filtering and organization
3. **Filters** - `$and`, `$or` for complex queries
4. **Pagination** - `limit` and `offset` for large collections
5. **Identifiers** - Use unique IDs for documents
6. **English Queries** - Always query in English for best results
7. **Knowledge Base First** - Search Chroma before analyzing code

**When to Use Chroma MCP:**

- ✅ Knowledge base for AI assistant
- ✅ Project documentation search
- ✅ Semantic code search
- ✅ Problem solution tracking
- ✅ Change history
- ✅ RAG (Retrieval-Augmented Generation)

---

## 📖 Useful Resources

- [Chroma Documentation](https://docs.trychroma.com/)
- [Chroma GitHub](https://github.com/chroma-core/chroma)
- [Chroma MCP GitHub](https://github.com/chroma-core/chroma-mcp)

---

**Start today!** Create a collection, add documents, and ask questions in natural language (English only for best results).
