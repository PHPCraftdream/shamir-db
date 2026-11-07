use crate::repo::instance::repo_instance::table_store::new_table_store_id;

#[test]
fn generated_ids_are_distinct() {
    assert_ne!(new_table_store_id(), new_table_store_id());
}
