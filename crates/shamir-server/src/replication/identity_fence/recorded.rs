//! The fence uses the engine's advancing bookmark value, not a separate key.

pub(crate) use shamir_db::engine::meta::repl_bookmark::{
    load_fence_record as load, save_fence_record_v3_aware as save, FenceRecord,
};
