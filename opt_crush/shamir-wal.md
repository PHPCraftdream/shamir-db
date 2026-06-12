# shamir-wal — оптимизация производительности

## Обзор
WAL (Write-Ahead Log) — маркеры транзакций для crash recovery. Store-based (info_store).

## Вывод
Относительно простой крейт. Основной overhead — I/O (Store::set/get), не CPU.

## 🟡 Значимые
### 1. bincode serialize на каждый begin/commit
**Файл:** `wal_manager.rs:72-77`
**Сейчас:** `bincode::serialize(&entry)` на каждый WAL write.
**Решение:** Pre-allocate buffer, reuse across calls. Или написать прямой binary encode без serde overhead.

### 2. recovery: full scan info_store
**Проблема:** Recovery сканирует все ключи info_store для поиска WAL маркеров.
**Решение:** Prefix scan по WAL-префиксу вместо full scan.
