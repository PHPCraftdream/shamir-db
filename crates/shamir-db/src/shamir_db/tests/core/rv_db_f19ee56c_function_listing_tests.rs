use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::types::value::QueryValue;

#[tokio::test]
async fn rv_db_f19ee56c_list_functions_with_kind_loads_catalogue_once() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    for name in ["fn_a", "fn_b", "fn_c"] {
        shamir
            .register_fn(name, false, |_, _, _| async { Ok(QueryValue::Null) })
            .unwrap();
    }

    let store = shamir.system_store();
    store.reset_function_catalogue_reads();
    let listed = shamir.list_functions_with_kind().await.unwrap();
    assert_eq!(store.function_catalogue_read_count(), 1);
    for name in ["fn_a", "fn_b", "fn_c"] {
        assert!(listed
            .iter()
            .any(|(listed_name, kind)| listed_name == name && *kind == ArtifactKind::Wasm));
    }
}
