use crate::{FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

struct Replacement(i64);

#[async_trait]
impl ShamirFunction for Replacement {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(self.0))
    }
}

#[test]
fn replace_if_artifact_id_replaces_matching_artifact_and_preserves_metadata() {
    let registry = FunctionRegistry::new();
    registry
        .register_with_artifact_id("entry", Arc::new(Replacement(1)), Some("artifact-a".into()))
        .unwrap();
    let before = registry.capture("entry").unwrap();
    let replacement: Arc<dyn ShamirFunction> = Arc::new(Replacement(2));

    assert!(registry.replace_if_artifact_id("entry", Arc::clone(&replacement), Some("artifact-a")));

    let after = registry.capture("entry").unwrap();
    assert_eq!(after.identity(), before.identity());
    assert_eq!(after.artifact_id(), Some("artifact-a"));
    assert_eq!(after.is_database_builtin(), before.is_database_builtin());
    assert!(Arc::ptr_eq(after.artifact(), &replacement));
}

#[test]
fn replace_if_artifact_id_mismatch_leaves_entry_unchanged() {
    let registry = FunctionRegistry::new();
    registry
        .register_with_artifact_id("entry", Arc::new(Replacement(1)), Some("artifact-a".into()))
        .unwrap();
    let before = registry.capture("entry").unwrap();

    assert!(!registry.replace_if_artifact_id(
        "entry",
        Arc::new(Replacement(2)),
        Some("artifact-b")
    ));

    assert!(before.same_as(&registry.capture("entry").unwrap()));
}

#[test]
fn replace_if_artifact_id_does_not_insert_vacant_name() {
    let registry = FunctionRegistry::new();

    assert!(!registry.replace_if_artifact_id("missing", Arc::new(Replacement(1)), None));

    assert!(!registry.contains("missing"));
    assert!(registry.is_empty());
}

#[test]
fn replace_if_artifact_id_matches_legacy_entry_with_none() {
    let registry = FunctionRegistry::new();
    registry
        .register("entry", Arc::new(Replacement(1)))
        .unwrap();
    let before = registry.capture("entry").unwrap();
    let replacement: Arc<dyn ShamirFunction> = Arc::new(Replacement(2));

    assert!(registry.replace_if_artifact_id("entry", Arc::clone(&replacement), None));

    let after = registry.capture("entry").unwrap();
    assert_eq!(after.identity(), before.identity());
    assert_eq!(after.artifact_id(), None);
    assert_eq!(after.is_database_builtin(), before.is_database_builtin());
    assert!(Arc::ptr_eq(after.artifact(), &replacement));
}
