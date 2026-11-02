use super::super::HttpListenerSettings;
use crate::boot::config::{ConfigError, ListenerConfig};

#[test]
fn http_listener_defaults_apply() {
    let defaults = HttpListenerSettings::default();
    assert_eq!(defaults.max_request_body_bytes, 16 * 1024 * 1024);
    assert_eq!(defaults.head_timeout_ms, None);
    assert_eq!(defaults.body_timeout_ms, 30_000);
    assert_eq!(defaults.request_timeout_ms, 120_000);
    assert!(!defaults.tls);
    assert!(!defaults.allow_plaintext_remote);
    assert!(!defaults.allow_plaintext_write);
    assert!(defaults.trusted_proxies.is_empty());
}

#[test]
fn http_server_config_mapping_uses_limits_and_inherited_head_timeout() {
    let settings = HttpListenerSettings {
        max_request_body_bytes: 1024,
        head_timeout_ms: None,
        body_timeout_ms: 20,
        request_timeout_ms: 200,
        tls: false,
        allow_plaintext_remote: false,
        allow_plaintext_write: false,
        trusted_proxies: Vec::new(),
    };
    let config = super::super::to_http_server_config(
        &settings,
        shamir_transport_http::EndpointSet {
            rest: true,
            ws: false,
        },
        std::sync::Arc::new(shamir_transport_http::BodyBudget::new(2048)),
        17,
    );
    assert_eq!(config.limits.max_body_bytes, 1024);
    assert_eq!(config.limits.max_head_bytes, 8192);
    assert_eq!(config.limits.max_headers, 64);
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
    assert!(config.endpoints.rest);
    assert!(!config.endpoints.ws);
    assert_eq!(config.limits.body_budget.in_use(), 0);
}

#[test]
fn http_listener_validation_rejects_policy_and_limits() {
    let mut listener = ListenerConfig {
        kind: crate::boot::config::ListenerKind::Http,
        addr: "0.0.0.0:7340".into(),
        profile: None,
        http: HttpListenerSettings::default(),
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    };
    let mut settings = listener.http.clone();
    assert!(matches!(
        settings.validate(0, &listener, 5_000),
        Err(ConfigError::Validation(_))
    ));
    settings.allow_plaintext_remote = true;
    settings.max_request_body_bytes = 0;
    assert!(settings.validate(0, &listener, 5_000).is_err());
    settings.max_request_body_bytes = 1024;
    settings.allow_plaintext_write = true;
    settings.allow_plaintext_remote = false;
    assert!(settings.validate(0, &listener, 5_000).is_err());
    settings.allow_plaintext_remote = true;
    settings.head_timeout_ms = Some(121_000);
    assert!(settings.validate(0, &listener, 5_000).is_err());
    settings.head_timeout_ms = None;
    settings.trusted_proxies = vec!["bad-cidr".into()];
    assert!(settings.validate(0, &listener, 5_000).is_err());
    listener.addr = "127.0.0.1:7340".into();
    settings.trusted_proxies = vec!["10.0.0.0/8".into()];
    assert!(settings.validate(0, &listener, 5_000).is_ok());
    settings.tls = true;
    assert!(settings.validate(0, &listener, 5_000).is_ok());
    settings.allow_plaintext_remote = true;
    assert!(settings.validate(0, &listener, 5_000).is_err());
}
