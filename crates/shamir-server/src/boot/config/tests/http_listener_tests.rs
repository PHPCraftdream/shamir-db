use super::super::{to_http_server_config, HttpListenerSettings};
use crate::boot::config::{Config, ListenerConfig, ListenerKind, ProfileKind};
use shamir_query_types::wire::{DbRequest, DbResponse};

fn listener(kind: ListenerKind, addr: &str, profile: Option<ProfileKind>) -> ListenerConfig {
    ListenerConfig {
        kind,
        addr: addr.into(),
        profile,
        http: HttpListenerSettings::default(),
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    }
}

fn base_config() -> Config {
    let source = r#"
        data_dir: data
        kdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }
        listeners: [{ kind: tcp, addr: 127.0.0.1:7340, profile: tls_exporter }]
        tls: { cert_path: cert.pem, key_path: key.pem }
    "#;
    ktav::from_str(source).expect("base config parses")
}

#[test]
fn http_listener_defaults_apply() {
    let defaults = HttpListenerSettings::default();
    assert_eq!(defaults.max_request_body_bytes, 16 * 1024 * 1024);
    assert_eq!(defaults.head_timeout_ms, None);
    assert_eq!(defaults.body_timeout_ms, 30_000);
    assert_eq!(defaults.request_timeout_ms, 120_000);
    assert_eq!(defaults.tls, None);
    assert!(!defaults.allow_plaintext_remote);
    assert!(!defaults.allow_plaintext_write);
    assert!(!defaults.allow_self_signed);
    assert!(defaults.trusted_proxies.is_empty());
    assert!(!defaults.external_tls);
    assert!(!defaults.login.enabled);
    assert!(defaults.login.authority_allowlist.is_empty());
    assert_eq!(defaults.login.listener_id, None);
}

#[test]
fn minimal_flat_ktav_http_listener_parses_defaults() {
    let source = r#"{
        kind: http
        addr: 127.0.0.1:7340
        tls: false
    }"#;
    let parsed: ListenerConfig = ktav::from_str(source).expect("minimal listener parses");
    assert_eq!(parsed.kind, ListenerKind::Http);
    assert_eq!(parsed.http.tls, Some(false));
    assert_eq!(parsed.http.max_request_body_bytes, 16 * 1024 * 1024);
    assert_eq!(parsed.http.body_timeout_ms, 30_000);
}

#[test]
fn full_flat_ktav_http_listener_parses_numeric_settings() {
    let source = r#"{
        kind: http
        addr: 127.0.0.1:7340
        tls: true
        max_request_body_bytes: 8192
        head_timeout_ms: 50
        body_timeout_ms: 200
        request_timeout_ms: 1000
        allow_self_signed: true
        trusted_proxies: [10.0.0.0/8]
    }"#;
    let parsed: ListenerConfig = ktav::from_str(source).expect("full listener parses");
    assert_eq!(parsed.http.tls, Some(true));
    assert_eq!(parsed.http.max_request_body_bytes, 8192);
    assert_eq!(parsed.http.head_timeout_ms, Some(50));
    assert_eq!(parsed.http.body_timeout_ms, 200);
    assert_eq!(parsed.http.request_timeout_ms, 1000);
    assert!(parsed.http.allow_self_signed);
    assert_eq!(parsed.http.trusted_proxies, vec!["10.0.0.0/8"]);
}

#[test]
fn flat_ktav_numeric_buffer_handles_signed_range_and_rejects_negative_and_overflow() {
    // ktav turns integers above i64::MAX into strings; flatten buffers them as such.
    let source =
        "{ kind: http, addr: 127.0.0.1:7340, tls: true, request_timeout_ms: 9223372036854775807 }";
    let parsed: ListenerConfig = ktav::from_str(source).expect("largest ktav integer parses");
    assert_eq!(parsed.http.request_timeout_ms, i64::MAX as u64);
    let overflow =
        "{ kind: http, addr: 127.0.0.1:7340, tls: true, request_timeout_ms: 18446744073709551615 }";
    assert!(ktav::from_str::<ListenerConfig>(overflow).is_err());
    let negative = "{ kind: http, addr: 127.0.0.1:7340, tls: true, max_request_body_bytes: -1 }";
    assert!(ktav::from_str::<ListenerConfig>(negative).is_err());
}

#[test]
fn flat_http_parser_covers_plaintext_rails_and_tls_absence() {
    let source = "{ kind: http, addr: 127.0.0.1:7340, tls: false, allow_plaintext_remote: true, allow_plaintext_write: true }";
    let parsed: ListenerConfig = ktav::from_str(source).expect("plaintext options parse");
    assert_eq!(parsed.http.tls, Some(false));
    assert!(parsed.http.allow_plaintext_remote);
    assert!(parsed.http.allow_plaintext_write);
    let missing = "{ kind: http, addr: 127.0.0.1:7340 }";
    let parsed: ListenerConfig =
        ktav::from_str(missing).expect("missing selector deserializes for validation");
    assert_eq!(parsed.http.tls, None);
}

#[test]
fn http_server_config_mapping_uses_limits_and_messagepack_bodies() {
    let settings = HttpListenerSettings {
        max_request_body_bytes: 1024,
        head_timeout_ms: None,
        body_timeout_ms: 20,
        request_timeout_ms: 200,
        tls: Some(false),
        allow_plaintext_remote: false,
        allow_plaintext_write: false,
        allow_self_signed: false,
        trusted_proxies: Vec::new(),
        external_tls: false,
        login: Default::default(),
    };
    let forbidden = std::sync::Arc::new(|code: &str| {
        bytes::Bytes::from(
            rmp_serde::to_vec_named(&DbResponse::Error {
                code: code.to_owned(),
                message: "request forbidden".to_owned(),
            })
            .expect("encode forbidden response"),
        )
    });
    let ping = bytes::Bytes::from(rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping"));
    let config = to_http_server_config(
        &settings,
        shamir_transport_http::EndpointSet {
            rest: true,
            ws: false,
        },
        std::sync::Arc::new(shamir_transport_http::BodyBudget::new(2048)),
        17,
        None,
        forbidden,
        ping.clone(),
    )
    .expect("TLS setting agrees");
    assert_eq!(config.limits.max_body_bytes, 1024);
    assert_eq!(
        config.limits.head_timeout,
        std::time::Duration::from_millis(17)
    );
    assert_eq!(
        config.limits.body_timeout,
        std::time::Duration::from_millis(20)
    );
    assert_eq!(
        config.limits.request_timeout,
        std::time::Duration::from_millis(200)
    );
    assert_eq!(
        config.limits.write_timeout,
        std::time::Duration::from_millis(200)
    );
    assert!(!config.hsts);
    assert!(config.tls.is_none());
    assert_eq!(config.ping_body, ping);
    assert!(matches!(
        rmp_serde::from_slice::<DbRequest>(&config.ping_body).expect("decode ping"),
        DbRequest::Ping
    ));
    let body = (config.forbidden_body)("token_request_denied");
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&body).expect("decode forbidden"), DbResponse::Error { code, .. } if code == "token_request_denied")
    );
}

#[test]
fn mapping_tls_true_propagates_tls_hsts_and_write_timeout() {
    let (cert, key) =
        shamir_transport_tcp::tls::generate_self_signed_server_cert(vec!["localhost".into()])
            .expect("test cert");
    let server_tls = shamir_transport_tcp::tls::make_server_config_from_pem(&cert, &key)
        .expect("test TLS config");
    let settings = HttpListenerSettings {
        tls: Some(true),
        allow_plaintext_remote: false,
        allow_plaintext_write: false,
        ..HttpListenerSettings::default()
    };
    let forbidden = std::sync::Arc::new(|_: &str| bytes::Bytes::new());
    let ping = bytes::Bytes::from(rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping"));
    let config = to_http_server_config(
        &settings,
        shamir_transport_http::EndpointSet {
            rest: true,
            ws: false,
        },
        std::sync::Arc::new(shamir_transport_http::BodyBudget::new(2048)),
        5000,
        Some(server_tls.clone()),
        forbidden,
        ping,
    )
    .expect("TLS agrees");
    assert!(std::sync::Arc::ptr_eq(
        config.tls.as_ref().expect("TLS retained"),
        &server_tls
    ));
    assert!(config.hsts);
    assert_eq!(config.limits.write_timeout, config.limits.request_timeout);
}

#[test]
fn mapping_rejects_tls_configuration_mismatch() {
    let endpoints = shamir_transport_http::EndpointSet {
        rest: true,
        ws: false,
    };
    let body_budget = std::sync::Arc::new(shamir_transport_http::BodyBudget::new(2048));
    let settings = HttpListenerSettings {
        tls: Some(true),
        ..HttpListenerSettings::default()
    };
    let result = to_http_server_config(
        &settings,
        endpoints,
        std::sync::Arc::clone(&body_budget),
        5000,
        None,
        std::sync::Arc::new(|_: &str| bytes::Bytes::new()),
        bytes::Bytes::new(),
    );
    assert!(result
        .err()
        .expect("enabled TLS without config rejected")
        .to_string()
        .contains("does not agree"));

    let settings = HttpListenerSettings {
        tls: Some(false),
        ..HttpListenerSettings::default()
    };
    let result = to_http_server_config(
        &settings,
        endpoints,
        std::sync::Arc::clone(&body_budget),
        5000,
        None,
        std::sync::Arc::new(|_: &str| bytes::Bytes::new()),
        bytes::Bytes::new(),
    );
    assert!(result.is_ok(), "disabled TLS without config agrees");

    let settings = HttpListenerSettings::default();
    let result = to_http_server_config(
        &settings,
        endpoints,
        body_budget,
        5000,
        None,
        std::sync::Arc::new(|_: &str| bytes::Bytes::new()),
        bytes::Bytes::new(),
    );
    assert!(result
        .err()
        .expect("missing TLS selector rejected")
        .to_string()
        .contains("does not agree"));
}

#[test]
fn http_listener_validation_rejects_isolated_policy_invariants() {
    let mut listener = listener(ListenerKind::Http, "127.0.0.1:7340", None);
    let mut settings = listener.http.clone();
    settings.head_timeout_ms = Some(500);
    listener.addr = "127.0.0.1:7340".into();
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("requires tls"));
    settings.tls = Some(false);
    listener.addr = "0.0.0.0:7340".into();
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("not loopback"));
    settings.allow_plaintext_remote = true;
    assert!(settings.validate(0, &listener, 5000).is_ok());
    settings.max_request_body_bytes = 0;
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("max_request_body_bytes"));
    settings = HttpListenerSettings {
        tls: Some(false),
        allow_plaintext_remote: false,
        allow_plaintext_write: true,
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("requires allow_plaintext_remote"));
    listener.addr = "127.0.0.1:7340".into();
    settings = HttpListenerSettings {
        tls: Some(false),
        body_timeout_ms: 0,
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("body_timeout_ms=0"));
    settings = HttpListenerSettings {
        tls: Some(false),
        request_timeout_ms: 0,
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("request_timeout_ms=0"));
    settings = HttpListenerSettings {
        tls: Some(false),
        body_timeout_ms: 1001,
        request_timeout_ms: 1000,
        head_timeout_ms: Some(500),
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("body_timeout_ms=1001"));
    settings = HttpListenerSettings {
        tls: Some(false),
        head_timeout_ms: Some(1001),
        body_timeout_ms: 500,
        request_timeout_ms: 1000,
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("head_timeout_ms=1001"));
    settings = HttpListenerSettings {
        tls: Some(false),
        head_timeout_ms: None,
        body_timeout_ms: 500,
        request_timeout_ms: 1000,
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("security.connection.auth_init_timeout_ms inheritance"));
    listener.addr = "not-a-socket".into();
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("invalid HTTP bind address"));
    listener.addr = "127.0.0.1:7340".into();
    settings = HttpListenerSettings {
        tls: Some(false),
        trusted_proxies: vec!["bad/8".into()],
        ..HttpListenerSettings::default()
    };
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("invalid address"));
    settings.trusted_proxies = vec!["10.0.0.0/33".into()];
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("prefix exceeds 32"));
    settings.trusted_proxies = vec!["2001:db8::/129".into()];
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("prefix exceeds 128"));
    settings.trusted_proxies.clear();
    settings.allow_plaintext_remote = false;
    settings.allow_plaintext_write = false;
    settings.tls = Some(true);
    assert!(settings.validate(0, &listener, 5000).is_ok());
    settings.allow_plaintext_remote = true;
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("only valid when tls is false"));
    settings.allow_plaintext_remote = false;
    listener.profile = Some(ProfileKind::TlsExporter);
    assert!(settings
        .validate(0, &listener, 5000)
        .unwrap_err()
        .to_string()
        .contains("profile must be absent"));
}

#[test]
fn http_listener_rejects_zero_head_timeout_and_malformed_proxy_cidrs() {
    let listener = listener(ListenerKind::Http, "127.0.0.1:7340", None);
    let cases = [
        (
            HttpListenerSettings {
                tls: Some(false),
                head_timeout_ms: Some(0),
                ..HttpListenerSettings::default()
            },
            "greater than zero",
        ),
        (
            HttpListenerSettings {
                tls: Some(false),
                trusted_proxies: vec!["127.0.0.1".into()],
                ..HttpListenerSettings::default()
            },
            "must be CIDR",
        ),
        (
            HttpListenerSettings {
                tls: Some(false),
                trusted_proxies: vec!["127.0.0.1/33".into()],
                ..HttpListenerSettings::default()
            },
            "prefix exceeds 32",
        ),
    ];
    for (settings, expected_message) in cases {
        let error = settings
            .validate(0, &listener, 5000)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected_message), "{error}");
    }
}

#[test]
fn non_http_kinds_reject_http_only_keys_via_config_validation() {
    for kind in [ListenerKind::Tcp, ListenerKind::Ws, ListenerKind::Unix] {
        let (addr, profile, path) = match kind {
            ListenerKind::Tcp => ("127.0.0.1:7340", Some(ProfileKind::Plain), None),
            ListenerKind::Ws => (
                "127.0.0.1:7340",
                Some(ProfileKind::TlsExporter),
                Some("/shamir/v1".to_owned()),
            ),
            ListenerKind::Unix => (
                if cfg!(windows) {
                    r"\\.\pipe\shamir-config-validation"
                } else {
                    "/tmp/shamir-config-validation.sock"
                },
                Some(ProfileKind::Plain),
                None,
            ),
            ListenerKind::Http => unreachable!(),
        };
        let mut parsed = listener(kind, addr, profile);
        parsed.path = path;
        for http in [
            HttpListenerSettings {
                tls: Some(false),
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                tls: Some(true),
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                allow_self_signed: true,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                allow_plaintext_remote: true,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                allow_plaintext_write: true,
                allow_plaintext_remote: true,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                head_timeout_ms: Some(10),
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                body_timeout_ms: 10,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                request_timeout_ms: 10,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                max_request_body_bytes: 10,
                ..HttpListenerSettings::default()
            },
            HttpListenerSettings {
                trusted_proxies: vec!["127.0.0.1/32".into()],
                ..HttpListenerSettings::default()
            },
        ] {
            parsed.http = http;
            let error = super::super::validate_listener(0, &parsed, 5000).unwrap_err();
            assert!(error.to_string().contains("HTTP-only settings"));
        }
        parsed.http = HttpListenerSettings::default();
        if kind == ListenerKind::Ws {
            parsed.path = Some("/shamir/v1".to_owned());
        }
        let config = Config {
            listeners: vec![parsed],
            ..base_config()
        };
        assert!(config.validate().is_ok());
    }
}

#[test]
fn tcp_and_ws_without_profiles_report_requires_profile() {
    for kind in [ListenerKind::Tcp, ListenerKind::Ws] {
        let parsed = listener(kind, "127.0.0.1:7340", None);
        let config = Config {
            listeners: vec![parsed],
            ..base_config()
        };
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("requires profile"));
    }
}
