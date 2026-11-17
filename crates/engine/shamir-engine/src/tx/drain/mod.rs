// `tx::drain` — background drain and WAL deletion admission.
#[allow(dead_code)]
pub mod drainer;
pub(crate) mod truncation_lease;
pub(crate) mod truncation_pin;
pub(crate) mod truncation_pin_guard;
