#![allow(unused_variables)]

use shamir_sdk::prelude::*;

#[shamir_sdk::validator]
pub async fn check_user(record: Value, old_record: Option<Value>, ctx: Ctx) -> Validation {
    if record == Value::Null {
        return Validation::record_error("empty_record");
    }
    Validation::accept()
}
