use crate::engine::table::TableManager;
use crate::shamir_db::system_store::{FormatEpoch, SchemaMigrationContext};
use crate::types::value::{InnerValue, QueryValue};
use futures::StreamExt;
use shamir_query_builder::{ddl, SchemaMigration};
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationBody,
};
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::types::record_id::RecordId;

/// The journal needs a persistent system store; the directory must outlive the db.
pub(super) async fn persistent_db() -> (tempfile::TempDir, crate::ShamirDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::ShamirDb::init(crate::shamir_db::system_store::SystemStoreConfig::Fjall(
        dir.path().join("system"),
    ))
    .await
    .unwrap();
    (dir, db)
}

pub(super) fn context() -> SchemaMigrationContext {
    SchemaMigrationContext::new(crate::access::Actor::System, FormatEpoch::Known(7))
}

pub(super) fn body(id: &str, ordinal: u64) -> SchemaMigrationBody {
    SchemaMigration::new(id, ordinal, format!("migration {ordinal}"))
        .op(ddl::create_table(format!("items_{ordinal}")).build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

pub(super) fn checksum(body: &SchemaMigrationBody) -> [u8; 32] {
    canonical_body_checksum(body).expect("test migration body canonicalizes")
}

pub(super) async fn rows(table: &TableManager) -> Vec<(RecordId, QueryValue)> {
    let interner = table
        .info_store_read_view()
        .interner_read_view()
        .await
        .expect("journal interner is readable");
    let mut stream = Box::pin(table.list_stream(128));
    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        for (id, record) in batch.expect("journal scan succeeds") {
            let value = match record {
                crate::engine::table::record_cow::RecordCow::Borrowed(bytes) => interner
                    .record_bytes_to_query_value(&bytes)
                    .expect("journal record materialization succeeds")
                    .expect("journal row is a supported value"),
                crate::engine::table::record_cow::RecordCow::Owned(value) => interner
                    .inner_to_query_value(&value)
                    .expect("journal record materializes"),
            };
            records.push((id, value));
        }
    }
    records
}

pub(super) async fn overwrite_row(table: &TableManager, id: RecordId, value: QueryValue) {
    let interner = table
        .interner_for_test()
        .get()
        .await
        .expect("test table interner is loaded");
    let inner: InnerValue =
        query_value_to_inner(&value, interner).expect("raw fixture value interns");
    table
        .set(id, &inner)
        .await
        .expect("raw fixture write succeeds");
}

/// Run a test body on a roomy thread with its own runtime: the apply runner
/// nests the admin handlers' async frames (function compilation among them)
/// deeper than a debug test thread's default stack allows. Same idea as the
/// server's lifecycle fixtures.
pub(super) fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}
