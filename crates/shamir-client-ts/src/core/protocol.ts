/**
 * SCRAM handshake state machine (auth_init → challenge → client_proof → auth_ok).
 *
 * Operates over a WsFramer + Platform. No platform-specific imports.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Platform } from './platform.js';
import { WsFramer, encode, decode } from './framing.js';
import {
  buildAuthMessage,
  computeClientProof,
  verifyServerSignature,
  ARGON2_VERSION_13,
  BINDING_MODE_TLS_NO_EXPORT,
  SUPPORTED_VERSION,
  type KdfParams,
} from './scram.js';

/** Pre-validation bounds for KDF params from the server challenge. */
const KDF_LIMITS = {
  maxMemoryKb: 262144,
  maxTime: 8,
  maxParallelism: 8,
};

/**
 * rmp_serde (the Rust msgpack library) serialises structs as **arrays**
 * (sequences) by default, not maps. The wire format for all handshake
 * messages is therefore positional:
 *
 *   Challenge = [salt, memory_kb, time, parallelism, argon2_version, server_nonce]
 *   AuthOk    = [server_signature, server_pub_key, identity_sig, session_id,
 *                expires_at_ns, resumption_ticket?, resumption_expires_at_ns?]
 */

// Positional indices for Challenge (rmp_serde array layout).
const CH_SALT = 0;
const CH_MEMORY_KB = 1;
const CH_TIME = 2;
const CH_PARALLELISM = 3;
const CH_ARGON2_VERSION = 4;
const CH_SERVER_NONCE = 5;

// Positional indices for AuthOk (rmp_serde array layout).
const OK_SERVER_SIG = 0;
const OK_SERVER_PUB = 1;
const OK_IDENTITY_SIG = 2;
const OK_SESSION_ID = 3;
const OK_EXPIRES_AT_NS = 4;

function asBytes(v: unknown, what: string): Uint8Array {
  if (v instanceof Uint8Array) return v;
  throw new Error(`${what}: expected binary, got ${typeof v}`);
}

/** Result of a successful SCRAM handshake. */
export interface HandshakeResult {
  sessionId: Uint8Array;
  serverPubKey: Uint8Array;
  expiresAtNs: bigint;
}

/**
 * Run the 4-message SCRAM-Argon2id handshake over `framer`.
 * Throws on any validation or crypto failure.
 */
export async function runHandshake(
  platform: Platform,
  framer: WsFramer,
  username: string,
  password: string,
): Promise<HandshakeResult> {
  // Server normalises the username (PRECIS UsernameCaseMapped + NFC).
  // We send the NFC form so the two byte strings agree for ASCII usernames.
  const normalizedUser = username.normalize('NFC');

  // --- msg1: auth_init ---
  const clientNonce = platform.randomBytes(32);
  framer.send(
    encode({
      user: normalizedUser,
      client_nonce: clientNonce,
      binding_mode: BINDING_MODE_TLS_NO_EXPORT,
      version: SUPPORTED_VERSION,
    }),
  );

  // --- msg2: challenge ---
  // rmp_serde encodes structs as arrays: [salt, memory_kb, time, parallelism,
  // argon2_version, server_nonce]
  const chArr = decode(await framer.recv()) as unknown[];
  const salt = asBytes(chArr[CH_SALT], 'challenge.salt');
  const serverNonce = asBytes(chArr[CH_SERVER_NONCE], 'challenge.server_nonce');
  const kdf: KdfParams = {
    memoryKb: chArr[CH_MEMORY_KB] as number,
    time: chArr[CH_TIME] as number,
    parallelism: chArr[CH_PARALLELISM] as number,
    argon2Version: chArr[CH_ARGON2_VERSION] as number,
  };
  if (kdf.memoryKb > KDF_LIMITS.maxMemoryKb) {
    throw new Error(`challenge memory_kb ${kdf.memoryKb} exceeds limit`);
  }
  if (kdf.time > KDF_LIMITS.maxTime) {
    throw new Error(`challenge time ${kdf.time} exceeds limit`);
  }
  if (kdf.parallelism > KDF_LIMITS.maxParallelism) {
    throw new Error(`challenge parallelism ${kdf.parallelism} exceeds limit`);
  }
  if (kdf.argon2Version !== ARGON2_VERSION_13) {
    throw new Error(
      `challenge argon2_version must be 0x13, got ${kdf.argon2Version}`,
    );
  }
  if (salt.length !== 16) throw new Error('challenge salt must be 16 bytes');
  if (serverNonce.length !== 32) {
    throw new Error('challenge server_nonce must be 32 bytes');
  }

  // --- msg3: client_proof ---
  // Send as array too: [client_proof_bytes]
  const authMessage = buildAuthMessage(normalizedUser, clientNonce, {
    serverNonce,
    salt,
    kdf,
  });
  const { clientProof, serverKey } = await computeClientProof(
    platform,
    password,
    salt,
    kdf,
    authMessage,
  );
  // Send as a named map — rmp_serde on the server can decode either
  // array or named-map format for struct deserialization.
  framer.send(encode({ client_proof: clientProof }));

  // --- msg4: auth_ok | error ---
  // rmp_serde AuthOk array: [server_signature, server_pub_key, identity_sig,
  //                           session_id, expires_at_ns, ...]
  const okRaw = decode(await framer.recv()) as unknown[] | { error?: string };
  if (!Array.isArray(okRaw)) {
    const errMap = okRaw as { error?: string };
    if (typeof errMap.error === 'string') {
      throw new Error(`authentication failed: ${errMap.error}`);
    }
    throw new Error(`auth_ok: unexpected non-array response`);
  }
  const serverSignature = asBytes(okRaw[OK_SERVER_SIG], 'auth_ok.server_signature');
  const sessionId = asBytes(okRaw[OK_SESSION_ID], 'auth_ok.session_id');
  const serverPubKey = asBytes(okRaw[OK_SERVER_PUB], 'auth_ok.server_pub_key');

  if (!verifyServerSignature(platform, serverKey, authMessage, serverSignature)) {
    throw new Error('server signature verification failed (MITM?)');
  }
  if (sessionId.length !== 32) {
    throw new Error('auth_ok.session_id must be 32 bytes');
  }

  const expiresAtNs = BigInt(okRaw[OK_EXPIRES_AT_NS] as number | bigint);
  return { sessionId, serverPubKey, expiresAtNs };
}
