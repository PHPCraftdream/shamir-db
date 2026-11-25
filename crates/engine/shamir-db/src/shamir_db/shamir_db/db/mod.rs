mod db_gateway;

#[cfg(test)]
pub(crate) use db_gateway::classify_batch_error;
mod db_management;
mod function;
pub(in crate::shamir_db::shamir_db) use function::FunctionDdlLocks;
pub use function::{FunctionDescription, FunctionExport, FunctionState};
pub(crate) mod schema_management;
pub(in crate::shamir_db) mod validator;
pub(in crate::shamir_db) use validator::{ValidatorRegistrationError, ValidatorTargetAdmission};
mod table_management;
