import { describe, expect, it, vi } from 'vitest';
import { encode } from '../../../core/protocol/framing.js';
import { TokenWsLifecycle } from '../token-lifecycle.js';
import type { TokenSocket, TokenSocketCloseEvent } from '../../../platform/token/types.js';

class TimeoutSocket implements TokenSocket {
  sent: Uint8Array[] = [];
  private message: ((data: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  async close(): Promise<void> {}
  send(data: Uint8Array): void { this.sent.push(data); }
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const data of this.early.splice(0)) handler(data); }
  onClose(_handler: (event: TokenSocketCloseEvent) => void): void {}
  receive(value: unknown): void { const data = encode(value); if (this.message) this.message(data); else this.early.push(data); }
}

describe('token request timeout', () => {
  it('times out a request and allows a later request to complete', async () => {
    vi.useFakeTimers();
    const socket = new TimeoutSocket();
    try {
      const lifecycle = await TokenWsLifecycle.connect(new URL('wss://example.test/'), {} as never, undefined, 0, 25, async () => {
        socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n });
        return socket;
      });
      const timedOut = lifecycle.request({ op: 'ping' });
      const rejection = expect(timedOut).rejects.toMatchObject({ code: 'client_timeout' });
      await vi.advanceTimersByTimeAsync(25);
      await rejection;
      socket.receive({ rid: 1, res: encode({ kind: 'pong' }) });
      const later = lifecycle.request({ op: 'ping' });
      socket.receive({ rid: 2, res: encode({ kind: 'pong' }) });
      await expect(later).resolves.toMatchObject({ kind: 'pong' });
      await lifecycle.close();
    } finally {
      vi.useRealTimers();
    }
  });
});
