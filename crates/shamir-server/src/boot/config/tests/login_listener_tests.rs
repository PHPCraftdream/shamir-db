use super::super::login_listener::{canonical_authority, parse_listener_id};
use super::super::HttpListenerSettings;
use crate::boot::config::{Config, ListenerConfig, ListenerKind, TokensConfig};

fn listener(addr: &str, tls: bool) -> ListenerConfig {
    ListenerConfig {
        kind: ListenerKind::Http,
        addr: addr.into(),
        profile: None,
        http: HttpListenerSettings {
            tls: Some(tls),
            ..HttpListenerSettings::default()
        },
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    }
}

fn config_with(listeners: Vec<ListenerConfig>) -> Config {
    let source = "data_dir: data\nkdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }\nlisteners: [{ kind: tcp, addr: 127.0.0.1:7340, profile: tls_exporter }]\ntls: { cert_path: cert.pem, key_path: key.pem }\n";
    let mut config: Config = ktav::from_str(source).expect("valid base config parses");
    config.listeners = listeners;
    config
}

#[test]
fn login_defaults_and_nested_unknown_keys() {
    let config: Config = ktav::from_str("data_dir: data\nkdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }\nlisteners: [{ kind: tcp, addr: 127.0.0.1:7340, profile: tls_exporter }]\ntls: { cert_path: cert.pem, key_path: key.pem }\n").unwrap();
    assert!(!config.listeners[0].http.login.enabled);
    assert!(config.listeners[0]
        .http
        .login
        .authority_allowlist
        .is_empty());
    assert!(!config.listeners[0].http.external_tls);
    assert!(config.security.login_enumeration_hardening);
    assert_eq!(TokensConfig::default().login_ttl, 86_400);
    assert!(!TokensConfig::default().login_allow_read_write);
    assert!(ktav::from_str::<ListenerConfig>(
        "{ kind: http, addr: 127.0.0.1:1, tls: false, login: { enabled: false, typo: true } }"
    )
    .is_err());
}

#[test]
fn listener_id_requires_exact_lowercase_hex() {
    assert_eq!(
        parse_listener_id("00000000000000000000000000000001").unwrap()[15],
        1
    );
    assert!(parse_listener_id("0000000000000000000000000000000A").is_err());
    assert!(parse_listener_id("00").is_err());
}

#[test]
fn authority_canonicalization_enforces_host_and_port_syntax() {
    assert_eq!(
        canonical_authority("LOCALHOST:443", true).unwrap(),
        "localhost"
    );
    assert_eq!(canonical_authority("[::1]:80", false).unwrap(), "[::1]");
    for bad in [
        "localhost/path",
        "2001:db8::1",
        "..",
        "-bad.test",
        "bad-.test",
        "bad..test",
        "localhost:+80",
        "localhost:",
        "[::1]:",
    ] {
        assert!(canonical_authority(bad, true).is_err(), "accepted {bad}");
    }
}

#[test]
fn login_plaintext_profile_and_head_timeout_are_validated_individually() {
    let mut value = listener("127.0.0.1:7340", false);
    value.http.login.enabled = true;
    assert!(config_with(vec![value.clone()]).validate().is_ok());
    value.addr = "0.0.0.0:7340".into();
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("plaintext HTTP bind"));
    value.addr = "127.0.0.1:7340".into();
    value.http.allow_plaintext_remote = true;
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("login cannot be enabled with allow_plaintext_remote"));
    value.http.allow_plaintext_remote = false;
    value.http.allow_plaintext_write = true;
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("allow_plaintext_write requires allow_plaintext_remote"));
    value.http.allow_plaintext_write = false;
    value.http.head_timeout_ms = Some(5_001);
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("at most 5000 ms"));
    value.http.head_timeout_ms = None;
    let mut inherited = config_with(vec![value.clone()]);
    inherited.security.connection.auth_init_timeout_ms = 5_001;
    assert!(inherited
        .validate()
        .unwrap_err()
        .to_string()
        .contains("at most 5000 ms"));
    value.kdf_override = Some(crate::boot::config::KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 19,
    });
    assert!(config_with(vec![value])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("kdf_override is forbidden"));
}

#[test]
fn login_tls_profiles_enforce_external_and_direct_authority_requirements() {
    let mut value = listener("127.0.0.1:7340", true);
    value.http.login.enabled = true;
    value.http.external_tls = true;
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("external_tls is invalid"));
    value.http.external_tls = false;
    value.http.allow_plaintext_remote = true;
    value.http.allow_plaintext_write = true;
    assert!(config_with(vec![value.clone()]).validate().is_err());
    value.http.allow_plaintext_remote = false;
    value.http.allow_plaintext_write = false;
    value.http.external_tls = true;
    value.http.tls = Some(false);
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("non-empty trusted_proxies"));
    value.http.trusted_proxies.push("127.0.0.1/32".into());
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("explicit non-empty authority_allowlist"));
    value
        .http
        .login
        .authority_allowlist
        .push("localhost".into());
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("canonical HTTPS origins"));
    value.browser_origin_allowlist = vec!["https://".into()];
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("canonical HTTPS origins"));
    value.browser_origin_allowlist = vec!["https://example.test/path".into()];
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("canonical HTTPS origins"));
    value.browser_origin_allowlist = vec!["https://EXAMPLE.test".into()];
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("canonical HTTPS origins"));
    value.browser_origin_allowlist = vec!["https://example.test".into()];
    assert!(config_with(vec![value.clone()]).validate().is_ok());
    value.http.external_tls = false;
    value.http.tls = Some(true);
    value.addr = "0.0.0.0:7340".into();
    value.http.login.authority_allowlist.clear();
    assert!(config_with(vec![value])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("requires explicit authority_allowlist"));
}

#[test]
fn login_authorities_reject_canonical_duplicates_and_invalid_listener_ids() {
    let mut value = listener("127.0.0.1:7340", false);
    value.http.login.enabled = true;
    value.http.login.authority_allowlist = vec!["LOCALHOST".into(), "localhost:80".into()];
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("canonical duplicates"));
    value.http.login.authority_allowlist.clear();
    value.http.login.listener_id = Some("0000000000000000000000000000000A".into());
    assert!(config_with(vec![value.clone()])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("lowercase hexadecimal"));
    value.http.login.listener_id = Some("00112233445566778899aabbccddeeff".into());
    let duplicate = ListenerConfig {
        addr: "127.0.0.1:7341".into(),
        ..value.clone()
    };
    assert!(config_with(vec![value, duplicate])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("duplicates another enabled listener"));
}

#[test]
fn login_ttl_range_and_hardening_defaults() {
    let mut tokens = TokensConfig::default();
    for ttl in [0, 86_401] {
        tokens.login_ttl = ttl;
        assert!(tokens.validate().is_err());
    }
    for ttl in [1, 86_400] {
        tokens.login_ttl = ttl;
        assert!(tokens.validate().is_ok());
    }
}

#[test]
fn external_tls_is_rejected_on_non_http_listener() {
    let mut value = ListenerConfig {
        kind: ListenerKind::Tcp,
        addr: "127.0.0.1:7340".into(),
        profile: Some(crate::boot::config::ProfileKind::Plain),
        http: HttpListenerSettings::default(),
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    };
    value.http.external_tls = true;
    assert!(config_with(vec![value])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("HTTP-only settings"));
}

#[test]
fn login_settings_are_rejected_on_non_http_listener() {
    let mut value = ListenerConfig {
        kind: ListenerKind::Tcp,
        addr: "127.0.0.1:7340".into(),
        profile: Some(crate::boot::config::ProfileKind::TlsExporter),
        http: HttpListenerSettings::default(),
        path: None,
        kdf_override: None,
        browser_origin_allowlist: Vec::new(),
    };
    value.http.login.enabled = true;
    assert!(config_with(vec![value])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("HTTP-only settings"));
}
