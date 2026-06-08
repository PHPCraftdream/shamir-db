/**
 * Shared type definitions used by the core and exported from entry points.
 *
 * PLATFORM-AGNOSTIC.
 */

/** Connection parameters (mirrors the napi `ConnectOptions`). */
export interface ConnectOptions {
  host: string;
  port: number;
  username: string;
  password: string;
  tls?: { rejectUnauthorized?: boolean };
  /**
   * Origin header sent on the WS upgrade. The server's browser endpoint
   * REQUIRES an Origin in its allowlist (browser.rs §9). Defaults to
   * `https://${host}`.
   */
  origin?: string;
  acceptNewHost?: boolean;
  trustedPin?: Uint8Array;
}
