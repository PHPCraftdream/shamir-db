export type TokenTransportErrorCode =
  | 'token_read_only' | 'token_database_denied' | 'token_request_denied'
  | 'token_channel_denied' | 'token_plaintext_write_denied'
  | 'not_supported_over_rest' | 'invalid_request'
  | 'token_label_exists' | 'token_limit_reached' | 'token_ttl_invalid'
  | 'token_not_found' | 'token_self_service_disabled' | 'token_browser_session'
  | 'permission_denied' | 'hmac_required' | 'hmac_mismatch'
  | 'http_bad_request' | 'unauthorized' | 'http_not_found' | 'http_method_not_allowed'
  | 'network_error' | 'client_timeout' | 'http_timeout' | 'http_body_too_large'
  | 'http_unsupported_media_type' | 'http_bad_headers' | 'rate_limited' | 'server_unavailable'
  | 'protocol_error' | 'client_aborted' | 'tls_configuration_error' | 'connection_closed'
  | 'authentication_failed' | 'token_sid_mismatch' | 'token_revoked' | 'token_expired'
  | 'token_channel_unavailable' | 'first_frame_timeout' | 'preauth_overloaded' | 'server_busy'
  | 'timeout' | 'lock_timeout' | 'tx_conflict' | 'read_only_replica' | 'wal_retention_exceeded';

export function isTokenTransportErrorCode(code: string): code is TokenTransportErrorCode {
  return TOKEN_TRANSPORT_ERROR_CODES.has(code);
}

const TOKEN_TRANSPORT_ERROR_CODES: ReadonlySet<string> = new Set([
  'token_read_only','token_database_denied','token_request_denied','token_channel_denied','token_plaintext_write_denied',
  'not_supported_over_rest','invalid_request','token_label_exists','token_limit_reached','token_ttl_invalid','token_not_found',
  'token_self_service_disabled','token_browser_session','permission_denied','hmac_required','hmac_mismatch','http_bad_request',
  'unauthorized','http_not_found','http_method_not_allowed','network_error','client_timeout','http_timeout','http_body_too_large',
  'http_unsupported_media_type','http_bad_headers','rate_limited','server_unavailable','protocol_error','client_aborted',
  'tls_configuration_error','connection_closed','authentication_failed','token_sid_mismatch','token_revoked','token_expired',
  'token_channel_unavailable','first_frame_timeout','preauth_overloaded','server_busy','timeout','lock_timeout','tx_conflict',
  'read_only_replica','wal_retention_exceeded',
]);

export interface TokenTransportErrorOptions {
  status?: number;
  retryable?: boolean;
  retryAfterMs?: number;
  phase?: 'connect' | 'request' | 'close';
}

/** Sanitized transport failure; does not retain peer data or underlying causes. */
export class TokenTransportError extends Error {
  readonly status: number | undefined;
  readonly retryable: boolean;
  readonly retryAfterMs: number | undefined;
  readonly phase: 'connect' | 'request' | 'close';

  constructor(readonly code: TokenTransportErrorCode, message: string, options: TokenTransportErrorOptions = {}) {
    super(message);
    this.name = 'TokenTransportError';
    this.status = options.status;
    this.retryable = options.retryable ?? false;
    this.retryAfterMs = options.retryAfterMs;
    this.phase = options.phase ?? 'request';
  }
}
