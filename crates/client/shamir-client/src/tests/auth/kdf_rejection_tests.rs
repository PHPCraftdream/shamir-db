//! Local IPC regression coverage for rejecting unsafe server KDF challenges.

use std::time::Duration;

use zeroize::Zeroizing;

use crate::client::transport::{WireAuthInit, WireChallenge};
use crate::{Client, ClientError, ConnectLocalOptions};
use shamir_connect::common::error::Error;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{limits, ProtocolVersion};
use shamir_transport_ipc::IpcListener;
use tokio::io::{split, AsyncReadExt};
use tokio::time::timeout;

const GUARD: Duration = Duration::from_secs(5);

fn bad_tuples() -> [(&'static str, KdfParams); 7] {
    let mut below_memory = KdfParams::DEFAULT;
    below_memory.memory_kb = limits::KDF_MIN_MEMORY_KB - 1;
    let mut above_memory = KdfParams::DEFAULT;
    above_memory.memory_kb = limits::KDF_MAX_MEMORY_KB + 1;
    let mut below_time = KdfParams::DEFAULT;
    below_time.time = limits::KDF_MIN_TIME - 1;
    let mut above_time = KdfParams::DEFAULT;
    above_time.time = limits::KDF_MAX_TIME + 1;
    let mut below_parallelism = KdfParams::DEFAULT;
    below_parallelism.parallelism = limits::KDF_MIN_PARALLELISM - 1;
    let mut above_parallelism = KdfParams::DEFAULT;
    above_parallelism.parallelism = limits::KDF_MAX_PARALLEL + 1;
    let mut wrong_version = KdfParams::DEFAULT;
    wrong_version.argon2_version = 0x12;
    [
        ("below memory", below_memory),
        ("above memory", above_memory),
        ("below time", below_time),
        ("above time", above_time),
        ("below parallelism", below_parallelism),
        ("above parallelism", above_parallelism),
        ("wrong version", wrong_version),
    ]
}

async fn read_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
    shamir_transport_tcp::framing::read_frame(reader, 64 * 1024)
        .await
        .expect("read frame")
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) {
    shamir_transport_tcp::framing::write_frame(writer, bytes)
        .await
        .expect("write frame");
}

#[tokio::test]
async fn local_connect_rejects_invalid_server_kdf_before_sending_proof() {
    for (label, params) in bad_tuples() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("kdf.sock").to_string_lossy().into_owned();
        let mut listener = IpcListener::bind(&path)
            .await
            .expect("bind fake IPC listener");

        let server = async {
            let stream = listener.accept().await.expect("accept local client");
            let (mut reader, mut writer) = split(stream);
            let init: WireAuthInit =
                rmp_serde::from_slice(&read_frame(&mut reader).await).expect("typed auth_init");
            assert_eq!(init.version, ProtocolVersion::V1.as_u8());
            let challenge = WireChallenge {
                salt: vec![5; 16],
                memory_kb: params.memory_kb,
                time: params.time,
                parallelism: params.parallelism,
                argon2_version: params.argon2_version,
                server_nonce: vec![7; 32],
            };
            let bytes = rmp_serde::to_vec(&challenge).expect("serialize challenge");
            write_frame(&mut writer, &bytes).await;
            let mut byte = [0];
            let read = reader
                .read(&mut byte)
                .await
                .expect("observe client disconnect");
            assert_eq!(read, 0, "{label}: client must not send proof");
        };
        let client = Client::connect_local(ConnectLocalOptions {
            addr: path,
            username: "kdf-test".into(),
            password: Zeroizing::new(b"correct horse battery staple".to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(GUARD),
            request_timeout: None,
        });

        let (_, client_result) = timeout(GUARD, async { tokio::join!(server, client) })
            .await
            .expect("fake peer and connect_local must not hang");
        match client_result {
            Err(ClientError::Handshake {
                context,
                source: Some(source),
            }) => {
                assert!(
                    context.contains("kdf_params_rejected"),
                    "{label}: {context}"
                );
                assert!(
                    matches!(
                        source.downcast_ref::<Error>(),
                        Some(Error::KdfParamsRejected)
                    ),
                    "{label}: unexpected typed handshake source: {source}"
                );
            }
            Err(error) => panic!("{label}: expected typed KDF rejection, got {error}"),
            Ok(_) => panic!("{label}: client unexpectedly accepted invalid KDF parameters"),
        }
    }
}
