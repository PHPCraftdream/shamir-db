//! Shared integration-test helper: mint a field id through the admitted path.

use shamir_collections::TMap;
use shamir_engine::repo::RepoInstance;
use shamir_engine::table::TableManager;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Intern `name` and return its id. `touch_ind` is a dictionary WRITE, so it
/// goes through `intern_query_value_admitted` under a `RepoWritePermit`
/// (the raw `interner()` handle is crate-private).
pub async fn field_id(repo: &RepoInstance, tbl: &TableManager, name: &str) -> u64 {
    let permit = repo.write_domain().admit().await.unwrap();
    let mut probe: TMap<String, QueryValue> = TMap::default();
    probe.insert(name.to_string(), QueryValue::Null);
    let inner = tbl
        .intern_query_value_admitted(&permit, &QueryValue::Map(probe))
        .await
        .unwrap();
    let InnerValue::Map(m) = inner else {
        unreachable!("intern_query_value_admitted of a map returns a map")
    };
    m.keys().next().expect("one field key").id()
}
