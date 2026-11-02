use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::token::{parse_token, Endpoint, TokenMode, VerifyFailure};

use super::common::{request, Harness};

const GOLDEN_401_BODY: &[u8] = b"\x81\xa5error\xacunauthorized";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_unknown_bad_mac_revoked_expired_are_uniform_unauthorized() {
    let mut bodies = Vec::new();
    for case in ["unknown", "bad-mac", "revoked", "expired"] {
        let harness = Harness::launch().await;
        let token = harness.token("uniform-auth", TokenMode::ReadOnly).await;
        let credentials = match case {
            "unknown" => {
                let parsed = parse_token(token.as_bytes()).expect("issued token parses");
                let encoded_id = URL_SAFE_NO_PAD.encode([0xA5; 16]);
                format!(
                    "shk1.{encoded_id}.{}",
                    URL_SAFE_NO_PAD.encode(parsed.secret.as_bytes())
                )
            }
            "bad-mac" => {
                let mut bytes = token.into_bytes();
                let secret_start = bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'.')
                    .nth(1)
                    .expect("secret segment")
                    .0
                    + 1;
                bytes[secret_start] = if bytes[secret_start] == b'a' {
                    b'b'
                } else {
                    b'a'
                };
                String::from_utf8(bytes).expect("canonical token remains ASCII")
            }
            "revoked" => {
                let parsed = parse_token(token.as_bytes()).expect("issued token parses");
                harness
                    .handle
                    .token_service
                    .revoke(
                        parsed.id,
                        shamir_connect::common::time::UnixNanos::try_now()
                            .expect("clock")
                            .as_u64(),
                    )
                    .await
                    .expect("revoke token");
                token
            }
            "expired" => {
                let parsed = parse_token(token.as_bytes()).expect("issued token parses");
                let mut record = harness
                    .handle
                    .token_service
                    .mirror()
                    .get(&parsed.id)
                    .expect("issued token in mirror")
                    .record()
                    .clone();
                record.created_at_ns = 1;
                record.expires_at_ns = 2;
                assert!(harness.handle.token_service.mirror().remove(&parsed.id));
                assert!(harness.handle.token_service.mirror().insert(record));
                token
            }
            _ => unreachable!(),
        };
        if case == "unknown" {
            let parsed = parse_token(credentials.as_bytes()).expect("unknown token parses");
            assert!(
                harness
                    .handle
                    .token_service
                    .mirror()
                    .get(&parsed.id)
                    .is_none(),
                "fixed unknown id must not collide with mirror"
            );
        }
        let expected = match case {
            "unknown" | "bad-mac" => VerifyFailure::Invalid,
            "revoked" => VerifyFailure::Revoked,
            "expired" => VerifyFailure::Expired,
            _ => unreachable!(),
        };
        let now_ns = UnixNanos::try_now().expect("clock").as_u64();
        let verified =
            harness
                .handle
                .token_service
                .verify(credentials.as_bytes(), Endpoint::Rest, now_ns);
        assert!(
            matches!((verified, expected), (Err(actual), expected) if actual == expected),
            "{case}: direct token verification classification mismatch"
        );
        let (status, headers, body) = request(
            harness.addr(),
            Some(&credentials),
            &super::common::ping_body(),
        )
        .await;
        assert_eq!(status, 401, "{case}");
        assert_eq!(body, GOLDEN_401_BODY, "{case}");
        let headers = headers.to_ascii_lowercase();
        assert!(
            headers
                .lines()
                .any(|line| line.trim() == "www-authenticate: bearer"),
            "{case}: {headers}"
        );
        assert!(
            headers
                .lines()
                .any(|line| line.trim() == "connection: close"),
            "{case}: {headers}"
        );
        bodies.push(body);
        assert!(
            harness.handle.shutdown().await.is_clean(),
            "{case} shutdown"
        );
    }
    assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
}
