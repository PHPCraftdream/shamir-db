import { describe, it, expect } from 'vitest';
import { SubscriptionRouter } from '../subscription-router.js';
import type { PushEnvelope } from '../types/subscribe.js';
import type { SubscriptionEvent } from '../subscription-router.js';

describe('SubscriptionRouter', () => {
  it('routes event to registered handler', () => {
    const router = new SubscriptionRouter();
    const received: SubscriptionEvent[] = [];
    router.register(1, (ev) => received.push(ev));

    const envelope: PushEnvelope = {
      push: 'event',
      sub: 1,
      seq: 0,
      data: new Uint8Array([1, 2, 3]),
    };
    const handled = router.route(envelope);

    expect(handled).toBe(true);
    expect(received).toHaveLength(1);
    expect(received[0]).toEqual({
      kind: 'event',
      seq: 0,
      data: new Uint8Array([1, 2, 3]),
      gap_at: undefined,
    });
  });

  it('buffers early pushes and flushes on register', () => {
    const router = new SubscriptionRouter();

    router.route({ push: 'event', sub: 5, seq: 0 });
    router.route({ push: 'event', sub: 5, seq: 1 });

    const received: SubscriptionEvent[] = [];
    router.register(5, (ev) => received.push(ev));

    expect(received).toHaveLength(2);
    expect(received[0].seq).toBe(0);
    expect(received[1].seq).toBe(1);
  });

  it('routes multiple subs independently', () => {
    const router = new SubscriptionRouter();
    const eventsA: SubscriptionEvent[] = [];
    const eventsB: SubscriptionEvent[] = [];
    router.register(1, (ev) => eventsA.push(ev));
    router.register(2, (ev) => eventsB.push(ev));

    router.route({ push: 'event', sub: 1, seq: 0 });
    router.route({ push: 'event', sub: 2, seq: 0 });
    router.route({ push: 'event', sub: 1, seq: 1 });

    expect(eventsA).toHaveLength(2);
    expect(eventsB).toHaveLength(1);
  });

  it('unregister stops delivery', () => {
    const router = new SubscriptionRouter();
    const received: SubscriptionEvent[] = [];
    router.register(3, (ev) => received.push(ev));

    router.route({ push: 'event', sub: 3, seq: 0 });
    router.unregister(3);
    router.route({ push: 'event', sub: 3, seq: 1 });

    expect(received).toHaveLength(1);
  });

  it('delivers closed event normally', () => {
    const router = new SubscriptionRouter();
    const received: SubscriptionEvent[] = [];
    router.register(7, (ev) => received.push(ev));

    router.route({ push: 'closed', sub: 7, seq: 4 });

    expect(received).toHaveLength(1);
    expect(received[0].kind).toBe('closed');
  });

  it('caps earlyBuffer at 256 entries per sub (drop NEW)', () => {
    const router = new SubscriptionRouter();
    // Silence the console.warn the drop path emits.
    const origWarn = console.warn;
    console.warn = () => {};
    try {
      for (let i = 0; i < 257; i++) {
        router.route({ push: 'event', sub: 99, seq: i });
      }
    } finally {
      console.warn = origWarn;
    }

    const received: SubscriptionEvent[] = [];
    router.register(99, (ev) => received.push(ev));

    // 256 retained; the 257th (seq=256) was dropped on arrival.
    expect(received).toHaveLength(256);
    expect(received[0].seq).toBe(0);
    expect(received[255].seq).toBe(255);
  });

  it('clear() wipes all state', () => {
    const router = new SubscriptionRouter();
    const received: SubscriptionEvent[] = [];
    router.register(1, (ev) => received.push(ev));
    router.route({ push: 'event', sub: 2, seq: 0 }); // buffered

    router.clear();

    // Handler gone
    router.route({ push: 'event', sub: 1, seq: 0 });
    expect(received).toHaveLength(0);

    // Buffer gone — registering now yields nothing
    const received2: SubscriptionEvent[] = [];
    router.register(2, (ev) => received2.push(ev));
    expect(received2).toHaveLength(0);
  });
});
