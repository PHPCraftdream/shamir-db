/**
 * Connection wire types.
 *
 * Pure type declarations; the connecting code lives in `../client.ts` and
 * the platform entry points (`../../index.ts`, `../../browser.ts`).
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
