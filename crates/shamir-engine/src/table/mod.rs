pub mod buffer_config;
pub mod doctor;
mod filtered_vector;
#[cfg(test)]
pub mod index2_backfill_hook;
pub mod interner_manager;
pub mod persistable;
mod read_exec;
mod read_index_scan;
mod read_planner;
mod read_temporal;
pub mod record_counter;
pub mod record_cow;
#[allow(clippy::module_inception)]
pub mod table;
pub mod table_config;
pub mod table_manager;
mod table_manager_buffer;
mod table_manager_changefeed;
mod table_manager_crud;
mod table_manager_index_mgmt;
mod table_manager_locks;
mod table_manager_replication;
mod table_manager_sorted_index;
mod table_manager_streaming;
mod table_manager_tx_ops;
mod table_manager_validators;
mod tx_scan_overlay;
mod write_exec;
pub(crate) mod write_helpers;

#[cfg(test)]
pub mod tests;

pub use interner_manager::InternerManager;
pub use persistable::{PersistRegistry, Persistable};
pub use record_counter::RecordCounter;
pub use table_config::TableConfig;
pub use table_manager::{table_token_for, TableManager};

#[cfg(test)]
pub use table::Table;
