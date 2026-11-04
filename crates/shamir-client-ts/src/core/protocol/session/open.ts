/**
 * Session open: dial + SCRAM handshake, returning plain data
 * ({@link SessionOpened}) — never a ShamirClient.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Platform } from '../../support/platform.js';
import { WsFramer } from '../framing.js';
import { runHandshake } from '../protocol.js';
import { withConnectTimeout } from '../../client/client-internals.js';
import { DEFAULT_CONNECT_TIMEOUT_MS } from '../../types/connection.js';
import type {
  ConnectOptions as ScramConnectOptions,
  ConnectLocalOptions,
} from '../../types/connection.js';
import { browserWsBinding, ipcBinding } from './binding.js';
import type { SessionLink } from './endpoint.js';
import type { SessionOpened } from './opened.js';

/**
 * Run the SCRAM handshake over an already-open link. Closes the framer and
 * rethrows on any failure.
 */
export async function openSession(
  platform: Platform,
  link: SessionLink,
  username: string,
  password: string,
): Promise<SessionOpened> {
  const framer = new WsFramer(link.socket);

  try {
    const {
      sessionId,
      serverPubKey,
      expiresAtNs,
      resumptionTicket,
      resumptionExpiresAtNs,
      serverQueryVersion,
    } = await runHandshake(platform, framer, username, password, link.binding);
    return {
      framer,
      sessionId,
      serverPubKey,
      expiresAtNs,
      resumptionTicket,
      resumptionExpiresAtNs,
      serverQueryVersion,
      normalizedUser: username.normalize('NFC'),
      binding: link.binding,
    };
  } catch (e) {
    await framer.close();
    throw e;
  }
}

/**
 * Open a WS connection and run the SCRAM handshake. Platform provides
 * crypto + socket.
 */
export async function openScramSession(
  platform: Platform,
  opts: ScramConnectOptions,
): Promise<SessionOpened> {
  const origin = opts.origin ?? `https://${opts.host}`;
  const url = `wss://${opts.host}:${opts.port}/shamir/v1/browser`;
  const socket = await withConnectTimeout(
    platform.openSocket(url, {
      rejectUnauthorized: opts.tls?.rejectUnauthorized ?? true,
      origin,
    }),
    opts.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS,
  );
  return openSession(
    platform,
    { socket, binding: browserWsBinding() },
    opts.username,
    opts.password,
  );
}

/**
 * Open a local-IPC connection (Unix domain socket / Windows Named Pipe,
 * spec TRANSPORT_UNIX.md) and run the SCRAM handshake. Mirrors
 * {@link openScramSession} exactly except: no TLS setup, and the channel
 * binding is `ipcBinding()` (`TRANSPORT_KIND_UNIX`/`BINDING_MODE_NONE`)
 * instead of the WS/browser profile.
 *
 * Node-only — `platform.openIpcSocket` is optional on {@link Platform}
 * precisely because there is no browser implementation; this throws
 * immediately (before touching the network) if the platform lacks it.
 */
export async function openLocalSession(
  platform: Platform,
  opts: ConnectLocalOptions,
): Promise<SessionOpened> {
  if (!platform.openIpcSocket) {
    throw new Error(
      'connectLocal: this Platform does not implement openIpcSocket ' +
        '(local IPC has no browser equivalent — Node only)',
    );
  }
  const socket = await withConnectTimeout(
    platform.openIpcSocket(opts.addr),
    opts.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS,
  );
  return openSession(
    platform,
    { socket, binding: ipcBinding() },
    opts.username,
    opts.password,
  );
}
