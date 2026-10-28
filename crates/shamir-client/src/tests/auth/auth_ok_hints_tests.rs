//! Auth-ok hints and reconnect-to-approve regression coverage.

#[cfg(unix)]
mod unix_tests {
    use zeroize::Zeroizing;

    use crate::client::transport::{WireAuthInit, WireAuthOk, WireChallenge, WireClientProof};
    use crate::{Client, ClientError, ConnectLocalOptions};
    use shamir_connect::client::rotation::ROTATION_MAX_TRANSITION_NS;
    use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
    use shamir_connect::common::identity::build_identity_input;
    use shamir_connect::common::kdf_params::KdfParams;
    use shamir_connect::common::rotation::build_rotation_proof_input;
    use shamir_connect::common::scram::{build_client_proof, DerivedKeys};
    use shamir_connect::common::types::{BindingMode, ProtocolVersion, TransportKind};
    use shamir_connect::common::username::NormalizedUsername;
    use shamir_connect::server::rotation::ServerIdentityState;
    use shamir_connect::server::{
        AuthInitView, ListenerPolicy, ProofOutcome, ServerHandshake, ServerSecrets, UserRecord,
    };
    use shamir_transport_ipc::IpcListener;
    use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    const PASSWORD: &[u8] = b"correct horse battery staple";
    const SALT: [u8; 16] = [5; 16];
    const KDF: KdfParams = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    const SESSION_MAX_AGE_NS: u64 = 60_000_000_000;

    fn now_ns() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u64::MAX as u128) as u64
    }

    async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Vec<u8> {
        let mut len = [0; 4];
        reader
            .read_exact(&mut len)
            .await
            .expect("read frame length");
        let mut bytes = vec![0; u32::from_be_bytes(len) as usize];
        reader
            .read_exact(&mut bytes)
            .await
            .expect("read frame body");
        bytes
    }

    async fn write_frame<W: AsyncWriteExt + Unpin>(writer: &mut W, bytes: &[u8]) {
        writer
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .expect("write frame length");
        writer.write_all(bytes).await.expect("write frame body");
    }

    #[derive(serde::Serialize)]
    struct EightFieldAuthOk {
        #[serde(with = "serde_bytes")]
        server_signature: Vec<u8>,
        #[serde(with = "serde_bytes")]
        server_pub_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        identity_sig: Vec<u8>,
        #[serde(with = "serde_bytes")]
        session_id: Vec<u8>,
        expires_at_ns: u64,
        #[serde(with = "serde_bytes")]
        resumption_ticket: Vec<u8>,
        resumption_expires_at_ns: u64,
        server_query_version: u8,
    }

    async fn spawn_fake_server(
        path: String,
        current_seed: [u8; 32],
        previous_seed: Option<[u8; 32]>,
        mode: &'static str,
    ) -> (oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut listener = IpcListener::bind(&path).await.expect("bind fake server");
            let _ = ready_tx.send(());
            let stream = listener.accept().await.expect("accept client");
            let (mut reader, mut writer) = split(stream);
            let init: WireAuthInit =
                rmp_serde::from_slice(&read_frame(&mut reader).await).expect("typed auth_init");
            let username = NormalizedUsername::from_raw(&init.user).expect("normalize username");
            let client_nonce: [u8; 32] = init
                .client_nonce
                .as_slice()
                .try_into()
                .expect("client nonce width");
            let current = Ed25519Keypair::from_seed(&current_seed);
            let old_seed = previous_seed.unwrap_or([14; 32]);
            let salt = SALT;
            let keys = DerivedKeys::derive(PASSWORD, &salt, &KDF).expect("derive SCRAM keys");
            let record = UserRecord {
                salt,
                stored_key: keys.stored_key,
                server_key: keys.server_key,
                kdf_params: KDF,
                tickets_invalid_before_ns: 0,
            };
            let transition_until_ns = now_ns().saturating_add(if mode == "long-window" {
                ROTATION_MAX_TRANSITION_NS.saturating_add(3_600_000_000_000)
            } else {
                ROTATION_MAX_TRANSITION_NS / 2
            });
            let identity_state = previous_seed.map(|old| {
                ServerIdentityState::from_material(
                    &current_seed,
                    Some(&old),
                    Some(transition_until_ns),
                    1,
                )
            });
            let secrets = ServerSecrets {
                server_secret: [1; 32],
                lockout_secret: [2; 32],
            };
            let handshake = ServerHandshake::new(
                ListenerPolicy::new(BindingMode::None),
                TransportKind::Unix,
                &secrets,
                AuthInitView {
                    user: username.clone(),
                    client_nonce,
                    binding_mode: BindingMode::None,
                    version: init.version,
                },
                [0; 32],
                KDF,
                |_| Some(record),
            )
            .expect("construct ServerHandshake");
            let challenge = handshake.challenge();
            let challenge_wire = WireChallenge {
                salt: challenge.salt.to_vec(),
                memory_kb: challenge.kdf_params.memory_kb,
                time: challenge.kdf_params.time,
                parallelism: challenge.kdf_params.parallelism,
                argon2_version: challenge.kdf_params.argon2_version,
                server_nonce: challenge.server_nonce.to_vec(),
            };
            write_frame(
                &mut writer,
                &rmp_serde::to_vec(&challenge_wire).expect("serialize challenge"),
            )
            .await;

            let auth_message = shamir_connect::common::AuthMessage::build(
                shamir_connect::common::auth_message::AuthMessageInputs {
                    username: &username,
                    client_nonce: &client_nonce,
                    server_nonce: &challenge.server_nonce,
                    salt: &challenge.salt,
                    kdf_params: challenge.kdf_params,
                    transport_kind: TransportKind::Unix,
                    binding_mode: BindingMode::None,
                    tls_exporter_or_zeros: &[0; 32],
                    supported_version: ProtocolVersion::V1,
                },
            )
            .expect("build auth message");
            let client_keys = DerivedKeys::derive(PASSWORD, &challenge.salt, &challenge.kdf_params)
                .expect("derive client keys");
            let expected_proof = build_client_proof(
                &client_keys.client_key,
                &client_keys.stored_key,
                &auth_message,
            );
            let wire_proof: WireClientProof =
                rmp_serde::from_slice(&read_frame(&mut reader).await).expect("typed client proof");
            let proof: [u8; 32] = wire_proof
                .client_proof
                .as_slice()
                .try_into()
                .expect("proof width");
            assert_eq!(proof, expected_proof);
            let ProofOutcome::Accepted(mut auth_ok) = handshake
                .verify_proof(&proof, &current, SESSION_MAX_AGE_NS)
                .expect("verify SCRAM")
            else {
                panic!("valid proof rejected");
            };
            let identity_input = build_identity_input(
                &auth_ok.server_pub_key,
                TransportKind::Unix,
                BindingMode::None,
                &[0; 32],
                &auth_message,
                &auth_ok.session_id,
                auth_ok.expires_at_ns,
            );
            if let Some(state) = identity_state.as_ref() {
                let mut payload = handshake
                    .rotation_in_progress_payload(&auth_ok, state)
                    .expect("build rotation payload")
                    .expect("active rotation");
                if mode == "long-window" {
                    let previous = Ed25519Keypair::from_seed(&old_seed);
                    payload.transition_until_ns = transition_until_ns;
                    payload.rotation_proof = previous.sign(&build_rotation_proof_input(
                        &payload.previous_pub,
                        &auth_ok.server_pub_key,
                        transition_until_ns,
                    ));
                    payload.identity_sig_previous = previous.sign(&identity_input);
                }
                if mode == "bad-proof" {
                    payload.rotation_proof[0] ^= 1;
                }
                if mode == "bad-old-signature" {
                    payload.identity_sig_previous[0] ^= 1;
                }
                auth_ok.rotation_in_progress = Some(payload);
            }
            if mode == "eight-field" {
                let legacy = EightFieldAuthOk {
                    server_signature: auth_ok.server_signature.to_vec(),
                    server_pub_key: auth_ok.server_pub_key.to_vec(),
                    identity_sig: auth_ok.identity_sig.to_vec(),
                    session_id: auth_ok.session_id.to_vec(),
                    expires_at_ns: auth_ok.expires_at_ns,
                    resumption_ticket: Vec::new(),
                    resumption_expires_at_ns: 0,
                    server_query_version: 0,
                };
                write_frame(
                    &mut writer,
                    &rmp_serde::to_vec(&legacy).expect("serialize legacy auth_ok"),
                )
                .await;
            } else {
                let typed = WireAuthOk {
                    server_signature: auth_ok.server_signature.to_vec(),
                    server_pub_key: auth_ok.server_pub_key.to_vec(),
                    identity_sig: auth_ok.identity_sig.to_vec(),
                    session_id: auth_ok.session_id.to_vec(),
                    expires_at_ns: auth_ok.expires_at_ns,
                    resumption_ticket: Vec::new(),
                    resumption_expires_at_ns: 0,
                    server_query_version: 0,
                    rotation_in_progress: auth_ok
                        .rotation_in_progress
                        .as_ref()
                        .map(shamir_connect::common::wire::RotationInProgressWire::from),
                    kdf_upgrade_required: Some(true),
                };
                write_frame(
                    &mut writer,
                    &rmp_serde::to_vec(&typed).expect("serialize auth_ok"),
                )
                .await;
            }
        });
        (ready_rx, task)
    }

    async fn connect(path: String, pin: Option<[u8; 32]>) -> Result<Client, ClientError> {
        Client::connect_local(ConnectLocalOptions {
            addr: path,
            username: "admin".into(),
            password: Zeroizing::new(PASSWORD.to_vec()),
            accept_new_host: pin.is_none(),
            trusted_pin: pin,
            connect_timeout: None,
            request_timeout: None,
        })
        .await
    }

    async fn wait_ready(ready: oneshot::Receiver<()>) {
        ready.await.expect("fake listener ready");
    }

    #[tokio::test]
    async fn first_use_surfaces_both_hints_and_eight_field_defaults() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let current_seed = [9; 32];
        let path = temp
            .path()
            .join("first.sock")
            .to_string_lossy()
            .into_owned();
        let (ready, server) =
            spawn_fake_server(path.clone(), current_seed, Some([14; 32]), "hints").await;
        wait_ready(ready).await;
        let client = connect(path, None).await.expect("first-use connect");
        assert_eq!(
            client.server_pub_key_pin(),
            sha256(&Ed25519Keypair::from_seed(&current_seed).public_bytes())
        );
        assert!(client.rotation_in_progress().is_some());
        assert!(client.kdf_upgrade_required());
        client.close().await;
        server.await.expect("server completed");
        let path = temp
            .path()
            .join("legacy.sock")
            .to_string_lossy()
            .into_owned();
        let (ready, server) =
            spawn_fake_server(path.clone(), current_seed, None, "eight-field").await;
        wait_ready(ready).await;
        let client = connect(path, None).await.expect("legacy connect");
        assert!(client.rotation_in_progress().is_none());
        assert!(!client.kdf_upgrade_required());
        client.close().await;
        server.await.expect("legacy server completed");
    }

    #[tokio::test]
    async fn valid_rotation_is_pending_then_reconnects_with_approved_pin() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let old = Ed25519Keypair::from_seed(&[10; 32]);
        let current = Ed25519Keypair::from_seed(&[11; 32]);
        let path = temp
            .path()
            .join("rotation.sock")
            .to_string_lossy()
            .into_owned();
        let (ready, server) =
            spawn_fake_server(path.clone(), [11; 32], Some([10; 32]), "valid").await;
        wait_ready(ready).await;
        let result = connect(path, Some(sha256(&old.public_bytes()))).await;
        match result {
            Err(ClientError::IdentityRotationPending {
                previous_pin,
                new_pin,
                transition_until_ns,
            }) => {
                assert_eq!(previous_pin, sha256(&old.public_bytes()));
                assert_eq!(new_pin, sha256(&current.public_bytes()));
                assert!(transition_until_ns > 0);
            }
            other => panic!("expected IdentityRotationPending, got {:?}", other.err()),
        }
        server.await.expect("rotation server completed");
        let path = temp
            .path()
            .join("approved.sock")
            .to_string_lossy()
            .into_owned();
        let (ready, server) = spawn_fake_server(path.clone(), [11; 32], None, "eight-field").await;
        wait_ready(ready).await;
        let client = connect(path, Some(sha256(&current.public_bytes())))
            .await
            .expect("approved reconnect");
        assert_eq!(client.server_pub_key_pin(), sha256(&current.public_bytes()));
        client.close().await;
        server.await.expect("approved server completed");
    }

    #[tokio::test]
    async fn tampered_proof_signature_window_and_unrelated_pin_fail_closed() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        for (name, mode) in [
            ("proof", "bad-proof"),
            ("signature", "bad-old-signature"),
            ("window", "long-window"),
            ("unrelated", "valid"),
        ] {
            let path = temp
                .path()
                .join(format!("{name}.sock"))
                .to_string_lossy()
                .into_owned();
            let old = Ed25519Keypair::from_seed(&[12; 32]);
            let (ready, server) =
                spawn_fake_server(path.clone(), [13; 32], Some([12; 32]), mode).await;
            wait_ready(ready).await;
            let pin = if name == "unrelated" {
                [0xA5; 32]
            } else {
                sha256(&old.public_bytes())
            };
            let result = connect(path, Some(pin)).await;
            assert!(!matches!(
                &result,
                Err(ClientError::IdentityRotationPending { .. })
            ));
            if name == "unrelated" {
                assert!(matches!(
                    &result,
                    Err(ClientError::Handshake { context, .. })
                        if context.contains("server_identity_changed")
                ));
            } else {
                assert!(matches!(&result, Err(ClientError::Handshake { .. })));
            }
            server.await.expect("server completed");
        }
    }
}
