export type LoginErrorCode = 'invalid_request' | 'authentication_failed' | 'rate_limited' | 'server_busy' | 'unsupported_server' | 'protocol_error' | 'kdf_params_rejected' | 'security_mismatch' | 'client_aborted' | 'client_timeout' | 'http_timeout' | 'http_body_too_large' | 'http_bad_headers' | 'http_unsupported_media_type' | 'http_method_not_allowed' | 'http_not_found' | 'network_error' | 'token_expired' | 'origin_forbidden' | 'authority_forbidden' | 'proxy_forbidden' | 'login_write_forbidden' | 'token_self_service_disabled' | 'login_read_only_replica' | 'login_account_denied' | 'token_label_exists' | 'token_limit_reached';

export class LoginError extends Error {
  readonly retryAfterMs?: number;
  readonly relogin: boolean;
  constructor(readonly code: LoginErrorCode, retryAfterMs?: number, readonly status?: number) {
    super(code === 'token_expired' ? 'Token expired; log in again' : `Login failed: ${code}`);
    this.name = 'LoginError';
    if (retryAfterMs !== undefined) this.retryAfterMs = retryAfterMs;
    this.relogin = code === 'token_expired';
  }
}
