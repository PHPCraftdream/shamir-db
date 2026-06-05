//! Example: a **validator** bound to a table.
//!
//! A validator takes `(record: Value, old_record: Option<Value>, ctx: Ctx)`
//! and returns `Validation`. It fires before a record is written, and can
//! accumulate field-bound or record-level errors to reject the write.
//!
//! Build:
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown -p fn-validator
//! ```

use shamir_sdk::prelude::*;

/// Rejects records that lack a non-empty `"name"` string field.
#[shamir_sdk::validator]
pub async fn require_name(record: Value, _old: Option<Value>, _ctx: Ctx) -> Validation {
    match &record {
        Value::Map(entries) => {
            let name = entries.iter().find(|(k, _)| k == "name");
            match name {
                Some((_, Value::Str(s))) if !s.is_empty() => Validation::accept(),
                Some((_, Value::Str(_))) => Validation::reject("name", "name_empty"),
                _ => Validation::reject("name", "name_required"),
            }
        }
        _ => Validation::record_error("expected_map"),
    }
}
