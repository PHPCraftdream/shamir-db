use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

/// List databases.
pub fn list_databases() -> BatchOp {
    BatchOp::List(ListOp::Databases)
}
/// List repos in the current database.
pub fn list_repos() -> BatchOp {
    BatchOp::List(ListOp::Repos)
}
/// List users.
pub fn list_users() -> BatchOp {
    BatchOp::List(ListOp::Users)
}
/// List all registered validators (catalogue-wide: id + name + bound tables).
///
/// NOTE: This is different from `list_validators(table)` which lists
/// per-table bindings via `ListValidatorsOp`.
pub fn list_all_validators() -> BatchOp {
    BatchOp::List(ListOp::Validators)
}
