/**
 * ShamirDB Node.js client SDK.
 *
 * Native binding to the Rust `shamir-client` crate. Implements the full
 * TLS 1.3 + SCRAM-Argon2id + Ed25519 channel binding handshake; JS
 * never touches crypto.
 */

/** Connection parameters. */
export interface ConnectOptions {
  /** Server host (e.g. "127.0.0.1" or "db.example.com"). */
  host: string;
  /** Server port. */
  port: number;
  /** SNI hostname for TLS — usually matches the cert's CN. */
  serverName: string;
  /** Username (raw — server-side normalisation applies). */
  username: string;
  /** Plaintext password. Zeroised in the native side after handshake. */
  password: string;
  /**
   * Trust-on-first-use: accept whatever Ed25519 pubkey the server
   * presents on first connection. Once you persist the pin, switch
   * to `false` and pass `trustedPin`.
   */
  acceptNewHost?: boolean;
  /**
   * Pre-pinned `SHA256(server_ed25519_pub_key)` — exactly 32 bytes.
   * When supplied, mismatch fails the handshake.
   */
  trustedPin?: Buffer;
}

/** A connected, authenticated client. */
export class ShamirClient {
  /**
   * Run the full TCP→TLS→SCRAM handshake. Resolves to a connected
   * client.
   */
  static connect(opts: ConnectOptions): Promise<ShamirClient>;

  /**
   * `SHA256(server_ed25519_pub_key)` — 32 bytes. Persist this and
   * pass back as `trustedPin` on subsequent connections.
   */
  serverPubKeyPin(): Buffer;

  /** 32-byte session id assigned by the server. */
  sessionId(): Buffer;

  /** Absolute session expiry (unix nanoseconds, BigInt). */
  expiresAtNs(): bigint;

  /** Resumption ticket bytes (if the server issued one). */
  resumptionTicket(): Buffer | null;

  /** Resumption expiry (paired with `resumptionTicket`). */
  resumptionExpiresAtNs(): bigint | null;

  /** Health check. */
  ping(): Promise<void>;

  /**
   * Execute a `BatchRequest` (passed as a JS object) against the
   * named database. Returns the full `BatchResponse` as a JS object.
   */
  execute(db: string, batch: object): Promise<object>;

  /**
   * Create a new SCRAM-authenticatable user. Requires the current
   * session to belong to a superuser. Returns the stable 16-byte
   * `user_id` as a Buffer.
   */
  createScramUser(name: string, password: string, roles: string[]): Promise<Buffer>;

  /** Close the TLS write half cleanly. Idempotent. */
  close(): Promise<void>;
}
