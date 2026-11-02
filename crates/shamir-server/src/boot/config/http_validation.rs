use super::http_listener::HttpListenerSettings;
use crate::boot::config::{ConfigError, ListenerConfig, ListenerKind};

/// Rejects HTTP-only settings on non-HTTP listener kinds and checks shared profile requirements.
pub(super) fn validate_listener_http_fields(
    idx: usize,
    settings: &HttpListenerSettings,
    listener: &ListenerConfig,
) -> Result<(), ConfigError> {
    if listener.kind == ListenerKind::Http {
        if listener.profile.is_some() {
            return Err(ConfigError::Validation(format!(
                "listeners[{idx}]: profile must be absent for kind=http"
            )));
        }
        return Ok(());
    }

    let defaults = HttpListenerSettings::default();
    let has_http_only_settings = settings.tls.is_some()
        || settings.max_request_body_bytes != defaults.max_request_body_bytes
        || settings.head_timeout_ms.is_some()
        || settings.body_timeout_ms != defaults.body_timeout_ms
        || settings.request_timeout_ms != defaults.request_timeout_ms
        || settings.allow_plaintext_remote
        || settings.allow_plaintext_write
        || settings.allow_self_signed
        || !settings.trusted_proxies.is_empty();
    if has_http_only_settings {
        return Err(ConfigError::Validation(format!(
            "listeners[{idx}]: HTTP-only settings are not valid for kind={:?}",
            listener.kind
        )));
    }
    Ok(())
}

/// Validates HTTP transport policy after generic per-kind field checks.
pub(super) fn validate_http_settings(
    idx: usize,
    settings: &HttpListenerSettings,
    listener: &ListenerConfig,
    auth_init_timeout_ms: u64,
) -> Result<(), ConfigError> {
    settings.validate(idx, listener, auth_init_timeout_ms)
}
