//! Admission, serialization, and recovery evidence for server-global registrations.

pub(super) mod admission;
mod binding;
pub(super) mod gate;
pub(super) mod reconcile;
mod registration;

#[cfg(test)]
pub(in crate::shamir_db) use reconcile::{test_capture_intent, test_verify};
