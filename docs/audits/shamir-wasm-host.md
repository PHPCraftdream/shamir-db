# shamir-wasm-host — оптимизация производительности

## Обзор
WASM runtime host: загружает и выполняет user-defined functions как WASM modules.

## 🟡 Значимые
### 1. WASM instantiation overhead
Каждый вызов функции = WASM module instantiation?
**Решение:** Module caching / instance pooling — instantiate один раз, reuse.
### 2. Memory copy in/out
Value conversion Rust → WASM linear memory → Rust.
**Решение:** Shared memory, zero-copy buffers.
