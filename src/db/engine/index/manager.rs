use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
use crate::db::engine::index::target::IndexTarget;
use crate::db::storage::types::Store;

pub struct IndexManager {
    // Метаданные (уже есть)
    index_target: Arc<RwLock<IndexTarget>>,
    indexes_unique: Arc<RwLock<IndexTarget>>,

    // Флаги быстрого пути
    has_indexes: AtomicBool,
    has_indexes_unique: AtomicBool,

    // Хранилище для персистентности метаданных
    info_store: Arc<dyn Store>,

    // === ЗАГОТОВКИ для будущих RAM индексов ===
    // indexes_ram: Option<RamIndexStore>,
    // indexes_unique_ram: Option<RamIndexStore>,
}
