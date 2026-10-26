/**
 * RU6 golden/contract tests — `RecordId` on the wire is a canonical base58
 * string (RV-QUERY-TYPES-A984DFED), applied to `QueryResult::op_id`,
 * `DdlOpStatus.op_id`, `DropIndexOp.request_id` and `RenameIndexOp.request_id`.
 *
 * Mirrors crates/shamir-query-types/src/wire/tests/record_id_string_tests.rs.
 * The byte oracle is derived transparently from the rmp_serde named-map
 * encoding rules (fixmap header `0x80 + n`, fixstr `0xa0 + len`) — no runtime
 * capture was possible (tests are CI-only per the repo's compile/build
 * policy), so the expected byte strings are built field-by-field with local
 * helpers instead of hard-coded hex.
 *
 * The TS side decodes those goldens with `@msgpack/msgpack` and asserts the
 * op/request id arrives as a plain JS **string** (msgpack str — never bin8),
 * and drives the real `client.getDdlOpStatus()` against a fake transport to
 * prove the poll request encodes `op_id` as a msgpack str too.
 *
 * Fixed RecordId used everywhere: raw bytes `0x01..=0x10`;
 * base58btc → `8DfbjXLth7APvt3qQPgtf`.
 */

import { describe, it, expect } from 'vitest';
import { decode as msgpackDecode } from '@msgpack/msgpack';
import { encode, WsFramer } from '../framing.js';
import type { Socket } from '../../support/platform.js';
import { ShamirClient } from '../../client/client.js';

// ─── byte oracle helpers (mirror the Rust test's fixstr/fixmap) ──────────────

/** The canonical base58 rendering of raw bytes 0x01..=0x10. */
const OP_ID = '8DfbjXLth7APvt3qQPgtf';
const BASE58_RE = /^[1-9A-HJ-NP-Za-km-z]{16,22}$/;

function fixstr(s: string): number[] {
  const out = [0xa0 + s.length];
  for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i));
  return out;
}

function fixmap(entries: Array<[string, number[]]>): Uint8Array {
  const out: number[] = [0x80 + entries.length];
  for (const [key, value] of entries) {
    out.push(...fixstr(key), ...value);
  }
  return new Uint8Array(out);
}

// ─── golden byte arrays (same values the Rust tests assert byte-for-byte) ────

/** `QueryResult { records: [], op_id: Some(fixed_id()) }` — named map. */
const QUERY_RESULT_GOLDEN: Uint8Array = fixmap([
  ['records', [0x90]],
  ['op_id', fixstr(OP_ID)],
]);

/**
 * `DbResponse::DdlOpStatus { status: Some(DdlOpStatus {
 *   op_id: fixed_id(), kind: Other { description: "x" }, state: InProgress }) }`.
 */
const DDL_OP_STATUS_GOLDEN: Uint8Array = fixmap([
  ['kind', fixstr('ddl_op_status')],
  [
    'status',
    Array.from(
      fixmap([
        ['op_id', fixstr(OP_ID)],
        ['kind', Array.from(fixmap([['other', Array.from(fixmap([['description', fixstr('x')]]))]]))],
        ['state', fixstr('in_progress')],
      ]),
    ),
  ],
]);

/** `DropIndexOp { drop_index:"idx", table:"t", repo:"main", request_id: Some(id) }`. */
const DROP_INDEX_GOLDEN: Uint8Array = fixmap([
  ['drop_index', fixstr('idx')],
  ['table', fixstr('t')],
  ['repo', fixstr('main')],
  ['request_id', fixstr(OP_ID)],
]);

/** `RenameIndexOp { rename_index:"old", to:"new", table:"t", repo:"main", request_id: Some(id) }`. */
const RENAME_INDEX_GOLDEN: Uint8Array = fixmap([
  ['rename_index', fixstr('old')],
  ['to', fixstr('new')],
  ['table', fixstr('t')],
  ['repo', fixstr('main')],
  ['request_id', fixstr(OP_ID)],
]);

/**
 * Assert the byte immediately preceding the id bytes is a fixstr marker
 * (0xa0..=0xbf), never bin8 (0xc4) — the wire-level str assertion that
 * survives even if someone swaps the decoder.
 */
function assertFixstrMarker(bytes: Uint8Array, key: string, id: string): void {
  const keyBytes = Array.from(fixstr(key));
  for (let i = 0; i + keyBytes.length + 1 <= bytes.length; i++) {
    let match = true;
    for (let j = 0; j < keyBytes.length; j++) {
      if (bytes[i + j] !== keyBytes[j]) {
        match = false;
        break;
      }
    }
    if (match) {
      const marker = bytes[i + keyBytes.length];
      expect(marker).toBeGreaterThanOrEqual(0xa0);
      expect(marker).toBeLessThanOrEqual(0xbf);
      expect(marker).not.toBe(0xc4);
      expect(marker - 0xa0).toBe(id.length);
      return;
    }
  }
  throw new Error(`key "${key}" not found in golden bytes`);
}

// ─── fake transport (mirrors protocol.test.ts / framing.test.ts) ─────────────

class FakeSocket implements Socket {
  sent: Uint8Array[] = [];
  messageHandler: (_data: Uint8Array) => void = () => {};
  closeHandler: (_err?: Error) => void = () => {};
  closed = false;

  send(data: Uint8Array): void {
    if (this.closed) throw new Error('connection closed');
    this.sent.push(data);
  }
  onMessage(h: (data: Uint8Array) => void): void {
    this.messageHandler = h;
  }
  onClose(h: (err?: Error) => void): void {
    this.closeHandler = h;
  }
  close(): Promise<void> {
    this.closed = true;
    return Promise.resolve();
  }
  /** Push a body, wrapping it in the [u32_be length][payload] framing. */
  pushFrame(body: Uint8Array): void {
    const buf = new Uint8Array(4 + body.length);
    const len = body.length >>> 0;
    buf[0] = (len >>> 24) & 0xff;
    buf[1] = (len >>> 16) & 0xff;
    buf[2] = (len >>> 8) & 0xff;
    buf[3] = len & 0xff;
    buf.set(body, 4);
    this.messageHandler(buf);
  }
}

/**
 * Build a ShamirClient over a fake socket/framer without a real handshake
 * (the constructor is private — invoked through an any-cast, exactly like
 * the values it stores are only `platform` + `framer` + session metadata).
 */
function makeClient(): { client: ShamirClient; socket: FakeSocket } {
  const socket = new FakeSocket();
  const framer = new WsFramer(socket);
  const Ctor = ShamirClient as unknown as new (...args: unknown[]) => ShamirClient;
  const client = new Ctor(
    {}, // platform — not touched by getDdlOpStatus/sendDbRequest/readLoop
    framer,
    new Uint8Array(32).fill(0x01), // sessionId
    new Uint8Array(32).fill(0x02), // serverPubKey
    BigInt('1830000000000000000'), // expiresAtNs
    undefined,
    undefined,
    0, // serverQueryVersion
    0, // requestTimeoutMs — disable the pending-request deadline timer
  );
  return { client, socket };
}

// ─── tests ───────────────────────────────────────────────────────────────────

describe('RecordId wire goldens — decode as base58 str (RV-QUERY-TYPES-A984DFED)', () => {
  it('QueryResult golden: op_id decodes to a base58 string, fixstr not bin8', () => {
    assertFixstrMarker(QUERY_RESULT_GOLDEN, 'op_id', OP_ID);
    const decoded = msgpackDecode(QUERY_RESULT_GOLDEN) as { op_id: unknown };
    expect(typeof decoded.op_id).toBe('string');
    expect(decoded.op_id).toMatch(BASE58_RE);
    expect(decoded.op_id).toBe(OP_ID);
  });

  it('DbResponse::DdlOpStatus golden: status.op_id decodes to a base58 string', () => {
    assertFixstrMarker(DDL_OP_STATUS_GOLDEN, 'op_id', OP_ID);
    const decoded = msgpackDecode(DDL_OP_STATUS_GOLDEN) as {
      kind: string;
      status: { op_id: unknown };
    };
    expect(decoded.kind).toBe('ddl_op_status');
    expect(typeof decoded.status.op_id).toBe('string');
    expect(decoded.status.op_id).toMatch(BASE58_RE);
    expect(decoded.status.op_id).toBe(OP_ID);
  });

  it('DropIndexOp golden: request_id decodes to a base58 string', () => {
    assertFixstrMarker(DROP_INDEX_GOLDEN, 'request_id', OP_ID);
    const decoded = msgpackDecode(DROP_INDEX_GOLDEN) as { request_id: unknown };
    expect(typeof decoded.request_id).toBe('string');
    expect(decoded.request_id).toMatch(BASE58_RE);
    expect(decoded.request_id).toBe(OP_ID);
  });

  it('RenameIndexOp golden: request_id decodes to a base58 string', () => {
    assertFixstrMarker(RENAME_INDEX_GOLDEN, 'request_id', OP_ID);
    const decoded = msgpackDecode(RENAME_INDEX_GOLDEN) as { request_id: unknown };
    expect(typeof decoded.request_id).toBe('string');
    expect(decoded.request_id).toMatch(BASE58_RE);
    expect(decoded.request_id).toBe(OP_ID);
  });
});

describe('getDdlOpStatus over a fake transport encodes op_id as a msgpack str', () => {
  it('poll request carries op_id as fixstr bytes; response string round-trips', async () => {
    const { client, socket } = makeClient();

    const pending = client.getDdlOpStatus('db', 'main', 't', OP_ID);

    // Let sendDbRequest's synchronous body run, then inspect the sent frame.
    await Promise.resolve();
    expect(socket.sent.length).toBe(1);
    const envelope = msgpackDecode(socket.sent[0].subarray(4)) as {
      rid: number;
      req: Uint8Array;
    };
    expect(typeof envelope.rid).toBe('number');

    // Inner DbRequest bytes: op_id must be encoded as a msgpack str (fixstr
    // marker 0xb5 = 0xa0 + 21), never bin8 (0xc4).
    const reqBytes = envelope.req;
    assertFixstrMarker(reqBytes, 'op_id', OP_ID);
    const req = msgpackDecode(reqBytes) as {
      op: string;
      db: string;
      repo: string;
      table: string;
      op_id: unknown;
    };
    expect(req.op).toBe('get_ddl_op_status');
    expect(req.db).toBe('db');
    expect(req.repo).toBe('main');
    expect(req.table).toBe('t');
    expect(typeof req.op_id).toBe('string');
    expect(req.op_id).toMatch(BASE58_RE);
    expect(req.op_id).toBe(OP_ID);

    // Respond so the pending request resolves (mirrors the Rust
    // `poll_by_response_string_round_trip` test): decode the response as an
    // untyped `{ kind, status: { op_id: String } }` and hand it back.
    socket.pushFrame(
      encode({
        rid: envelope.rid,
        res: encode({
          kind: 'ddl_op_status',
          status: {
            op_id: OP_ID,
            kind: { other: { description: 'x' } },
            state: 'in_progress',
          },
        }),
      }),
    );

    const status = await pending;
    expect(status).not.toBeNull();
    expect(status?.op_id).toBe(OP_ID);
    expect(typeof status?.op_id).toBe('string');
  });

  it('returns null when the server reports the op as unknown', async () => {
    const { client, socket } = makeClient();

    const pending = client.getDdlOpStatus('db', 'main', 't', OP_ID);
    await Promise.resolve();

    const envelope = msgpackDecode(socket.sent[0].subarray(4)) as {
      rid: number;
    };
    socket.pushFrame(
      encode({ rid: envelope.rid, res: encode({ kind: 'ddl_op_status', status: null }) }),
    );

    expect(await pending).toBeNull();
  });
});
