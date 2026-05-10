use crate::db::engine::table::TableConfig;

#[test]
fn test_table_config_new() {
    let config = TableConfig::new("users");
    assert_eq!(config.name, "users");
    assert!(!config.enable_indexes);
}

#[test]
fn test_table_config_with_indexes() {
    let config = TableConfig::new("products").with_indexes();
    assert_eq!(config.name, "products");
    assert!(config.enable_indexes);
}

#[test]
fn test_table_config_clone() {
    let config1 = TableConfig::new("test").with_indexes();
    let config2 = config1.clone();

    assert_eq!(config1.name, config2.name);
    assert_eq!(config1.enable_indexes, config2.enable_indexes);
}
