use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{OnceCell, RwLock};
use crate::core::interner::Interner;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::storage::types::Store;

pub struct TableIndexManager {
    // Таблица данных
    data_store: Arc<dyn Store>,

    // Интернер, чтобы кодировать имена полей в u64 и обратно
    interner: Arc<OnceCell<Interner>>,

    // Метаданные (уже есть)
    indexes: Arc<RwLock<IndexInfo>>,
    indexes_unique: Arc<RwLock<IndexInfo>>,

    // Флаги быстрого пути
    has_indexes: AtomicBool,
    has_indexes_unique: AtomicBool,

    // Хранилище для персистентности метаданных
    info_store: Arc<dyn Store>,

    // === ЗАГОТОВКИ для будущих RAM индексов ===
    // indexes_ram: Option<RamIndexStore>,
    // indexes_unique_ram: Option<RamIndexStore>,
}
