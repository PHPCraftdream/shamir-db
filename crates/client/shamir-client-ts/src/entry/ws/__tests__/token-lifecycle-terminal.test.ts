import { describe, expect, it, vi } from 'vitest';
import { TokenWsLifecycle } from '../token-lifecycle.js';
import type { TokenSocket, TokenSocketCloseEvent } from '../../../platform/token/types.js';
import { encode } from '../../../core/protocol/framing.js';

class FakeSocket implements TokenSocket {
  private message: ((data: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  private readonly closes: Array<(event: TokenSocketCloseEvent) => void> = [];
  async close(code = 1000): Promise<void> { for (const handler of this.closes) handler({ code, reason: '', wasClean: true }); }
  send(): void {}
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(handler: (event: TokenSocketCloseEvent) => void): void { this.closes.push(handler); }
  receive(frame: unknown): void { const bytes = encode(frame); if (this.message) this.message(bytes); else this.early.push(bytes); }
}

describe('TokenWsLifecycle terminal subscribers', () => {
  const connect = async (socket: FakeSocket) => TokenWsLifecycle.connect(new URL('wss://localhost/'), {} as never, undefined, 0, 10, async () => { queueMicrotask(() => socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n })); return socket; });
  it.each([[4000,'authentication_failed'],[4010,'token_sid_mismatch'],[4011,'token_revoked'],[4012,'token_expired'],[4013,'token_channel_denied'],[4014,'token_plaintext_write_denied'],[1013,'server_busy'],[1000,'connection_closed'],[1003,'protocol_error'],[1009,'protocol_error'],[1006,'network_error']] as const)('maps terminal close %i', async (code, expected) => {
    const socket = new FakeSocket(); const lifecycle = await connect(socket); const terminal = vi.fn(); lifecycle.onTerminal(terminal);
    socket.close(code); expect(terminal).toHaveBeenCalledWith(expect.objectContaining({ code: expected }));
  });
  it('notifies local resources once on terminal close', async () => {
    const socket = new FakeSocket();
    const lifecycle = await connect(socket);
    const terminal = vi.fn(); lifecycle.onTerminal(terminal);
    socket.receive({ rid: 1, error: 'rate_limited' }); await lifecycle.close();
    expect(terminal).toHaveBeenCalledTimes(1);
  });

});
