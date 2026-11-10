//! Tracked facade for database emission support.

pub(crate) mod checked;
#[path = "core.rs"]
pub(crate) mod implementation;

pub(crate) use self::implementation::{
    emit_batch_append_helper, emit_context_init, emit_db_call_helper, emit_db_execute_helper,
    emit_dynmap_find_helper, emit_response_affected_helper, emit_response_get_helper,
    host_import_name,
};
