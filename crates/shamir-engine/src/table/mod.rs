pub mod buffer_config;
pub mod doctor;
pub mod interner_manager;
pub mod persistable;
mod read_exec;
pub mod record_counter;
#[allow(clippy::module_inception)]
pub mod table;
pub mod table_config;
pub mod table_manager;
mod write_exec;

#[cfg(test)]
pub mod tests;

pub use interner_manager::InternerManager;
pub use persistable::{PersistRegistry, Persistable};
pub use record_counter::RecordCounter;
pub use table_config::TableConfig;
pub use table_manager::{table_token_for, TableManager};

#[cfg(test)]
pub use table::Table;
