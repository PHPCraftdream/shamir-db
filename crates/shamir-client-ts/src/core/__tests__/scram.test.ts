/**
 * Platform-agnostic SCRAM unit tests.
 *
 * Uses NodePlatform as the injected platform (not imported inside core/).
 * Tests cover:
 *   1. buildAuthMessage byte shape (prefix, lengths, layout).
 *   2. computeClientProof SCRAM invariant: SHA256(clientKey) == storedKey.
 *   3. verifyServerSignature: accept correct / reject tampered.
 */

import { describe, it, expect } from 'vitest';
import {
  buildAuthMessage,
  computeClientProof,
  verifyServerSignature,
  ARGON2_VERSION_13,
  TRANSPORT_KIND_WS,
  BINDING_MODE_TLS_NO_EXPORT,
  SUPPORTED_VERSION,
  type KdfParams,
  type AuthMessageChallenge,
} from '../scram.js';
import { NodePlatform } from '../../platform/node.js';

// ─── helpers ──────────────────────────────────────────────────────────────────

function bytes(n: number, fill = 0): Uint8Array {
  return new Uint8Array(n).fill(fill);
}

function fromHex(hex: string): Uint8Array {
  const clean = hex.replace(/\s/g, '');
  const arr = new Uint8Array(clean.length / 2);
  for (let i = 0; i < arr.length; i++) {
    arr[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return arr;
}

const FIXED_CLIENT_NONCE = bytes(32, 0xaa);
const FIXED_SERVER_NONCE = bytes(32, 0xbb);
const FIXED_SALT = bytes(16, 0xcc);

const KDF: KdfParams = {
  memoryKb: 19456,
  time: 2,
  parallelism: 1,
  argon2Version: ARGON2_VERSION_13,
};

const CHALLENGE: AuthMessageChallenge = {
  serverNonce: FIXED_SERVER_NONCE,
  salt: FIXED_SALT,
  kdf: KDF,
};

// ─── tests ────────────────────────────────────────────────────────────────────

describe('buildAuthMessage', () => {
  it('starts with the 14-byte "SHAMIR-AUTH-v1" prefix', () => {
    const msg = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const prefix = new TextDecoder().decode(msg.subarray(0, 14));
    expect(prefix).toBe('SHAMIR-AUTH-v1');
  });

  it('encodes username length as u16-BE at byte 14', () => {
    const username = 'alice'; // 5 ASCII bytes
    const msg = buildAuthMessage(username, FIXED_CLIENT_NONCE, CHALLENGE);
    const high = msg[14];
    const low = msg[15];
    expect((high << 8) | low).toBe(5);
  });

  it('places username bytes immediately after the u16 length', () => {
    const username = 'bob';
    const msg = buildAuthMessage(username, FIXED_CLIENT_NONCE, CHALLENGE);
    const extracted = new TextDecoder().decode(msg.subarray(16, 19));
    expect(extracted).toBe('bob');
  });

  it('total message length is deterministic for a known input', () => {
    // 14 + 2 + len(user) + 32 + 32 + 16 + 4 + 4 + 4 + 1 + 1 + 1 + 32 + 1
    // = 14 + 2 + 5 + 32 + 32 + 16 + 4 + 4 + 4 + 1 + 1 + 1 + 32 + 1 = 149
    const msg = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    expect(msg.length).toBe(149);
  });

  it('encodes transport_kind, binding_mode and version at correct offsets', () => {
    const msg = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    // After prefix(14) + u16(2) + user(5) + clientNonce(32) + serverNonce(32)
    // + salt(16) + u32(4) + u32(4) + u32(4) = offset 113 → argon2_version(1)
    // → transport_kind(1) → binding_mode(1) → exporter(32) → version(1)
    const base = 14 + 2 + 5 + 32 + 32 + 16 + 4 + 4 + 4;
    expect(msg[base]).toBe(ARGON2_VERSION_13);
    expect(msg[base + 1]).toBe(TRANSPORT_KIND_WS);
    expect(msg[base + 2]).toBe(BINDING_MODE_TLS_NO_EXPORT);
    expect(msg[base + 3 + 32]).toBe(SUPPORTED_VERSION);
  });

  it('throws if clientNonce is not 32 bytes', () => {
    expect(() =>
      buildAuthMessage('alice', bytes(16, 0xaa), CHALLENGE),
    ).toThrow('client_nonce must be 32 bytes');
  });

  it('throws if serverNonce is not 32 bytes', () => {
    const bad: AuthMessageChallenge = {
      ...CHALLENGE,
      serverNonce: bytes(16, 0xbb),
    };
    expect(() =>
      buildAuthMessage('alice', FIXED_CLIENT_NONCE, bad),
    ).toThrow('server_nonce must be 32 bytes');
  });

  it('throws if salt is not 16 bytes', () => {
    const bad: AuthMessageChallenge = { ...CHALLENGE, salt: bytes(32, 0xcc) };
    expect(() =>
      buildAuthMessage('alice', FIXED_CLIENT_NONCE, bad),
    ).toThrow('salt must be 16 bytes');
  });
});

describe('computeClientProof — SCRAM invariant', () => {
  it('SHA256(clientKey) === storedKey', async () => {
    const authMessage = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const { clientKey, storedKey } = await computeClientProof(
      NodePlatform,
      'secretpassword',
      FIXED_SALT,
      KDF,
      authMessage,
    );
    const expected = NodePlatform.sha256(clientKey);
    expect(expected).toEqual(storedKey);
  }, 60_000);

  it('clientProof is 32 bytes', async () => {
    const authMessage = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const { clientProof } = await computeClientProof(
      NodePlatform,
      'pass',
      FIXED_SALT,
      KDF,
      authMessage,
    );
    expect(clientProof.length).toBe(32);
  }, 60_000);
});

describe('verifyServerSignature', () => {
  it('accepts the correct server signature', async () => {
    const authMessage = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const { serverKey } = await computeClientProof(
      NodePlatform,
      'secretpassword',
      FIXED_SALT,
      KDF,
      authMessage,
    );
    // Compute what the server would send back.
    const correctSig = NodePlatform.hmacSha256(serverKey, authMessage);
    expect(
      verifyServerSignature(NodePlatform, serverKey, authMessage, correctSig),
    ).toBe(true);
  }, 60_000);

  it('rejects a tampered server signature', async () => {
    const authMessage = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const { serverKey } = await computeClientProof(
      NodePlatform,
      'secretpassword',
      FIXED_SALT,
      KDF,
      authMessage,
    );
    const tampered = NodePlatform.hmacSha256(serverKey, authMessage);
    tampered[0] ^= 0xff; // flip a byte
    expect(
      verifyServerSignature(NodePlatform, serverKey, authMessage, tampered),
    ).toBe(false);
  }, 60_000);

  it('rejects a signature of wrong length', async () => {
    const authMessage = buildAuthMessage('alice', FIXED_CLIENT_NONCE, CHALLENGE);
    const { serverKey } = await computeClientProof(
      NodePlatform,
      'pass',
      FIXED_SALT,
      KDF,
      authMessage,
    );
    expect(
      verifyServerSignature(NodePlatform, serverKey, authMessage, bytes(16)),
    ).toBe(false);
  }, 60_000);
});
