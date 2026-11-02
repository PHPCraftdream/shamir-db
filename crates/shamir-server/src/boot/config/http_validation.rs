use crate::boot::config::ConfigError;

pub(super) fn validate_http_settings(
    idx: usize,
    settings: &super::http_listener::HttpListenerSettings,
    listener: &crate::boot::config::ListenerConfig,
    auth_init_timeout_ms: u64,
) -> Result<(), ConfigError> {
    settings.validate(idx, listener, auth_init_timeout_ms)
}
