//! Wire-shape guard for the optional resume-proof preamble.
//!
//! The server sniffs the first frame as `ResumeProofInit` before dispatching
//! to `ResumeInit` / `AuthInit`. A MAP-encoded `auth_init` or `resume_init`
//! (the TypeScript client sends named maps) carries a `client_nonce` key too
//! and must NOT be mistaken for the preamble.

use crate::connection::wire::{AuthInit, ResumeInit, ResumeProofInit};

fn nonce() -> Vec<u8> {
    vec![7u8; 32]
}

#[test]
fn map_encoded_auth_init_is_not_a_resume_proof_preamble() {
    let frame = rmp_serde::to_vec_named(&AuthInit {
        user: "alice".to_owned(),
        client_nonce: nonce(),
        binding_mode: 1,
        version: 1,
    })
    .expect("encode auth_init as a map");
    assert!(rmp_serde::from_slice::<ResumeProofInit>(&frame).is_err());
}

#[test]
fn map_encoded_resume_init_is_not_a_resume_proof_preamble() {
    let frame = rmp_serde::to_vec_named(&ResumeInit {
        ticket: vec![1u8; 16],
        client_nonce: nonce(),
        binding_mode: 1,
    })
    .expect("encode resume_init as a map");
    assert!(rmp_serde::from_slice::<ResumeProofInit>(&frame).is_err());
}

#[test]
fn positional_and_named_resume_proof_preambles_still_decode() {
    let positional = rmp_serde::to_vec(&ResumeProofInit {
        client_nonce: nonce(),
    })
    .expect("encode positional preamble");
    let named = rmp_serde::to_vec_named(&ResumeProofInit {
        client_nonce: nonce(),
    })
    .expect("encode named preamble");
    for frame in [positional, named] {
        let decoded: ResumeProofInit = rmp_serde::from_slice(&frame).expect("preamble decodes");
        assert_eq!(decoded.client_nonce, nonce());
    }
}
