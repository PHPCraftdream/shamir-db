# shamir-connect — оптимизация производительности

## Обзор
Аутентификация: SCRAM handshake, Argon2 KDF, session tickets, key rotation.
Crypto-heavy — но однократно на connection, не в hot loop.

## 🟡 Значимые
### 1. Argon2 — CPU-intensive KDF
**Проблема:** Argon2 по design медленный (memory-hard). ~100-500ms на hash.
**Решение:** Параллелизовать через semaphore (уже есть `argon2_semaphore`). ✅

### 2. SCRAM round-trips
**Проблема:** 2 RTT для handshake.
**Решение:** Optimistic bootstrap (1 RTT) — уже реализован. ✅

## Вывод
Уже хорошо оптимизирован. Crypto operations по definition не «быстрые» — это security feature.
