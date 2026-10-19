//! Shared test helpers for the resume-flow integration tests.

use shamir_connect::common::time::ns;
use shamir_connect::server::resume::{ResumeConfig, ResumeUserState};

pub(crate) const TICKET_TTL: u64 = ns::HOUR;

/// Test helper: a minimal `ResumeUserState` carrying only a
/// `tickets_invalid_before_ns` epoch (username/roles/superuser left at
/// harmless defaults). Used at the mechanical call sites that only care
/// about the epoch check.
pub(crate) fn state(tib: u64) -> ResumeUserState {
    ResumeUserState {
        username: "u".into(),
        roles: vec![],
        superuser: false,
        replicator: false,
        tickets_invalid_before_ns: tib,
    }
}

pub(crate) fn fixed_config() -> ResumeConfig {
    ResumeConfig::new(
        [0xa1u8; 32], // ticket_key
        None,         // ticket_key_previous
        true,         // allow_browser_ticket_upgrade
        false,        // disable_plain_ticket_upgrade
    )
}
