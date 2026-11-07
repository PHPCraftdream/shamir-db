#[path = "receiver.rs"]
pub mod receiver;
#[path = "registry.rs"]
pub(super) mod registry;

#[cfg(test)]
#[path = "tests/gate_tests.rs"]
mod tests;
