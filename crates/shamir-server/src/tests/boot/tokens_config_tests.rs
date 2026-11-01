use crate::config::{Config, TokensConfig};

#[test]
fn tokens_config_deserialization_applies_each_field_default() {
    let config: TokensConfig = ktav::from_str("{}").expect("empty token section");
    assert_eq!(config.default_ttl_days, 30);
    assert_eq!(config.max_ttl_days, 365);
    assert_eq!(config.max_per_user, 20);
    assert!(config.allow_self_service);
    assert_eq!(config.session_max_age_hours, 24);
    assert_eq!(config.max_sockets_per_token, 8);
    assert_eq!(config.max_sockets_per_owner, 32);
}

#[test]
fn tokens_config_defaults_match_token_api_contract() {
    let config = TokensConfig::default();
    assert_eq!(config.default_ttl_days, 30);
    assert_eq!(config.max_ttl_days, 365);
    assert_eq!(config.max_per_user, 20);
    assert!(config.allow_self_service);
    assert_eq!(config.session_max_age_hours, 24);
    assert_eq!(config.max_sockets_per_token, 8);
    assert_eq!(config.max_sockets_per_owner, 32);
    config.validate().expect("defaults valid");
}

#[test]
fn tokens_config_is_parsed_from_top_level_and_validated() {
    let src = "\
data_dir: /tmp/shamir\nkdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }\nlisteners: [{ kind: tcp, addr: 127.0.0.1:7331, profile: tls_exporter }]\ntls: { cert_path: /tmp/cert.pem, key_path: /tmp/key.pem }\ntokens: { default_ttl_days: 10, max_ttl_days: 20, max_per_user: 3, allow_self_service: false, session_max_age_hours: 7, max_sockets_per_token: 2, max_sockets_per_owner: 5 }\n";
    let config: Config = ktav::from_str(src).expect("parse tokens section");
    config.validate().expect("config valid");
    assert_eq!(config.tokens.default_ttl_days, 10);
    assert!(!config.tokens.allow_self_service);
}

#[test]
fn tokens_config_rejects_zero_and_inconsistent_bounds() {
    let mut tokens = TokensConfig::default();
    tokens.default_ttl_days = 0;
    assert!(tokens.validate().is_err());
    tokens = TokensConfig::default();
    tokens.default_ttl_days = 366;
    assert!(tokens.validate().is_err());
    tokens = TokensConfig::default();
    tokens.max_sockets_per_owner = 0;
    assert!(tokens.validate().is_err());
}
