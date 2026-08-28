/**
 * Connection wire types.
 *
 * Pure type declarations; the connecting code lives in `../client.ts` and
 * the platform entry points (`../../index.ts`, `../../browser.ts`).
 *
 * PLATFORM-AGNOSTIC.
 */

/**
 * Options for {@link ShamirClient.resume} — fast reconnection using a
 * resumption ticket obtained from a previous session.
 */
export interface ResumeOptions {
  host: string;
  port: number;
  /** Resumption ticket bytes obtained from a previous session. */
  ticket: Uint8Array;
  /** Server public key pinned from the first connection. */
  serverPubKey: Uint8Array;
  tls?: { rejectUnauthorized?: boolean };
  /**
   * Origin header sent on the WS upgrade. Defaults to `https://${host}`.
   */
  origin?: string;
  /**
   * Per-request deadline in ms (Finding 2.2). A pending request that gets no
   * server response within this budget rejects with a {@link ShamirTimeoutError}
   * instead of hanging forever. Defaults to {@link DEFAULT_REQUEST_TIMEOUT_MS}.
   * Pass `0` to disable.
   */
  requestTimeoutMs?: number;
  /**
   * Deadline in ms for the initial connection attempt (Finding 2.2). Defaults
   * to {@link DEFAULT_CONNECT_TIMEOUT_MS}. Pass `0` to disable.
   */
  connectTimeoutMs?: number;
}

/**
 * Connection parameters (mirrors the napi `ConnectOptions`).
 *
 * Unlike the napi/Node client, this TS client does NOT verify the server's
 * `identity_sig` / do TOFU pinning of its Ed25519 identity (task #622) — a
 * prior `acceptNewHost`/`trustedPin` pair was declared but never consulted
 * anywhere, so it was removed rather than left dead. Real pinning needs a
 * browser storage backend and an `acceptNewHost` UX design pass; until
 * then this client is not MITM-resistant on the initial connection.
 */
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
  /**
   * Per-request deadline in ms (Finding 2.2). A pending request that gets no
   * server response within this budget rejects with a {@link ShamirTimeoutError}
   * instead of hanging forever. Defaults to {@link DEFAULT_REQUEST_TIMEOUT_MS}.
   * Pass `0` to disable.
   */
  requestTimeoutMs?: number;
  /**
   * Deadline in ms for the initial connection attempt (Finding 2.2). Defaults
   * to {@link DEFAULT_CONNECT_TIMEOUT_MS}. Pass `0` to disable.
   */
  connectTimeoutMs?: number;
}

/**
 * Connection parameters for {@link ShamirClient.connectLocal} — local IPC
 * transport (Unix domain socket / Windows Named Pipe), spec
 * TRANSPORT_UNIX.md. No TLS, no `host`/`port`/`tls`/`origin` — the OS-level
 * access boundary (file permissions / DACL, enforced server-side) replaces
 * transport encryption. Node-only: there is no browser equivalent (a
 * browser sandbox has no filesystem/pipe access).
 */
export interface ConnectLocalOptions {
  /**
   * Local endpoint — a filesystem path (Unix) or a Named Pipe name
   * (Windows, e.g. `\\.\pipe\shamir-db`). Same string works cross-platform;
   * callers don't branch on OS.
   */
  addr: string;
  username: string;
  password: string;
  /**
   * Per-request deadline in ms — see {@link ConnectOptions.requestTimeoutMs}.
   */
  requestTimeoutMs?: number;
  /**
   * Deadline in ms for the initial connection attempt — see
   * {@link ConnectOptions.connectTimeoutMs}.
   */
  connectTimeoutMs?: number;
}

/**
 * Default per-request deadline. Chosen comfortably above the server's default
 * `max_execution_time_secs` (30 s) so a legitimately slow `Execute` is bounded
 * by the SERVER, and this client timer only fires for genuinely stuck ops
 * (`Ping`, `TxCommit`, a lost response id) — see Finding 2.2.
 */
export const DEFAULT_REQUEST_TIMEOUT_MS = 35_000;

/** Default deadline for the initial connection attempt. */
export const DEFAULT_CONNECT_TIMEOUT_MS = 10_000;
