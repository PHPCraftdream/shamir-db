import type { Platform, Socket } from '../../support/platform.js';
import { ShamirClient } from '../../client/client.js';
import { encode, decode } from '../../protocol/framing.js';

export async function fixture(requestTimeoutMs = 0): Promise<{
  client: ShamirClient;
  requests: Record<string, unknown>[];
  nextRequest: () => Promise<Record<string, unknown>>;
  respond: (response: Record<string, unknown>) => void;
}> {
  let message: (bytes: Uint8Array) => void = () => {};
  let close: (err?: Error) => void = () => {};
  let rid = 0;
  const queue: Record<string, unknown>[] = [];
  const waiters: Array<(request: Record<string, unknown>) => void> = [];
  const requests: Record<string, unknown>[] = [];
  const frame = (body: Uint8Array): Uint8Array => {
    const bytes = new Uint8Array(body.length + 4);
    new DataView(bytes.buffer).setUint32(0, body.length);
    bytes.set(body, 4);
    return bytes;
  };
  const socket: Socket = {
    send: (bytes) => {
      const envelope = decode(bytes.subarray(4)) as { rid?: number; req?: Uint8Array };
      if (envelope.req !== undefined) {
        rid = envelope.rid ?? 0;
        const request = decode(envelope.req) as Record<string, unknown>;
        requests.push(request);
        const waiter = waiters.shift();
        if (waiter !== undefined) waiter(request); else queue.push(request);
      }
    },
    onMessage: (handler) => {
      message = handler;
      message(frame(encode([new Uint8Array(32), 1n, new Uint8Array(), 0n, 3])));
    },
    onClose: (handler) => { close = handler; },
    close: async () => { close(); },
  };
  const platform: Platform = {
    hmacSha256: () => new Uint8Array(32), sha256: () => new Uint8Array(32),
    randomBytes: (n) => new Uint8Array(n), timingSafeEqual: () => true,
    argon2id: async () => new Uint8Array(32), openSocket: async () => socket,
  };
  const client = await ShamirClient.resume(platform, {
    host: 'localhost', port: 9999, ticket: new Uint8Array([1]),
    serverPubKey: new Uint8Array(32), requestTimeoutMs,
  });
  return { client, requests, nextRequest: () => {
    const request = queue.shift();
    return request !== undefined ? Promise.resolve(request)
      : new Promise((resolve) => { waiters.push(resolve); });
  }, respond: (response) => {
    message(frame(encode({ rid, res: encode(response) })));
  } };
}
