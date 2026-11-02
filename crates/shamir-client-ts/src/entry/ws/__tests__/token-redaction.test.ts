import { describe, expect, it } from 'vitest';
import { inspect } from 'node:util';
import { ApiToken } from '../../api-token.js';
import { encode } from '../../../core/protocol/framing.js';
import { TokenWsLifecycle } from '../token-lifecycle.js';
import type { TokenSocket, TokenSocketCloseEvent } from '../../../platform/token/types.js';

const secret = `shk1.${'A'.repeat(22)}.${'B'.repeat(42)}A`;
const token = ApiToken.from(secret);
function expectRedacted(value: unknown): void {
  const renderings = [String(value), JSON.stringify(value), inspect(value), value instanceof Error ? value.stack ?? '' : ''];
  for (const rendered of renderings) expect(rendered).not.toContain(secret);
}
class RedactionSocket implements TokenSocket {
  private message: ((data: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  private readonly closes: Array<(event: TokenSocketCloseEvent) => void> = [];
  async close(code = 1000): Promise<void> { for (const handler of this.closes) handler({ code, reason: secret, wasClean: true }); }
  send(_data: Uint8Array): void {}
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(handler: (event: TokenSocketCloseEvent) => void): void { this.closes.push(handler); }
  receive(value: unknown): void { const frame = encode(value); if (this.message) this.message(frame); else this.early.push(frame); }
}

describe('token error redaction', () => {
  it('redacts token handles, factory failures, wrapped server messages and peer close reasons', async () => {
    expect(String(token)).not.toContain(secret);
    expect(JSON.stringify({ token })).not.toContain(secret);
    expect(inspect(token)).not.toContain(secret);
    expect(`${token}`).not.toContain(secret);
    expect(token.toLocaleString()).not.toContain(secret);

    const factoryFailure = new Error(secret);
    let mappedFactoryFailure: unknown;
    try { await TokenWsLifecycle.connect(new URL('wss://example.test/'), token, undefined, 0, 1000, async () => { throw factoryFailure; }); } catch (error) { mappedFactoryFailure = error; }
    expect(mappedFactoryFailure).toMatchObject({ code: 'network_error' });
    expectRedacted(mappedFactoryFailure);

    const socket = new RedactionSocket();
    const lifecycle = await TokenWsLifecycle.connect(new URL('wss://example.test/'), token, undefined, 0, 1000, async () => {
      socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n });
      return socket;
    });
    const wrapped = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 1, res: encode({ kind: 'error', code: 'validation', message: secret }) });
    let mapped: unknown;
    try { await wrapped; } catch (error) { mapped = error; }
    expect(mapped).toBeInstanceOf(Error);
    expectRedacted(mapped);

    const terminalSocket = new RedactionSocket();
    const terminalLifecycle = await TokenWsLifecycle.connect(new URL('wss://example.test/'), token, undefined, 0, 1000, async () => {
      terminalSocket.receive({ ok: true, sid: new Uint8Array(32).fill(2), expires_at_ns: 1n });
      return terminalSocket;
    });
    const pending = terminalLifecycle.request({ op: 'ping' });
    await terminalSocket.close(1000);
    let terminalError: unknown;
    try { await pending; } catch (error) { terminalError = error; }
    expect(terminalError).toMatchObject({ code: 'connection_closed' });
    expectRedacted(terminalError);
    await terminalLifecycle.close();
    await lifecycle.close();
    expectRedacted(mappedFactoryFailure);
  });
});
