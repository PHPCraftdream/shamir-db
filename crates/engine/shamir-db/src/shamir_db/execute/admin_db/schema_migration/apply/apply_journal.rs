//! The durable journal / error / result layer of the application-schema
//! APPLY arm (release-1 R1c step 2b-3a): journal append under the owned
//! fence, in-doubt and postcondition-failure markers, durability error
//! mapping, the step-2a fence-refusal mapping, and the `applied` result
//! wire encoding. Admission lives in `apply.rs`; the fenced runner and the
//! per-operation postconditions live in `apply_operations.rs`.

use shamir_query_types::admin::types::schema_migration::SchemaMigrationMetadata;

use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEvent, JournalEventKind, SchemaMigrationContext, SchemaMigrationStep,
};
use crate::types::value::QueryValue;
use shamir_types::types::common::new_map;

use super::super::super::admin_dispatch::ShamirAdminExecutor;
use super::super::super::helpers::admin_result;
use super::super::plan::{coded, map_store_error};

/// Journal identity bundle for one fenced run: everything `append_step` and
/// the marker writers need besides the executor and the step itself.
pub(super) struct JournalRef<'a> {
    pub(super) serving: DatabaseIdentity,
    pub(super) metadata: &'a SchemaMigrationMetadata,
    pub(super) checksum: [u8; 32],
    pub(super) generation: u64,
    pub(super) context: &'a SchemaMigrationContext,
}

/// Append one journal event under the owned fence (step-2a API: validates the
/// candidate against the history, persists and syncs durably).
pub(super) async fn append_step(
    executor: &ShamirAdminExecutor,
    journal: &JournalRef<'_>,
    step: SchemaMigrationStep,
) -> Result<JournalEvent, crate::DbError> {
    let JournalRef {
        serving,
        metadata,
        checksum,
        generation,
        context,
    } = journal;
    executor
        .shamir
        .system_store()
        .append_schema_migration_event(
            *serving,
            *generation,
            (*metadata).clone(),
            *checksum,
            step,
            context,
        )
        .await
}

/// Best-effort durable `in_doubt` marker after an uncertain window, then the
/// caller-facing in-doubt error. A failing marker write is ignored (the
/// journal already shows the resumable intent-without-completion state).
pub(super) async fn record_in_doubt(
    executor: &ShamirAdminExecutor,
    journal: &JournalRef<'_>,
    index: u32,
    evidence: String,
) -> BatchError {
    let _ = append_step(
        executor,
        journal,
        SchemaMigrationStep {
            kind: JournalEventKind::InDoubt,
            operation_index: Some(index),
            evidence: Some(evidence.clone()),
        },
    )
    .await;
    coded(
        "schema_migration_in_doubt",
        format!(
            "migration '{}' stopped inside the uncertain window; the journal is left \
             resumable: {evidence}",
            journal.metadata.id
        ),
    )
}

/// Postcondition failure: the DDL effect was observed not to hold, so the
/// same best-effort resumable journal record as in doubt is written (started
/// plus operation intent, no completion), but the wire code is the stable
/// `schema_migration_postcondition_failed`. Journal-write failures of the
/// marker itself are ignored (the intent is already durable).
pub(super) async fn record_postcondition_failure(
    executor: &ShamirAdminExecutor,
    journal: &JournalRef<'_>,
    index: u32,
    evidence: String,
) -> BatchError {
    let _ = append_step(
        executor,
        journal,
        SchemaMigrationStep {
            kind: JournalEventKind::InDoubt,
            operation_index: Some(index),
            evidence: Some(evidence.clone()),
        },
    )
    .await;
    coded(
        "schema_migration_postcondition_failed",
        format!(
            "migration '{}' failed its postcondition; the journal is left \
             resumable: {evidence}",
            journal.metadata.id
        ),
    )
}

/// A journal write/sync/read-back failure: before any effect it is plain
/// durability unavailability; after intents are durable an effect may have
/// occurred → in doubt (§4.2).
pub(super) fn durability_error(error: crate::DbError, intents_durable: bool) -> BatchError {
    if intents_durable {
        coded(
            "schema_migration_in_doubt",
            format!(
                "journal durability failed after operation intents were recorded; the \
                 migration is in doubt and left resumable: {error}"
            ),
        )
    } else {
        map_store_error(error)
    }
}

/// Map step-2a `acquire_schema_migration_fence` refusals to the stable §5.3
/// codes (the CAS failures are fencing/progress states, not corruption).
///
/// The step-2a store raises these as untyped `DbError::Validation(String)`
/// messages (see `system_store/catalogue/schema_migrations/journal.rs`), so
/// the mapping is by English substring. The wording is PINNED by
/// `apply_fence_error_mapping_tests.rs`: any wording change fails CI instead
/// of silently degrading to `schema_migration_durability_unavailable`.
pub(in crate::shamir_db) fn map_fence_error(error: crate::DbError) -> BatchError {
    let text = error.to_string();
    let code = if text.contains("already owned") || text.contains("fence") {
        "schema_migration_in_progress"
    } else if text.contains("unresolved") {
        "schema_migration_in_doubt"
    } else if text.contains("ordinal gap") {
        "schema_migration_gap"
    } else if text.contains("already applied") || text.contains("checksum") {
        "schema_migration_checksum_mismatch"
    } else {
        return map_store_error(error);
    };
    coded(code, text)
}

pub(super) fn plain(message: String) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message,
        code: None,
    }
}

/// Encode `SchemaMigrationApplyResult` with its serde field names — the same
/// admin-record wire convention as the status and plan arms. Everything
/// trusted (actor, time, binary version, format epoch) was derived
/// server-side when the journal events were written; nothing comes from the
/// request beyond the echoed migration identity.
pub(super) fn applied_value(
    protocol_version: u16,
    migration_id: String,
    ordinal: u64,
    checksum: [u8; 32],
    applied_at_unix_micros: u64,
    fence_generation: u64,
) -> QueryResult {
    let mut value = new_map();
    value.insert(
        "protocol_version".to_string(),
        QueryValue::Int(protocol_version as i64),
    );
    value.insert("migration_id".to_string(), QueryValue::Str(migration_id));
    value.insert("ordinal".to_string(), QueryValue::Int(ordinal as i64));
    value.insert("checksum".to_string(), QueryValue::Bin(checksum.to_vec()));
    value.insert("status".to_string(), QueryValue::Str("applied".to_string()));
    value.insert(
        "applied_at_unix_micros".to_string(),
        QueryValue::Int(applied_at_unix_micros as i64),
    );
    value.insert(
        "fence_generation".to_string(),
        QueryValue::Int(fence_generation as i64),
    );
    admin_result(QueryValue::Map(value))
}
