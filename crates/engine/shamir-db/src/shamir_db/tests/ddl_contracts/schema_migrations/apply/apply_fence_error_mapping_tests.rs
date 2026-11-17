//! CI pins for the step-2a fence-refusal mapping (`map_fence_error`).
//!
//! The step-2a store raises fence refusals as untyped
//! `DbError::Validation(String)` messages (see
//! `system_store/catalogue/schema_migrations/journal.rs`), so the apply arm
//! maps them to §5.3 wire codes by English substring. Each test here
//! constructs the EXACT message the store emits and asserts the mapped code,
//! so a wording change in the store fails CI instead of silently degrading
//! to `schema_migration_durability_unavailable`.

use crate::shamir_db::execute::admin_db_schema_migration::apply_journal::map_fence_error;
use crate::DbError;

use super::helpers::assert_code;

/// `acquire_schema_migration_fence`: fence already owned by another migration.
#[test]
fn already_owned_maps_to_in_progress() {
    let error = DbError::Validation(
        "schema migration fence for identity db-1 is already owned".to_string(),
    );
    assert_code(map_fence_error(error), "schema_migration_in_progress");
}

/// `fence_conflict` (CAS generation mismatch on acquire/append/release).
#[test]
fn fence_cas_conflict_maps_to_in_progress() {
    let error = DbError::Validation(
        "schema migration fence CAS conflict for identity db-1: expected generation 3, actual 7"
            .to_string(),
    );
    assert_code(map_fence_error(error), "schema_migration_in_progress");
}

/// `acquire_schema_migration_fence`: unresolved history requires recovery.
#[test]
fn unresolved_history_maps_to_in_doubt() {
    let error = DbError::Validation(
        "schema migration history for identity db-1 is unresolved; explicit recovery is required"
            .to_string(),
    );
    assert_code(map_fence_error(error), "schema_migration_in_doubt");
}

/// `acquire_schema_migration_fence`: non-contiguous ordinal.
#[test]
fn ordinal_gap_maps_to_gap() {
    let error =
        DbError::Validation("schema migration ordinal gap: expected 4, received 9".to_string());
    assert_code(map_fence_error(error), "schema_migration_gap");
}

/// `acquire_schema_migration_fence`: the migration already applied durably.
#[test]
fn already_applied_maps_to_checksum_mismatch() {
    let error = DbError::Validation("migration 'alpha-001' is already applied".to_string());
    assert_code(map_fence_error(error), "schema_migration_checksum_mismatch");
}

/// `acquire_schema_migration_fence`: body checksum differs from history.
#[test]
fn checksum_differs_maps_to_checksum_mismatch() {
    let error = DbError::Validation(
        "migration 'alpha-001' checksum differs from recorded history".to_string(),
    );
    assert_code(map_fence_error(error), "schema_migration_checksum_mismatch");
}

/// Anything not one of the pinned fence refusals keeps the §5.3 store mapping.
#[test]
fn an_unrelated_store_error_keeps_the_durability_mapping() {
    let error = DbError::Validation("disk on fire".to_string());
    assert_code(
        map_fence_error(error),
        "schema_migration_durability_unavailable",
    );
}
