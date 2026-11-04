/**
 * Unit tests for changePassword() channel-binding handling. Fakes only: no
 * network, no real sockets, cheap deterministic argon2id.
 */

import { describe, it, expect } from 'vitest';
import { createHash, createHmac } from 'node:crypto';
import type { Platform } from '../../../support/platform.js';
import { changePassword } from '../../../client/client-internals.js';
import {
  TRANSPORT_KIND_WS,
  TRANSPORT_KIND_UNIX,
  BINDING_MODE_TLS_NO_EXPORT,
  BINDING_MODE_NONE,
  buildAuthMessageCp,
  computeClientProof,
} from '../../scram.js';
import type { KdfParams } from '../../scram.js';
import { browserWsBinding, ipcBinding } from '../binding.js';
import type { ChannelBinding } from '../binding.js';

const OLD_PW = 'old-password';
const NEW_PW = 'new-password';
const USER = 'alice';
const SESSION_ID = new Uint8Array(32).fill(7);
const CLIENT_NONCE = new Uint8Array(32).fill(0x11);
const SERVER_NONCE = new Uint8Array(32).fill(0x22);
const SALT = new Uint8Array(16).fill(0x33);
const KDF: KdfParams = {
  memoryKb: 19456,
  time: 2,
  parallelism: 1,
  argon2Version: 19,
};

function makePlatform(): Platform {
  const fake = {
    randomBytes: (n: number) => new Uint8Array(n).fill(0x11),
    hmacSha256: (key: Uint8Array, data: Uint8Array) =>
      new Uint8Array(createHmac('sha256', key).update(data).digest()),
    sha256: (data: Uint8Array) =>
      new Uint8Array(createHash('sha256').update(data).digest()),
    argon2id: async (password: Uint8Array, salt: Uint8Array) =>
      new Uint8Array(
        createHash('sha256').update(password).update(salt).digest(),
      ),
    timingSafeEqual: (a: Uint8Array, b: Uint8Array) =>
      a.length === b.length && a.every((v, i) => v === b[i]),
  };
  return fake as unknown as Platform;
}

function makeSend() {
  const requests: Record<string, unknown>[] = [];
  const send = async (req: object): Promise<Record<string, unknown>> => {
    requests.push(req as Record<string, unknown>);
    if (requests.length === 1) {
      return {
        kind: 'change_password_challenge',
        server_nonce_cp: SERVER_NONCE,
        salt: SALT,
        kdf_memory_kb: KDF.memoryKb,
        kdf_time: KDF.time,
        kdf_parallelism: KDF.parallelism,
        kdf_argon2_version: KDF.argon2Version,
      };
    }
    return { kind: 'change_password_ok' };
  };
  return { requests, send };
}

function authMessage(transportKind: number, bindingMode: number): Uint8Array {
  return buildAuthMessageCp({
    username: USER,
    sessionId: SESSION_ID,
    clientNonceCp: CLIENT_NONCE,
    serverNonceCp: SERVER_NONCE,
    salt: SALT,
    kdf: KDF,
    transportKind,
    bindingMode,
    channelBindingAtAuth: new Uint8Array(32),
  });
}

async function runAndCaptureProof(
  binding: ChannelBinding | undefined,
): Promise<Uint8Array> {
  const platform = makePlatform();
  const { requests, send } = makeSend();
  await changePassword(OLD_PW, NEW_PW, platform, SESSION_ID, USER, binding, send);
  expect(requests).toHaveLength(2);
  expect(requests[0].op).toBe('change_password_challenge');
  expect(requests[1].op).toBe('change_password_verify');
  return requests[1].client_proof_old as Uint8Array;
}

async function expectedProof(
  transportKind: number,
  bindingMode: number,
): Promise<Uint8Array> {
  const r = await computeClientProof(
    makePlatform(),
    OLD_PW,
    SALT,
    KDF,
    authMessage(transportKind, bindingMode),
  );
  return r.clientProof;
}

describe('changePassword channel binding', () => {
  it('(a) browserWsBinding matches the legacy WS/TLS-no-export wire', async () => {
    const got = await runAndCaptureProof(browserWsBinding());
    const want = await expectedProof(
      TRANSPORT_KIND_WS,
      BINDING_MODE_TLS_NO_EXPORT,
    );
    expect(Array.from(got)).toEqual(Array.from(want));
  });

  it('(b) ipcBinding uses TRANSPORT_KIND_UNIX / BINDING_MODE_NONE', async () => {
    const got = await runAndCaptureProof(ipcBinding());
    const want = await expectedProof(TRANSPORT_KIND_UNIX, BINDING_MODE_NONE);
    expect(Array.from(got)).toEqual(Array.from(want));
  });

  it('(c) the binding is really used: proofs and auth messages differ', async () => {
    const ws = await runAndCaptureProof(browserWsBinding());
    const ipc = await runAndCaptureProof(ipcBinding());
    expect(Array.from(ws)).not.toEqual(Array.from(ipc));

    const wsMsg = authMessage(TRANSPORT_KIND_WS, BINDING_MODE_TLS_NO_EXPORT);
    const ipcMsg = authMessage(TRANSPORT_KIND_UNIX, BINDING_MODE_NONE);
    expect(Array.from(wsMsg)).not.toEqual(Array.from(ipcMsg));
  });

  it('(d) undefined binding rejects and sends nothing', async () => {
    const { requests, send } = makeSend();
    await expect(
      changePassword(
        OLD_PW,
        NEW_PW,
        makePlatform(),
        SESSION_ID,
        USER,
        undefined,
        send,
      ),
    ).rejects.toThrow('requires a full connect() session');
    expect(requests).toHaveLength(0);
  });

  it('(d) undefined normalizedUser rejects and sends nothing', async () => {
    const { requests, send } = makeSend();
    await expect(
      changePassword(
        OLD_PW,
        NEW_PW,
        makePlatform(),
        SESSION_ID,
        undefined,
        browserWsBinding(),
        send,
      ),
    ).rejects.toThrow('requires a full connect() session');
    expect(requests).toHaveLength(0);
  });
});
