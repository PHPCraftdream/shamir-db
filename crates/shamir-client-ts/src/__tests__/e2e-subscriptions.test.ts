/**
 * End-to-end tests — live subscriptions against a live shamir-server.
 *
 * Split out of `e2e.test.ts` so the subscriptions topic owns its own file
 * (clearer git-blame, smaller donor file). Vitest runs test files in
 * parallel workers, so this file uses its OWN port to avoid EADDRINUSE
 * against the sibling e2e file.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse, WireValue } from '../index.js';
import {
  Query,
  Batch,
  filter,
  write,
  ddl,
} from '../index.js';
import {
  SERVER_BIN,
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
  setupDb,
  seed,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// ─── live subscriptions (A5) ─────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'live subscriptions (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[live-subs] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    /** Race a promise against a timeout — resolves with `null` if it doesn't settle in time. */
    async function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
      let to: ReturnType<typeof setTimeout> | null = null;
      const timer = new Promise<null>((resolve) => {
        to = setTimeout(() => resolve(null), ms);
      });
      const out = await Promise.race([p, timer]);
      if (to) clearTimeout(to);
      return out;
    }

    // 1. records delivery — INSERT must yield an event.
    it('records: insert matching the filter delivers an event', async () => {
      const dbName = await setupDb(client!, 'sub_records', ['m']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-1').subscribe('m', {
          store: 'main',
          table: 'm',
          where: (f) => f.eq('x', 1),
        }),
      );
      expect(subs.m).toBeDefined();
      expect(typeof subs.m.subId).toBe('number');

      await db.batch('ins').add('i', write.insert('m', [{ id: 'k1', x: 1 }])).run();

      const ev = await withTimeout(subs.m.next(), 1500);
      expect(ev).not.toBeNull();
      expect(ev!.done).toBe(false);
      expect(ev!.value.kind).toBe('event');

      await subs.m.unsubscribe().catch(() => {});
    });

    // 2. filter proves wire savings — unmatched rows must not arrive.
    it('filter: unmatched rows are dropped server-side (no wire frame)', async () => {
      const dbName = await setupDb(client!, 'sub_filter', ['messages']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-f').subscribe('messages', {
          store: 'main',
          table: 'messages',
          where: (f) => f.eq('thread_id', 42),
        }),
      );

      // Hold ONE next() across the whole assertion. Each next() is its own
      // waiter; if we timed it out and then called next() again, the brought-
      // in event would be delivered to the abandoned waiter — not the new one.
      const pending = subs.messages.next();

      // Insert a NON-matching row. The server-side filter drops the event
      // before any push frame is emitted — the wire stays empty.
      await db.batch('noise')
        .add('i', write.insert('messages', [{ id: 'n1', thread_id: 7, body: 'noise' }]))
        .run();

      const none = await Promise.race([
        pending.then((v) => ({ resolved: true as const, v })),
        new Promise<{ resolved: false }>((r) => setTimeout(() => r({ resolved: false }), 300)),
      ]);
      expect(none.resolved).toBe(false); // no frame within 300 ms

      // Now insert a matching row — it must arrive on the SAME pending next().
      await db.batch('real')
        .add('i', write.insert('messages', [{ id: 'r1', thread_id: 42, body: 'real' }]))
        .run();

      const ev = await withTimeout(pending, 1500);
      expect(ev).not.toBeNull();
      expect(ev!.value.kind).toBe('event');

      await subs.messages.unsubscribe().catch(() => {});
    });

    // 3. two subscriptions in one batch — each on its own stream.
    it('multi: two subscribe ops in one batch yield two independent streams', async () => {
      const dbName = await setupDb(client!, 'sub_multi', ['a', 'b']);
      const db = client!.db(dbName);

      const { response, subs } = await db.runLive(
        db.batch('multi-sub')
          .subscribe('sa', { store: 'main', table: 'a', where: (f) => f.eq('k', 1) })
          .subscribe('sb', { store: 'main', table: 'b', where: (f) => f.eq('k', 2) }),
      );
      expect(subs.sa).toBeDefined();
      expect(subs.sb).toBeDefined();
      expect(subs.sa.subId).not.toBe(subs.sb.subId);
      // Both grants carry a numeric sub id.
      expect(typeof (response.results.sa.value as { sub?: unknown }).sub).toBe('number');
      expect(typeof (response.results.sb.value as { sub?: unknown }).sub).toBe('number');

      await db.batch('ins-a').add('i', write.insert('a', [{ id: 'aa', k: 1 }])).run();
      await db.batch('ins-b').add('i', write.insert('b', [{ id: 'bb', k: 2 }])).run();

      const eva = await withTimeout(subs.sa.next(), 1500);
      const evb = await withTimeout(subs.sb.next(), 1500);
      expect(eva).not.toBeNull();
      expect(evb).not.toBeNull();
      expect(eva!.value.kind).toBe('event');
      expect(evb!.value.kind).toBe('event');

      await subs.sa.unsubscribe().catch(() => {});
      await subs.sb.unsubscribe().catch(() => {});
    });

    // 4. handle: reactive sub-batch delivers an event with the batch's result.
    it('handle: reactive sub-batch delivers a frame on insert', async () => {
      const dbName = await setupDb(client!, 'sub_handle', ['threads', 'msgs']);
      const db = client!.db(dbName);
      await seed(client!, dbName, 'threads', [{ id: 't1', topic: 'hello' }]);

      const { subs } = await db.runLive(
        db.batch('sub-h').subscribe('h', {
          store: 'main',
          table: 'msgs',
          // Reactive sub-batch: read all threads on every event. We don't
          // reference $event here — the test only verifies that the reactive
          // delivery path produces a non-empty frame on insert.
          handle: (b) => b.add('t', Query.from('threads')),
        }),
      );

      await db.batch('ins').add('i', write.insert('msgs', [{ id: 'm1', body: 'hi' }])).run();

      const ev = await withTimeout(subs.h.next(), 1500);
      expect(ev).not.toBeNull();
      expect(ev!.value.kind).toBe('event');

      await subs.h.unsubscribe().catch(() => {});
    });

    // 5. initial snapshot — pre-seeded rows arrive before `ready`.
    it('initial: pre-seeded rows arrive before kind:"ready"', async () => {
      const dbName = await setupDb(client!, 'sub_initial', ['items']);
      const db = client!.db(dbName);
      const N = 3;
      const seedRecs: Array<Record<string, WireValue>> = [];
      for (let i = 0; i < N; i += 1) seedRecs.push({ id: `s${i}`, n: i });
      await seed(client!, dbName, 'items', seedRecs);

      const { subs } = await db.runLive(
        db.batch('sub-i').subscribe(
          'i',
          { store: 'main', table: 'items' },
          { initial: true },
        ),
      );

      // Collect events until 'ready'.
      let preReady = 0;
      let sawReady = false;
      for (let i = 0; i < N + 5 && !sawReady; i += 1) {
        const ev = await withTimeout(subs.i.next(), 2000);
        if (!ev || ev.done) break;
        if (ev.value.kind === 'ready') { sawReady = true; break; }
        if (ev.value.kind === 'event') preReady += 1;
      }
      expect(sawReady).toBe(true);
      expect(preReady).toBeGreaterThanOrEqual(N);

      // Live events still flow after ready.
      await db.batch('ins').add('i', write.insert('items', [{ id: 'live', n: 99 }])).run();
      const live = await withTimeout(subs.i.next(), 1500);
      expect(live).not.toBeNull();
      expect(live!.value.kind).toBe('event');

      await subs.i.unsubscribe().catch(() => {});
    });

    // 6. unsubscribe stops the stream.
    it('unsubscribe: stream goes done; further inserts do not yield events', async () => {
      const dbName = await setupDb(client!, 'sub_unsub', ['x']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-u').subscribe('x', {
          store: 'main',
          table: 'x',
          where: (f) => f.eq('k', 1),
        }),
      );

      await db.batch('a').add('i', write.insert('x', [{ id: 'x1', k: 1 }])).run();
      const first = await withTimeout(subs.x.next(), 1500);
      expect(first).not.toBeNull();
      expect(first!.value.kind).toBe('event');

      await subs.x.unsubscribe();

      await db.batch('b').add('i', write.insert('x', [{ id: 'x2', k: 1 }])).run();
      const after = await withTimeout(subs.x.next(), 400);
      // Either timed out (null), or the iterator reports done.
      if (after !== null) {
        expect(after.done).toBe(true);
      }
    });

    // 7. grant refusal — multi-repo subscription is rejected.
    it('refusal: multi-repo subscription is rejected by the server', async () => {
      const dbName = uniqueDbName('sub_refuse');
      await client!.execute('default', {
        id: `setup-${dbName}-db`,
        queries: { mk: ddl.createDb(dbName) },
      });
      // Create two repos with one table each.
      await client!.execute(dbName, {
        id: `setup-${dbName}-repos`,
        queries: {
          r1: ddl.createRepo('main'),
          r2: ddl.createRepo('other'),
          t1: ddl.createTable('a', { repo: 'main' }),
          t2: ddl.createTable('b', { repo: 'other' }),
        },
      });
      const db = client!.db(dbName);

      // Subscribe to two sources spanning two repos — must throw with the
      // documented multi_repo_subscriptions_not_supported code.
      await expect(
        db.runLive(
          db.batch('refuse').subscribe('s', [
            { store: 'main', table: 'a' },
            { store: 'other', table: 'b' },
          ]),
        ),
      ).rejects.toThrow(/multi_repo_subscriptions_not_supported|multi-repo/i);
    });
  },
);

describe('e2e-subscriptions.test skip reason', () => {
  it('reports why the e2e-subscriptions test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        `[e2e-subscriptions.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
