use super::wasm_function::{read_guest_mem, write_bytes_to_guest, write_value_to_guest, HostState};
use shamir_types::types::value::QueryValue;

// ── Async host imports: db_get / db_insert / db_query (slice 8b) ───────
//
// Same three-phase borrow dance as host_call:
// 1. Read table-name (utf8) + key/doc/filter bytes (msgpack) from guest.
// 2. Clone Arc<dyn DbGateway> + repo from caller.data().
// 3. Drop Caller borrows, await the gateway method.
// 4. Re-acquire Caller, alloc + write result.

/// Host implementation of `db_get(table_ptr, table_len, key_ptr, key_len) -> i64`.
///
/// Reads a record from `repo.table` by the given key (msgpack `QueryValue`).
/// Returns 0 if not found, or the packed pointer to the record's msgpack.
pub(super) fn host_db_get(
    mut caller: wasmtime::Caller<'_, HostState>,
    (table_ptr, table_len, key_ptr, key_len): (i32, i32, i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        // Phase 1: read inputs (sync).
        let table_bytes;
        let key_bytes;
        let db;
        let repo;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("db_get: missing export `memory`"))?;

            table_bytes = read_guest_mem(memory.data(&caller), table_ptr, table_len)?;
            key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;

            let state = caller.data();
            db = state.db.clone();
            repo = state.repo.clone();
        }

        let table = String::from_utf8(table_bytes)
            .map_err(|_| wasmtime::Error::msg("db_get: table name is not valid UTF-8"))?;
        let key = QueryValue::from_bytes(&key_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("db_get: key decode error: {e}")))?;

        let gateway = db.ok_or_else(|| {
            wasmtime::Error::msg("db_get: no db gateway (invoke via invoke_function_in_db)")
        })?;

        // Phase 2: await the gateway.
        let result = gateway
            .get(&repo, &table, key)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("db_get: {e}")))?;

        // Phase 3: write result back.
        write_value_to_guest(&mut caller, result).await
    })
}

/// Host implementation of `db_insert(table_ptr, table_len, doc_ptr, doc_len) -> i64`.
///
/// Inserts a document into `repo.table`. Returns the stored record as
/// packed msgpack.
pub(super) fn host_db_insert(
    mut caller: wasmtime::Caller<'_, HostState>,
    (table_ptr, table_len, doc_ptr, doc_len): (i32, i32, i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        let table_bytes;
        let doc_bytes;
        let db;
        let repo;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("db_insert: missing export `memory`"))?;

            table_bytes = read_guest_mem(memory.data(&caller), table_ptr, table_len)?;
            doc_bytes = read_guest_mem(memory.data(&caller), doc_ptr, doc_len)?;

            let state = caller.data();
            db = state.db.clone();
            repo = state.repo.clone();
        }

        let table = String::from_utf8(table_bytes)
            .map_err(|_| wasmtime::Error::msg("db_insert: table name is not valid UTF-8"))?;
        let doc = QueryValue::from_bytes(&doc_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("db_insert: doc decode error: {e}")))?;

        let gateway = db.ok_or_else(|| {
            wasmtime::Error::msg("db_insert: no db gateway (invoke via invoke_function_in_db)")
        })?;

        let result = gateway
            .insert(&repo, &table, doc)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("db_insert: {e}")))?;

        write_value_to_guest(&mut caller, Some(result)).await
    })
}

/// Host implementation of `db_query(table_ptr, table_len, filter_ptr, filter_len) -> i64`.
///
/// Queries `repo.table` with an optional filter. The filter is a msgpack
/// `QueryValue`; zero-length bytes means `None` (no filter / return all).
/// Returns a msgpack `Value::List` of matching records, packed.
pub(super) fn host_db_query(
    mut caller: wasmtime::Caller<'_, HostState>,
    (table_ptr, table_len, filter_ptr, filter_len): (i32, i32, i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        let table_bytes;
        let filter_bytes;
        let db;
        let repo;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("db_query: missing export `memory`"))?;

            table_bytes = read_guest_mem(memory.data(&caller), table_ptr, table_len)?;
            filter_bytes = read_guest_mem(memory.data(&caller), filter_ptr, filter_len)?;

            let state = caller.data();
            db = state.db.clone();
            repo = state.repo.clone();
        }

        let table = String::from_utf8(table_bytes)
            .map_err(|_| wasmtime::Error::msg("db_query: table name is not valid UTF-8"))?;

        let filter =
            if filter_bytes.is_empty() {
                None
            } else {
                Some(QueryValue::from_bytes(&filter_bytes).map_err(|e| {
                    wasmtime::Error::msg(format!("db_query: filter decode error: {e}"))
                })?)
            };

        let gateway = db.ok_or_else(|| {
            wasmtime::Error::msg("db_query: no db gateway (invoke via invoke_function_in_db)")
        })?;

        let records = gateway
            .query(&repo, &table, filter)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("db_query: {e}")))?;

        // Pack as a Value::List.
        let list_value = QueryValue::List(records);
        write_value_to_guest(&mut caller, Some(list_value)).await
    })
}

/// Host implementation of `db_execute(req_ptr, req_len) -> i64`.
///
/// Reads a msgpack `BatchRequest` from guest memory, runs it through the
/// gateway (same executor a wire client uses, as the function's effective
/// actor) and writes the msgpack `BatchResponse` back. The general form of
/// db_get/db_insert/db_query. Mirrors the host_db_query 3-phase dance.
pub(super) fn host_db_execute(
    mut caller: wasmtime::Caller<'_, HostState>,
    (req_ptr, req_len): (i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        let req_bytes;
        let db;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("db_execute: missing export `memory`"))?;
            req_bytes = read_guest_mem(memory.data(&caller), req_ptr, req_len)?;
            db = caller.data().db.clone();
        }
        let gateway = db.ok_or_else(|| {
            wasmtime::Error::msg("db_execute: no db gateway (invoke via invoke_function_in_db)")
        })?;
        let resp_bytes = gateway
            .execute(&req_bytes)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("db_execute: {e}")))?;
        write_bytes_to_guest(&mut caller, &resp_bytes).await
    })
}
