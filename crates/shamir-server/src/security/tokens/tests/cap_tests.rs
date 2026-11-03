use super::service_tests::{new_token, record, service};
use crate::security::tokens::TokenCreateError;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn token_service_rejects_label_and_database_values_above_shared_caps() {
    let temp = TempDir::new().expect("temporary token store");
    let now = 92_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let mut valid = new_token("x".repeat(128), Some(60));
    valid.databases = Some(vec!["d".repeat(255)]);
    assert!(service.create(valid).await.is_ok());
    assert!(matches!(
        service.create(new_token("x".repeat(129), Some(60))).await,
        Err(TokenCreateError::InvalidInput)
    ));
    let mut too_long_database = new_token("db-limit".into(), Some(60));
    too_long_database.databases = Some(vec!["d".repeat(256)]);
    assert!(matches!(
        service.create(too_long_database).await,
        Err(TokenCreateError::InvalidInput)
    ));
    let mut record_label = record(1);
    record_label.label = "x".repeat(129);
    assert!(record_label.validate_shape().is_err());
    let mut too_long_database_record = record(2);
    too_long_database_record.databases = Some(vec!["d".repeat(256)]);
    assert!(too_long_database_record.validate_shape().is_err());
    let mut too_many_databases = record(3);
    too_many_databases.databases = Some(vec!["db".to_owned(); 65]);
    assert!(too_many_databases.validate_shape().is_err());
}
