import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import { decode as decodeWire, encode as encodeWire } from '@msgpack/msgpack';
import * as NodePlatform from '../../platform/node.js';
import { loginNode } from '../node/node.js';
type Fields = Record<string, unknown>;
function encodeMessage(value: unknown): Uint8Array { return encodeWire(value, { useBigInt64: true }); }
function decodeRecord(buffer: Uint8Array): Fields {
  const value: unknown = decodeWire(buffer, { useBigInt64: true });
  if (!value || typeof value !== 'object' || Array.isArray(value) || value instanceof Uint8Array) throw new Error('expected MsgPack record');
  return value as Fields;
}

const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);
const body = (value: unknown) => encodeMessage(value);
const readBody = async (req: IncomingMessage): Promise<Uint8Array> => {
  const chunks: Uint8Array[] = [];
  for await (const chunk of req) chunks.push(new Uint8Array(chunk));
  return Buffer.concat(chunks);
};

async function listen(handler: (req: IncomingMessage, res: ServerResponse) => void) {
  const server = createServer(handler);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('missing loopback address');
  return { server, url: `http://127.0.0.1:${address.port}/` };
}

function reply(res: ServerResponse, status: number, value: unknown, contentType = 'application/msgpack') {
  res.writeHead(status, { 'Content-Type': contentType, Connection: 'close' });
  res.end(body(value));
}

function close(server: ReturnType<typeof createServer>): Promise<void> {
  server.closeAllConnections();
  const closed = once(server, 'close');
  server.close();
  return closed.then(() => undefined);
}

describe('loginNode HTTP contract', () => {
  it('sends canonical begin fields and reports the public authentication failure', async () => {
    let begin: Record<string, unknown> | undefined;
    const fixture = await listen((req, res) => {
      void readBody(req).then(raw => {
        const message = decodeRecord(raw);
        if (req.url?.endsWith('/begin')) {
          begin = message;
          reply(res, 401, { error: 'authentication_failed' });
        } else reply(res, 401, { error: 'authentication_failed' });
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'ALICE', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'authentication_failed' });
      expect(begin).toMatchObject({ protocol_version: 1, user: 'alice', binding_mode: 3,
        token: { label: 'http-login', mode: 'read_only', channels: 'rest', databases: null } });
      expect(begin?.client_nonce).toBeInstanceOf(Uint8Array);
    } finally { await close(fixture.server); }
  });

  it('rejects an unknown challenge field before invoking finish', async () => {
    let finishes = 0;
    const argon = vi.spyOn(NodePlatform.NodePlatform, 'argon2id');
    const fixture = await listen((req, res) => {
      void readBody(req).then(() => {
        if (req.url?.endsWith('/begin')) reply(res, 200, {
          protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1),
          handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456,
          time: 2, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4), extra: true,
        });
        else { finishes++; reply(res, 500, {}); }
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'alice', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'protocol_error' });
      expect(finishes).toBe(0);
      expect(argon).not.toHaveBeenCalled();
    } finally { argon.mockRestore(); await close(fixture.server); }
  });

  it('rejects signed or out-of-range challenge integers before Argon2', async () => {
    let finishes = 0;
    const argon = vi.spyOn(NodePlatform.NodePlatform, 'argon2id');
    const fixture = await listen((req, res) => {
      void readBody(req).then(() => {
        if (req.url?.endsWith('/begin')) reply(res, 200, {
          protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1),
          handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456,
          time: -1, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4),
        });
        else { finishes++; reply(res, 500, {}); }
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'alice', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'protocol_error' });
      expect(finishes).toBe(0);
      expect(argon).not.toHaveBeenCalled();
    } finally { argon.mockRestore(); await close(fixture.server); }
  });
});
