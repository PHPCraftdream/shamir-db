בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

**S.H.A.M.I.R. Database**

**S** - Secure (Безопасная - Rust)  
**H** - High-performance (Высокопроизводительная)  
**A** - Asynchronous (Асинхронная)  
**M** - Modular (Модульная - WASM)  
**I** - Interconnected (Взаимосвязанное - Chat/P2P)  
**R** - Repository (Хранилище)  

---

## 🧠 CONTEXT FOR AGENTS (КОНТЕКСТ ДЛЯ АГЕНТОВ)

Ты — инженер-разработчик, работающий над проектом **S.H.A.M.I.R.**
Это production-level, однофайловая (standalone binary), децентрализованная база данных на Rust.

### 🎯 Глобальные цели
1.  **Self-contained:** Один бинарный файл (<50MB). Никаких внешних зависимостей.
2.  **Hybrid Storage:** Данные — это MessagePack, но ключи полей (Schema) интернируются в числа (`u64`) для скорости и сжатия.
3.  **WASM-First:** Логика БД — это WASM модули.
4.  **Reliability:** WAL (Write Ahead Log), Checksums, Crash safety.

---

## 🛡️ PROTOCOL OF DEVELOPMENT (TDD)

1.  **🔴 RED:** Напиши тест (`tokio::test`), который падает или не компилируется.
2.  **🟢 GREEN:** Напиши минимальный код для прохождения теста.
3.  **🔵 REFACTOR:** Улучши код, сохраняя "зеленый" статус.

## 🛡️ Правила
**Важно:** Используй `arc_swap`, `dashmap`, `tokio::task::spawn_blocking` для конкурентности. Избегай `Mutex` где возможно.
**Важно:** не меняй код, который не относится к задаче.
**Важно:** не меняй комментарии, которые не относятся к задаче.
**Важно:** делай только точечные изменения.
**Важно:** в тестах JSON всегда должен быть форматированным, многотрочным.
**Важно:** mod.rs хранятся только экспорты - не пиши там ничего, кроме экспортов. типы складывай в соседнем файле.


# Обработка ошибок в Rust
- Используй `Result<T, E>` вместо исключений
- Оператор `?` извлекает `Ok` или пробрасывает `Err` вверх
- `Box<dyn Error>` для разных типов ошибок
- `thiserror` для своих enum-ов ошибок с `#[from]`
- `anyhow` для приложений, `thiserror` для библиотек
- Избегай `panic!`

---

## 📍 PHASE 1: THE FOUNDATION (ТИПЫ И ИНТЕРНИРОВАНИЕ)

Мы создаем "язык" общения и механизм сжатия ключей.

### 📋 Task List

#### Task 1.1: Project Setup
*   Создать `Cargo.toml` (workspace).
*   Dependencies: `tokio`, `serde`, `rmp-serde`, `dashmap`, `crc32fast`, `thiserror`, `anyhow`.

#### Task 1.2: Command Protocol (TDD)
*   Создать `enum Command` (Put, Get, Del, Execute).
*   Создать `struct Request` и `Response`.
*   **Constraint:** Все структуры должны сериализоваться через `rmp-serde` (MessagePack).
*   **TEST:** Round-trip сериализация (Struct -> Bytes -> Struct).

#### Task 1.3: The Interner (Memory)
*   **Goal:** Двунаправленный маппинг `String <-> u64`.
*   **Constraint:** Использовать `u64` для ID, но помнить, что на диске они будут сжаты.
*   **Components:** `DashMap<String, u64>` и `DashMap<u64, String>`.
*   **TEST:** Конкурентный доступ (много потоков читают/пишут одни и те же строки).

#### Task 1.4: Persisted Interner (WAL)
*   **Goal:** Сохранять новые ключи на диск, чтобы пережить рестарт.
*   **Format:** Append-Only файл.
    *   Запись: `[len: varint] [id: varint] [key_bytes] [crc32: u32]`.
    *   *Примечание:* Использовать `unsigned-varint` или аналог для записи чисел, чтобы экономить место (ID 1 занимает 1 байт, а не 8).
*   **Safety:** `fsync` (sync_data) после каждой записи нового ключа.
*   **Recovery:** При старте читать файл, проверять CRC32. Если последняя запись битая — обрезать файл (truncate).
*   **TEST:**
    1.  Записать ключи, рестарт, проверить чтение.
    2.  Simulate Corruption: записать, обрезать файл на 1 байт с конца, рестарт -> база должна восстановиться (отбросив битую запись) и работать дальше.