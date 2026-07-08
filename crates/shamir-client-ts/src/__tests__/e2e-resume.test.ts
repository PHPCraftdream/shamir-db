/**
 * End-to-end test for `ShamirClient.resume()` — the session-resume fast-path.
 *
 * Connects to a live shamir-server (full SCRAM handshake), obtains a
 * resumption ticket, then exercises `resume()` against the SAME server to
 * verify the real wire round-trip works end-to-end. This is the test that
 * would have caught CRIT-9 (the resume_ok response is a POSITIONAL msgpack
 * ARRAY, not a named map): any future wire-shape drift on resume_ok will
 * fail here against a real server, not just a mock.
 *
 * Skipped automatically when no release server binary is present
 * (SERVER_AVAILABLE from the harness).
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { resume } from '../index.js';
import {
  SERVER_BIN,
  SERVER_AVAILABLE,
  HOST,
  ORIGIN,
  startServer,
  connectAdmin,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e resume() (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let primary: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      primary = await connectAdmin(HOST, server.port);
    }, 60_000);

    afterAll(async () => {
      if (primary) {
        try { await primary.close(); } catch { /* ok */ }
        primary = null;
      }
      if (server) {
        try { await server.stop(); } catch { /* ok */ }
        server = null;
      }
    });

    it('resumes a session via a real ticket against the live server', async () => {
      // Sanity: the full-handshake connection must have yielded a ticket.
      const ticket = primary!.resumptionTicket();
      expect(ticket).toBeInstanceOf(Uint8Array);
      expect(ticket!.length).toBeGreaterThan(0);

      // Resume using the ticket + the pinned server public key. This exercises
      // the resume_ok positional-array decode against the real server binary.
      const resumed = await resume({
        host: HOST,
        port: server!.port,
        ticket: ticket!,
        serverPubKey: primary!.serverPubKeyPin(),
        tls: { rejectUnauthorized: false },
        origin: ORIGIN,
      });

      try {
        // The resumed session must be usable: a ping round-trips through the
        // real request loop, proving the session_id decoded from resume_ok is
        // valid and accepted by the server.
        const pong = await resumed.ping();
        expect(pong).toBeDefined();

        // The resumed session_id is a fresh 32-byte id issued by the server.
        expect(resumed.sessionId()).toBeInstanceOf(Uint8Array);
        expect(resumed.sessionId().length).toBe(32);

        // server_query_version must be propagated (a v2 server advertises >= 2).
        expect(resumed.serverQueryVersion()).toBeGreaterThanOrEqual(2);
      } finally {
        try { await resumed.close(); } catch { /* ok */ }
      }
    }, 30_000);

    it('resume() rejects a bogus ticket (server shuts the connection down)', async () => {
      // A ticket that is not valid AES-256-GCM ciphertext for this server's
      // ticket key cannot be resumed. The server shuts the socket down on
      // resume failure (handshake.rs:602-606) rather than sending an error
      // frame, so the client observes a transport-level rejection.
      await expect(
        resume({
          host: HOST,
          port: server!.port,
          ticket: new Uint8Array(96).fill(0xde), // plausible length, wrong bytes
          serverPubKey: primary!.serverPubKeyPin(),
          tls: { rejectUnauthorized: false },
          origin: ORIGIN,
        }),
      ).rejects.toThrow();
    }, 30_000);

    // Reference the binary path so the skip reason is self-documenting when
    // SERVER_AVAILABLE is false.
    it.skipIf(!SERVER_AVAILABLE)('binary path sanity', () => {
      expect(SERVER_BIN).toBeTruthy();
    });
  },
);
