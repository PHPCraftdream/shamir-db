//! Write-Ahead Log — cross-store atomicity & crash recovery.
//!
//! # Архитектура
//!
//! Каждый Store бэкенд (sled, redb, persy, …) гарантирует
//! durability и atomicity своих собственных writes. Но между
//! `data_store` и `info_store` встроенной транзакционной связки
//! НЕТ — partial crash может оставить data без индексов (или
//! наоборот).
//!
//! WAL это лёгкий журнал намерений: перед началом batch-операции
//! пишем маркер с perечнем затронутых record_id; после успешного
//! завершения — удаляем маркер. На следующем открытии:
//!
//! - Маркеров нет → всё чисто, штатная работа.
//! - Маркер есть → был crash. По маркеру точечно проверяем
//!   консистентность и доводим/откатываем.
//!
//! # Расширяемость
//!
//! Дизайн рассчитан на будущие надстройки:
//!
//! - **Explicit transactions** — пользователь делает `begin()`,
//!   N операций, `commit()`. Маркер открыт между begin и commit;
//!   на crash в любом месте recovery откатит все запланированные
//!   изменения.
//! - **Full-text search** — индексные операции FTS ничем не
//!   отличаются от обычных IndexEntry-операций: тот же
//!   `WalOp::IndexEntryAdded { index_name, key }`. Recovery умеет
//!   проверять любой "index entry" не зная про FTS специфики.
//! - **Schema migrations** — `WalOp::CreateIndex`, `DropIndex` etc.
//!
//! # Storage layout
//!
//! WAL живёт в том же `info_store` под фиксированным префиксом
//! `b"__wal_active_"`. Один маркер = одна KV-запись:
//!
//! ```text
//! key   = b"__wal_active_" || txn_id (8 bytes BE)         = 21 bytes
//! value = bincode(WalEntry { txn_id, started_at_ns, ops }) — содержит
//!         весь набор намеренных операций для этой транзакции
//! ```
//!
//! Маркер пишется ОДНИМ `info_store.set(...)` перед началом batch'а
//! и удаляется ОДНИМ `info_store.remove(...)` после. На backends с
//! buffered durability (sled, redb с `Durability::None`) обе записи
//! проходят через буфер — фактический fsync amortizируется фоном.
//! Performance overhead на happy path близок к нулю.
//!
//! # Recovery scope
//!
//! Один маркер описывает одну batch-операцию (или одну explicit
//! transaction). Recovery работает в O(operations_per_marker), не
//! в O(table_size).

pub mod wal_entry;
pub mod wal_manager;

pub use wal_entry::{WalEntry, WalOp};
pub use wal_manager::WalManager;
