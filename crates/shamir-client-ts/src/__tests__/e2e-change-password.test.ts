/**
 * E2E — changePassword (spec §12.5) end-to-end against a live server.
 *
 * Tests the full two-step SCRAM change-password flow:
 *   1. Challenge → server issues fresh nonce + echoes current salt/KDF.
 *   2. Verify → client submits old-password proof + new pre-derived
 *      credentials; server verifies and persists.
 *
 * Exercises: success path (old pw invalid after, new pw works), and the
 * old-proof-mismatch rejection path (wrong old password → AuthFailed).
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import {
  SERVER_BIN,
  SERVER_AVAILABLE,
  HOST,
  ORIGIN,
  startServer,
  connectAdmin,
  connectAs,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e changePassword (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let admin: ShamirClient | null = null;
    let PORT = 0;

    beforeAll(async () => {
      server = await startServer();
      PORT = server.port;
      admin = await connectAdmin(HOST, PORT);
    }, 60_000);

    afterAll(async () => {
      if (admin) {
        try { await admin.close(); } catch { /* ok */ }
        admin = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    it('changePassword: old pw stops working, new pw works', async () => {
      const uname = `cpw_ok_${process.pid}_${Date.now()}`;
      const oldPw = 'old correct horse battery staple';
      const newPw = 'new correct horse battery staple';

      // Admin creates a SCRAM user.
      await admin!.createScramUser(uname, oldPw, []);

      // User logs in with the old password.
      const user = await connectAs(HOST, PORT, uname, oldPw);
      try {
        // Change password — should resolve without error.
        await user.changePassword(oldPw, newPw);
      } finally {
        // The server kills other sessions on password change, but the
        // caller's own session survives — close it cleanly.
        try { await user.close(); } catch { /* ok */ }
      }

      // Old password must now fail.
      await expect(connectAs(HOST, PORT, uname, oldPw)).rejects.toThrow();

      // New password must work.
      const reconnected = await connectAs(HOST, PORT, uname, newPw);
      await reconnected.close();
    });

    it('changePassword: wrong old password is rejected', async () => {
      const uname = `cpw_fail_${process.pid}_${Date.now()}`;
      const correctPw = 'the actual old password';
      const wrongPw = 'a deliberately wrong old password';
      const newPw = 'should never be set';

      // Admin creates a SCRAM user.
      await admin!.createScramUser(uname, correctPw, []);

      // User logs in with the correct password.
      const user = await connectAs(HOST, PORT, uname, correctPw);
      try {
        // Attempt with wrong old password — server should reject the
        // proof mismatch (AuthFailed), proving the verification is real.
        await expect(
          user.changePassword(wrongPw, newPw),
        ).rejects.toThrow();

        // The original password must still work (change was rejected).
        const reconnected = await connectAs(HOST, PORT, uname, correctPw);
        await reconnected.close();
      } finally {
        try { await user.close(); } catch { /* ok */ }
      }
    });
  },
);
