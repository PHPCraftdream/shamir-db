pub(crate) mod copy;
pub(crate) mod names;
pub(crate) mod record_id;

#[cfg(test)]
mod tests;

pub(crate) use copy::copy_table_store;
pub(crate) use names::{table_store_names, validate_table_store_name};
pub(crate) use record_id::new_table_store_id;
