# shamir-client — оптимизация производительности

## Обзор
Rust client library: connection pool, request/response, subscriptions.

## Вывод
Client-side — горячие пути на server стороне. Клиент отправляет запросы и ждёт ответы.

## 🟡 Значимые
### 1. Buffer reuse для serialisation
`serde_json::to_vec` / `rmp_serde::to_vec` — alloc на каждый request.
**Решение:** Reuse serialization buffer (thread-local Vec<u8>).

### 2. Connection pooling
Если нет — каждый запрос открывает connection = TLS handshake overhead.
