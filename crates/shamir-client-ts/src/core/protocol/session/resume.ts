/**
 * Session resume: dial + resume frame, returning plain data
 * ({@link SessionOpened}) — never a ShamirClient.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Platform } from '../../support/platform.js';
import { WsFramer, encode, decode } from '../framing.js';
import {
  asBytes,
  RESUME_OK_SESSION_ID,
  RESUME_OK_EXPIRES_AT_NS,
  RESUME_OK_RESUMPTION_TICKET,
  RESUME_OK_RESUMPTION_EXPIRES_AT_NS,
  RESUME_OK_SERVER_QUERY_VERSION,
} from '../protocol.js';
import { withConnectTimeout } from '../../client/client-internals.js';
import { DEFAULT_CONNECT_TIMEOUT_MS } from '../../types/connection.js';
import type { ResumeOptions } from '../../types/connection.js';
import { browserWsBinding } from './binding.js';
import type { SessionOpened } from './opened.js';

/**
 * Fast reconnection using a resumption ticket from a previous session.
 *
 * Opens a new WS connection and sends a resume frame (ticket + fresh nonce)
 * instead of running the full SCRAM handshake. The server validates the
 * ticket and issues a new session.
 *
 * Throws (and closes the socket) if the server rejects the ticket or the
 * response is malformed.
 */
export async function resumeSession(
  platform: Platform,
  opts: ResumeOptions,
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
  const framer = new WsFramer(socket);
  const binding = browserWsBinding();

  try {
    const clientNonce = platform.randomBytes(32);

    // Wire: { ticket, client_nonce, binding_mode: 0x02 }
    // (0x02 = BINDING_MODE_TLS_NO_EXPORT, from the browser binding profile)
    framer.send(
      encode({
        ticket: opts.ticket,
        client_nonce: clientNonce,
        binding_mode: binding.bindingMode,
      }),
    );

    // Server responds with `ResumeOkWire` (crates/shamir-server/src/
    // connection/wire.rs:80-92): a plain struct with NO `#[serde(rename_all)]`,
    // emitted via `rmp_serde::to_vec` (not `to_vec_named`) → a POSITIONAL
    // msgpack ARRAY in declaration order:
    //   [0] session_id (bytes, 32)
    //   [1] expires_at_ns (u64)
    //   [2] resumption_ticket (bytes; empty Vec when no ticket)
    //   [3] resumption_expires_at_ns (u64; 0 when no ticket)
    //   [4] server_query_version (u8; 0 when absent)
    // This mirrors how `runHandshake` decodes `auth_ok` positionally.
    //
    // On resume REJECTION the server actually shuts the connection down
    // (handshake.rs:602-606), so the client observes a socket close rather
    // than an error frame. The array-vs-error-map guard below is kept
    // defensive (and mirrors runHandshake's auth_ok error-map check): a
    // map with an `error` string is still surfaced as a thrown rejection.
    const rawBytes = await framer.recv();
    const resp = decode(rawBytes) as unknown[] | { error?: string };

    if (!Array.isArray(resp)) {
      const errMap = resp as { error?: string };
      if (typeof errMap.error === 'string') {
        throw new Error(`resume rejected: ${errMap.error}`);
      }
      throw new Error('resume_ok: unexpected non-array response');
    }

    const sessionId = asBytes(
      resp[RESUME_OK_SESSION_ID],
      'resume response: session_id',
    );
    if (sessionId.length !== 32) {
      throw new Error('resume response: session_id must be 32 bytes');
    }

    const expiresAtNs = BigInt(
      resp[RESUME_OK_EXPIRES_AT_NS] as number | bigint,
    );

    // Optional trailing fields. The server always emits indices 2 and 3
    // (even when no ticket is issued: empty Vec and 0u64 respectively), but
    // a pre-v2 server that omits them yields a shorter array — read them
    // defensively, matching runHandshake's handling of the analogous
    // auth_ok trailing fields.
    // An empty/absent resumption_ticket means "no ticket" → undefined.
    const ticketRaw = resp[RESUME_OK_RESUMPTION_TICKET];
    const resumptionTicket =
      ticketRaw instanceof Uint8Array && ticketRaw.length > 0
        ? ticketRaw
        : undefined;
    const resumptionExpiresRaw = resp[RESUME_OK_RESUMPTION_EXPIRES_AT_NS];
    const resumptionExpiresAtNs =
      resumptionTicket !== undefined &&
      resumptionExpiresRaw !== undefined &&
      resumptionExpiresRaw !== null
        ? BigInt(resumptionExpiresRaw as number | bigint)
        : undefined;

    // Max query-language version the server supports (u8, default 0).
    // Absent/malformed → 0 (a pre-v2 server omitting the field is a valid
    // legacy case), mirroring runHandshake's handling of the analogous
    // `auth_ok.server_query_version` positional field (index 7 there).
    const serverQueryVersion =
      typeof resp[RESUME_OK_SERVER_QUERY_VERSION] === 'number'
        ? resp[RESUME_OK_SERVER_QUERY_VERSION]
        : 0;

    return {
      framer,
      sessionId,
      serverPubKey: opts.serverPubKey,
      expiresAtNs,
      resumptionTicket,
      resumptionExpiresAtNs,
      serverQueryVersion,
      normalizedUser: undefined,
      binding,
    };
  } catch (e) {
    await framer.close();
    throw e;
  }
}
