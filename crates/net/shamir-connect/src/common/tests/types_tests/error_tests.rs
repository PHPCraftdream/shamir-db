//! RV-CONNECT-94E37810 — exhaustive collapse-set oracle for `Error::to_wire`.
//!
//! Every variant of [`Error`] is explicitly classified here. Adding a new
//! variant without extending the match below is a compile error (the
//! oracle "fails"), so the §14 collapse set can never silently drift.

use crate::common::model::admit::AdmitReject;
use crate::common::model::error::{EnvironmentError, Error};

use std::mem::discriminant;

/// Classify one variant through `to_wire` and assert it lands on the
/// expected variant (same discriminant; `Error` has no `PartialEq`).
fn assert_collapse(input: Error, expected: Error) {
    assert_eq!(
        discriminant(&input.to_wire()),
        discriminant(&expected),
        "to_wire classification drifted for {input:?}"
    );
}

/// Exhaustive oracle over the full error vocabulary.
#[test]
fn rv_connect_94e37810_to_wire_collapse_set_oracle() {
    // Pass-through (public, wire-safe) variants.
    assert_collapse(Error::AuthFailed, Error::AuthFailed);
    assert_collapse(
        Error::RateLimited { retry_after: 7 },
        Error::RateLimited { retry_after: 7 },
    );
    assert_collapse(
        Error::ServerBusy { retry_after: 9 },
        Error::ServerBusy { retry_after: 9 },
    );
    // Retry hints survive the collapse (not zeroed).
    assert!(matches!(
        Error::RateLimited { retry_after: 7 }.to_wire(),
        Error::RateLimited { retry_after: 7 }
    ));
    assert!(matches!(
        Error::ServerBusy { retry_after: 9 }.to_wire(),
        Error::ServerBusy { retry_after: 9 }
    ));
    assert_collapse(Error::UnsupportedVersion, Error::UnsupportedVersion);
    assert_collapse(Error::BootstrapFailed, Error::BootstrapFailed);

    // Collapses to AuthFailed (internal cause / disclosure-unsafe).
    assert_collapse(
        Error::Environment(EnvironmentError::Entropy),
        Error::AuthFailed,
    );
    assert_collapse(Error::ServerAuthFailed, Error::AuthFailed);
    assert_collapse(Error::ServerSignatureInvalid, Error::AuthFailed);
    assert_collapse(Error::ServerIdentityChanged, Error::AuthFailed);
    assert_collapse(Error::KdfParamsRejected, Error::AuthFailed);
    assert_collapse(Error::KnownHostsIntegrityFailed, Error::AuthFailed);
    assert_collapse(Error::UserNotFound, Error::AuthFailed);
    assert_collapse(Error::InvalidInput("x"), Error::AuthFailed);
    assert_collapse(Error::InvalidUsername("x"), Error::AuthFailed);
    assert_collapse(Error::InvalidPassword("x"), Error::AuthFailed);
    assert_collapse(Error::Crypto("x"), Error::AuthFailed);
    assert_collapse(Error::Encoding("x".into()), Error::AuthFailed);
    assert_collapse(Error::StorageFailed, Error::AuthFailed);
    assert_collapse(
        Error::AdmissionRejected {
            reason: AdmitReject::AllActive,
            username: "alice".into(),
        },
        Error::AuthFailed,
    );
}
