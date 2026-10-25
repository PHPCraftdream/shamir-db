/**
 * Unit tests for the mandatory `shamir-v1` WebSocket subprotocol
 * (RV-TRANSPORT-WS-C0C43A18) in the platform adapters: `browser.ts` must
 * pass `['shamir-v1']` to the global `WebSocket` constructor and
 * `node.ts` must set `protocols: ['shamir-v1']` in the `ws` options.
 *
 * No real connection is attempted: the fake WebSocket / `ws` mock capture
 * the constructor arguments and `openSocket`'s promise is simply left
 * pending.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

describe('ws subprotocol negotiation (rv_transport_ws_c0c43a18)', () => {
  it('browser openSocket offers shamir-v1 to the WebSocket constructor', async () => {
    const constructed: Array<[string, unknown]> = [];

    class FakeWebSocket {
      binaryType = 'blob';
      constructor(url: string, protocols?: string[]) {
        constructed.push([url, protocols]);
      }
      addEventListener(): void {}
      removeEventListener(): void {}
    }
    vi.stubGlobal('WebSocket', FakeWebSocket);

    const { BrowserPlatform } = await import('../browser.js');
    void BrowserPlatform.openSocket('wss://x/shamir/v1', {});

    expect(constructed).toHaveLength(1);
    const [url, protocols] = constructed[0]!;
    expect(url).toBe('wss://x/shamir/v1');
    expect(protocols).toEqual(['shamir-v1']);
  });

  it('node openSocket passes [shamir-v1] as the ws protocols argument', async () => {
    const fakeWs = {
      binaryType: '',
      on: () => fakeWs,
      once: () => fakeWs,
      off: () => fakeWs,
    };
    const wsCtor = vi.fn(() => fakeWs);
    vi.doMock('ws', () => ({ default: wsCtor }));

    const { NodePlatform } = await import('../node.js');
    void NodePlatform.openSocket('wss://x/shamir/v1', {});

    expect(wsCtor).toHaveBeenCalledTimes(1);
    const [url, protocols] = wsCtor.mock.calls[0] as [string, string[], unknown];
    expect(url).toBe('wss://x/shamir/v1');
    expect(protocols).toEqual(['shamir-v1']);
  });
});
