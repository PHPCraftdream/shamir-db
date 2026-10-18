//! Durable subscription binding + quarantine coverage (S5c-X PR8b-2).
//!
//! The subscription row — not the current replication profile — is the
//! durable authority for `resync_required` quarantine coverage. Every row
//! carries a versioned, validated destination binding written in the same
//! transaction as the row itself; rows lacking it are refused at open with
//! a typed catalogue error.

pub mod binding;
pub(crate) mod confirmed;
pub mod coverage;
pub(crate) mod pending;
pub mod validate;

pub use binding::{SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION};
pub(crate) use confirmed::ConfirmedActive;
pub(crate) use coverage::{quarantined_row_keys, row_is_quarantined};
