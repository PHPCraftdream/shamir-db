use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_types::mpack;
use shamir_types::types::common::TSet;
use shamir_types::types::value::{QueryValue, Value};

use super::interner_cache_tests::with_client;
use crate::Client;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_set_of_maps_insert_and_read() {
    with_client(|client: Client| async move {
        let mut set = TSet::<QueryValue>::default();
        set.insert(mpack!({ "cold_nested": 37 }));
        let mut batch = Batch::new();
        batch.insert(
            "inserted",
            insert("items").row(mpack!({ "group": @(Value::Set(set)) })),
        );
        client
            .execute_with_touch("ic", batch.build())
            .await
            .expect("insert set of maps");
        assert!(client.resolve_field("ic", "main", "cold_nested").is_some());
        let mut read = Batch::new();
        read.query("rows", Query::from("items"));
        let response = client
            .execute("ic", read.build())
            .await
            .expect("read inserted row");
        let row = response.results["rows"].records[0].as_value();
        let items = row.get("group").unwrap().as_array().unwrap();
        assert_eq!(items[0].get("cold_nested"), Some(&Value::Int(37)));
        client
    })
    .await;
}
