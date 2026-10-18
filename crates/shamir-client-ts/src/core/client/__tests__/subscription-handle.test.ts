import { describe, it, expect } from 'vitest';
import { SubscriptionRouter } from '../subscription-router.js';
import { SubscriptionHandle } from '../subscription-handle.js';

describe('SubscriptionHandle', () => {
  it('yields events via async iteration', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(1, router);

    router.route({ push: 'event', sub: 1, seq: 1, data: new Uint8Array([1]) });
    router.route({ push: 'event', sub: 1, seq: 2, data: new Uint8Array([2]) });

    const ev1 = await handle.next();
    expect(ev1.done).toBe(false);
    expect(ev1.value.kind).toBe('event');
    expect(ev1.value.seq).toBe(1);

    const ev2 = await handle.next();
    expect(ev2.done).toBe(false);
    expect(ev2.value.seq).toBe(2);
  });

  it('resolves waiting next() when event arrives later', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(2, router);

    const promise = handle.next();
    router.route({ push: 'event', sub: 2, seq: 1 });

    const result = await promise;
    expect(result.done).toBe(false);
    expect(result.value.seq).toBe(1);
  });

  it('closes on server Closed push', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(3, router);

    router.route({ push: 'event', sub: 3, seq: 1 });
    router.route({ push: 'closed', sub: 3, seq: 2 });

    const ev1 = await handle.next();
    expect(ev1.done).toBe(false);

    const ev2 = await handle.next();
    expect(ev2.done).toBe(true);
  });

  it('for-await-of works', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(4, router);

    router.route({ push: 'event', sub: 4, seq: 1 });
    router.route({ push: 'event', sub: 4, seq: 2 });
    router.route({ push: 'closed', sub: 4, seq: 3 });

    const collected = [];
    for await (const ev of handle) {
      collected.push(ev.seq);
    }
    expect(collected).toEqual([1, 2]);
  });

  it('on() callback style', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(5, router);

    const events: number[] = [];
    handle.on((ev) => events.push(ev.seq));

    router.route({ push: 'event', sub: 5, seq: 10 });
    router.route({ push: 'event', sub: 5, seq: 20 });
    router.route({ push: 'closed', sub: 5, seq: 30 });

    await new Promise((r) => setTimeout(r, 10));
    expect(events).toEqual([10, 20]);
  });

  it('return() closes immediately', async () => {
    const router = new SubscriptionRouter();
    const handle = new SubscriptionHandle(6, router);

    await handle.return();
    const result = await handle.next();
    expect(result.done).toBe(true);
  });
});
