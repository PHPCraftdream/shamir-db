/**
 * E2E — changePassword (spec §12.5) over the local IPC transport.
 *
 * An IPC session (`connectLocal`, `kind: unix`, `profile: plain`) has
 * transport kind UNIX and channel-binding mode NONE. These differ from the
 * browser WebSocket constants used by e2e-change-password.test.ts, so
 * `changePassword` must use the session's real binding rather than the
 * WS defaults, otherwise the server rejects the proof.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../../index.js';
import { connectLocal } from '../../index.js';
import {
  SERVER_AVAILABLE,
  ADMIN_USER,
  ADMIN_PASSWORD,
  startUnixServer,
  connectLocalAs,
} from '../e2e-harness.js';
import type { LocalServerHandle } from '../e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e changePassword over IPC (requires release binary)',
  () => {
    let server: LocalServerHandle | null = null;
    let admin: ShamirClient | null = null;
    let addr = '';

    beforeAll(async () => {
      server = await startUnixServer();
      addr = server.addr;
      try {
        admin = await connectLocal({
          addr,
          username: ADMIN_USER,
          password: ADMIN_PASSWORD,
        });
      } catch (e) {
        console.error('[e2e-change-password-ipc] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
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

    it('changePassword over IPC: old pw stops working, new pw works', async () => {
      const uname = `cpw_ipc_ok_${process.pid}_${Date.now()}`;
      const oldPw = 'old correct horse battery staple';
      const newPw = 'new correct horse battery staple';

      await admin!.createScramUser(uname, oldPw, []);

      const user = await connectLocalAs(addr, uname, oldPw);
      try {
        await user.changePassword(oldPw, newPw);
      } finally {
        try { await user.close(); } catch { /* ok */ }
      }

      await expect(connectLocalAs(addr, uname, oldPw)).rejects.toThrow();

      const reconnected = await connectLocalAs(addr, uname, newPw);
      await reconnected.close();
    });

    it('changePassword over IPC: wrong old password is rejected', async () => {
      const uname = `cpw_ipc_fail_${process.pid}_${Date.now()}`;
      const correctPw = 'the actual old password';
      const wrongPw = 'a deliberately wrong old password';
      const newPw = 'should never be set';

      await admin!.createScramUser(uname, correctPw, []);

      const user = await connectLocalAs(addr, uname, correctPw);
      try {
        await expect(
          user.changePassword(wrongPw, newPw),
        ).rejects.toThrow();

        const reconnected = await connectLocalAs(addr, uname, correctPw);
        await reconnected.close();
      } finally {
        try { await user.close(); } catch { /* ok */ }
      }
    });
  },
);
