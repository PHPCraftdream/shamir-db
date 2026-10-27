use shamir_sdk::prelude::*;

#[shamir_sdk::validator]
pub async fn check_user(record: Value, _old_record: Option<Value>, _ctx: Ctx) -> Validation {
    if record == Value::Null {
        return Validation::record_error("empty_record");
    }
    Validation::accept()
}
