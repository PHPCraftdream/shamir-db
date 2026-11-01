import { describe, expect, it } from 'vitest';
import { encode } from '../../core/protocol/framing.js';
import { decodeTokenBinding, encodeBrowserTokenFrame } from '../../core/protocol/token/ws.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import { TokenWsLifecycle } from '../token-lifecycle.js';
import type { TokenSocket, TokenSocketCloseEvent } from '../../platform/token/types.js';

class MemorySocket implements TokenSocket {
  sent: Uint8Array[] = []; private message: ((data: Uint8Array) => void) | undefined; private early: Uint8Array[] = []; closes: Array<(event: TokenSocketCloseEvent) => void> = [];
  async close(code = 1000): Promise<void> { for (const handler of this.closes) handler({ code, reason: '', wasClean: true }); }
  send(data: Uint8Array): void { this.sent.push(data); }
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(handler: (event: TokenSocketCloseEvent) => void): void { this.closes.push(handler); }
  receive(value: unknown): void { const frame = encode(value); if (this.message) this.message(frame); else this.early.push(frame); }
}
const sid = new Uint8Array(32).fill(7);
const connect = (socket: MemorySocket) => TokenWsLifecycle.connect(new URL('wss://localhost/'), {} as never, undefined, 0, 35_000, async () => { socket.receive({ ok: true, sid, expires_at_ns: 9n }); return socket; });
describe('token websocket codec and lifecycle', () => {
  it('validates bindings and canonical browser token framing', () => {
    const token = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
    expect(decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 12n })).expiresAtNs).toBe(12n);
    expect(() => decodeTokenBinding(encode({ ok: true, sid: new Uint8Array(31), expires_at_ns: 12n }))).toThrow();
    expect(() => decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 12n, extra: true }))).toThrow();
    expect(encodeBrowserTokenFrame(token).byteLength).toBeLessThanOrEqual(256);
    expect(() => encodeBrowserTokenFrame('tkn_' + 'a'.repeat(67))).toThrow();
  });
  it('binds an early frame, returns sid copies and correlates responses', async () => {
    const socket = new MemorySocket(); const lifecycle = await connect(socket); const copy = lifecycle.sid; copy.fill(0); expect(lifecycle.sid).toEqual(sid);
    const a = lifecycle.request({ op: 'ping' }); const b = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 99, res: encode({ kind: 'pong' }) }); socket.receive({ rid: 2, res: encode({ kind: 'pong', n: 2 }) }); socket.receive({ rid: 1, res: encode({ kind: 'pong', n: 1 }) });
    await expect(b).resolves.toMatchObject({ n: 2 }); await expect(a).resolves.toMatchObject({ n: 1 }); await lifecycle.close();
  });
  it('cleans aborted requests and leaves zero timeout unbounded', async () => {
    const socket = new MemorySocket(); const lifecycle = await connect(socket); const abort = new AbortController();
    const pending = lifecycle.request({ op: 'ping' }, { signal: abort.signal }); abort.abort(); await expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    const noTimeout = lifecycle.request({ op: 'ping' }, { timeoutMs: 0 }); socket.receive({ rid: 2, res: encode({ kind: 'pong' }) }); await noTimeout; await lifecycle.close();
  });
  it('preserves application errors and terminates malformed replies', async () => {
    const socket = new MemorySocket(); const lifecycle = await connect(socket); const limited = lifecycle.request({ op: 'ping' }); socket.receive({ rid: 1, error: 'rate_limited' });
    await expect(limited).rejects.toMatchObject({ code: 'rate_limited' }); expect(lifecycle.sid).toEqual(sid);
    const bad = lifecycle.request({ op: 'ping' }); socket.receive({ rid: 2, res: encode([]) }); await expect(bad).rejects.toBeInstanceOf(TokenTransportError); expect(lifecycle.sid).toHaveLength(0);
  });
});
