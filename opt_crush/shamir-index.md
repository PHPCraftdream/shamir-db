# shamir-index — оптимизация производительности

## Обзор
Индексы: sorted B-tree, FTS (BM25), functional, vector (HNSW + SIMD).
Уже хорошо оптимизирован: zero-copy posting keys, SIMD distance, fxhash для tokens.

## 🟡 Значимые

### 1. NgramTokenizer: `chars().collect()` → `String` для каждого n-gram
**Файл:** `tokenizer.rs:111-122`
**Сейчас:** `let chars: Vec<char> = lowered.chars().collect();` + `chars[i..i+n].iter().collect()` — alloc на каждый n-gram.
**Решение:** Work directly with byte slices — UTF-8 n-gram по char boundaries без Vec<char> промежуточного.

### 2. Tokenizer trait → dyn dispatch
**Файл:** `tokenizer.rs:11`
**Сейчас:** `Box<dyn Tokenizer>` — virtual dispatch на каждый tokenize().
**Решение:** Enum-dispatch (как FilterNode) — один match вместо vtable.

### 3. FTS postings: batch write
**Проблема:** Каждый posting insert = отдельный Store::set.
**Решение:** Batch insert через Store::transact или set_many.

## 🟢 Уже хорошо
- ✅ SIMD vector distance (AVX-512, AVX2, NEON, scalar fallback)
- ✅ Zero-copy PostingKeyRef
- ✅ FxHash для token hashing
- ✅ Single-allocation posting key build
