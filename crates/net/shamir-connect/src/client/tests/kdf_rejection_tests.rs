use crate::client::handshake::{HandshakeBuilder, ServerChallenge};
use crate::common::crypto::argon2_call_count;
use crate::common::error::Error;
use crate::common::kdf_params::KdfParams;
use crate::common::types::{BindingMode, TransportKind};
use crate::common::username::NormalizedUsername;

#[test]
fn rejects_invalid_kdf_params_before_argon2_and_wipes_password() {
    let handshake = HandshakeBuilder::new(
        NormalizedUsername::from_raw("alice").unwrap(),
        TransportKind::Unix,
        BindingMode::None,
    )
    .accept_new_host(true)
    .build()
    .unwrap();

    let invalid_params = [
        KdfParams {
            memory_kb: 19_455,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            memory_kb: 262_145,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            time: 1,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            time: 9,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            parallelism: 0,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            parallelism: 9,
            ..KdfParams::DEFAULT
        },
        KdfParams {
            argon2_version: 0x10,
            ..KdfParams::DEFAULT
        },
    ];

    for kdf_params in invalid_params {
        let challenge = ServerChallenge {
            salt: [7; 16],
            kdf_params,
            server_nonce: [8; 32],
        };
        let mut password = b"valid password".to_vec();
        let calls_before = argon2_call_count();

        assert!(matches!(
            handshake.process_challenge(&challenge, &mut password),
            Err(Error::KdfParamsRejected)
        ));
        assert_eq!(argon2_call_count(), calls_before);
        assert!(password.iter().all(|&byte| byte == 0));
    }
}
