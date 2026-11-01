import { afterEach, describe, expect, it } from 'vitest';
import { decode, encode } from '../../core/protocol/framing.js';
import { Batch } from '../../core/builders/write/batch.js';
import { Query } from '../../core/builders/read/query.js';
import { ApiToken } from '../api-token.js';
import { createTokenHttpClient } from '../token-client.js';
import type { TokenPlatform, TokenHttpResponse } from '../../platform/token/types.js';
import type { BatchOpInput } from '../../core/types/write/batch.js';

const token = ApiToken.from(`shk1.${'A'.repeat(21)}A.${'A'.repeat(42)}A`);
const batchResponse = (id: unknown = 1) => ({ kind: 'batch', response: { id, results: { _: { records: [] }, read: { records: [] } }, execution_plan: [], execution_time_us: 2 } });
const clients: Array<{ close(): Promise<void> }> = [];
afterEach(async () => { await Promise.all(clients.splice(0).map((client) => client.close())); });
function harness(responses: unknown[] = []) {
  const requests: Array<{ url: URL; method: string; body?: unknown; headers: Readonly<Record<string,string>> }> = [];
  const platform: TokenPlatform = {
    async requestHttp(url, _token, init) {
      requests.push({ url, method: init.method, ...(init.body ? { body: decode(init.body) } : {}), headers: init.headers });
      const value = responses.shift() ?? (init.method === 'GET' ? { kind: 'pong' } : batchResponse());
      const response: TokenHttpResponse = { status: 200, headers: { get: (name) => name.toLowerCase() === 'content-type' ? 'application/msgpack' : null }, body: encode(value) };
      return response;
    }, async close() {},
  };
  const options = { auth: 'token' as const, channel: 'http' as const, token, url: 'https://example.test', requestTimeoutMs: 1234 };
  const client = createTokenHttpClient(options, (got) => { expect(got.url).toBe('https://example.test/'); return platform; });
  clients.push(client);
  return { client, requests };
}
const request = () => Batch.create().add('read', Query.from('items')).build();

 describe('Token HTTP client', () => {
  it('pings and executes through one POST with builder batch and normalized URL', async () => {
    const { client, requests } = harness();
    expect(await client.ping()).toEqual({ kind: 'pong' });
    await client.executeWithTouch('db', request());
    expect(requests).toHaveLength(2);
    expect(requests[0]?.method).toBe('GET');
    expect(requests[1]?.url.href).toBe('https://example.test/shamir/v1/request');
    expect(requests[1]?.method).toBe('POST');
    expect((requests[1]?.body as Record<string, { queries: Record<string, unknown> }>).batch.queries.read).toBeDefined();
    expect(requests[1]?.headers['Content-Type']).toBe('application/msgpack');
  });
  it('places normalized visibility waits on execute requests and supports bounded bigint versions', async () => {
    const { client, requests } = harness([batchResponse(), { kind: 'visibility_observed', version: 9007199254740991n, visibility: { state: 'visible' } }]);
    await client.execute('db', request(), { mode: 'bounded', timeout_ms: 10 });
    expect((requests[0]?.body as Record<string, { visibility_wait?: unknown }>).batch.visibility_wait).toBeDefined();
    await expect(client.waitForVisibility('db', 'repo', 9007199254740991n, { mode: 'bounded', timeout_ms: 10 })).resolves.toEqual({ state: 'visible' });
    expect((requests[1]?.body as Record<string, unknown>).version).toBe(9007199254740991n);
  });
  it('allows builder transactions and forwards db helpers through bound executions', async () => {
    const { client, requests } = harness();
    const db = client.db('db');
    const tx = Batch.create().transactional().add('read', Query.from('items')).build();
    await db.execute(tx);
    await db.run(Query.from('items'));
    await db.rows(Query.from('items'));
    await db.query('items').rows();
    await db.withRepo('repo', 'items').rows();
    expect(requests).toHaveLength(5);
    expect(requests.every((request) => request.method === 'POST')).toBe(true);
  });
  it('accepts nested for-each and sub-batch operations', async () => {
    const { client, requests } = harness();
    const inner = Batch.create().add('read', Query.from('items').whereEq('active', true));
    const batch = Batch.create().add('source', Query.from('items'))
      .subBatch('nested', inner)
      .forEach('loop', [{ $query: 'source' }], 'row', inner).build();
    await client.execute('db', batch);
    const wire = requests[0]?.body as Record<string, { queries: Record<string, Record<string, unknown>> }>;
    expect(wire.batch.queries.nested?.batch).toBeDefined();
    expect(wire.batch.queries.loop?.for_each).toBeDefined();
  });
  it('rejects nested and wire-only token operations with exact errors before sending', async () => {
    const { client, requests } = harness();
    const denied = (op: BatchOpInput, code: string) => expect(client.execute('db', Batch.create().add('bad', op).build())).rejects.toMatchObject({ code });
    const nested = (op: BatchOpInput) => Batch.create().add('source', Query.from('items')).subBatch('nested', Batch.create().add('bad', op)).build();
    const nestedForEach = (op: BatchOpInput) => Batch.create().add('source', Query.from('items')).forEach('loop', [{ $query: 'source' }], 'row', Batch.create().add('bad', op)).build();
    await denied({ admin: {} } as unknown as BatchOpInput, 'token_request_denied');
    for (const batch of [nested({ list: 'databases' } as unknown as BatchOpInput), nestedForEach({ list: 'databases' } as unknown as BatchOpInput)]) {
      await expect(client.execute('db', batch)).rejects.toMatchObject({ code: 'token_request_denied' });
    }
    await denied({ subscribe: {} } as unknown as BatchOpInput, 'not_supported_over_rest');
    await denied({ unsubscribe: {} } as unknown as BatchOpInput, 'not_supported_over_rest');
    await expect(client.execute('db', nested({ subscribe: {} } as unknown as BatchOpInput))).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    await expect(client.execute('db', nested({ unsubscribe: {} } as unknown as BatchOpInput))).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    await denied({ create_cursor: {} } as unknown as BatchOpInput, 'not_supported_over_rest');
    await denied({ unknown: {}, from: 'items' } as unknown as BatchOpInput, 'token_request_denied');
    await denied({ insert_into: 'items', values: [{}], records_idmsgpack: [new Uint8Array()] } as unknown as BatchOpInput, 'token_request_denied');
    await expect(client.execute('db', { ...request(), result_encoding: 'id' })).rejects.toMatchObject({ code: 'token_request_denied' });
    await expect(client.execute('db', Batch.create().subBatch('nested', { ...request(), interner_epochs: {} }).build())).rejects.toMatchObject({ code: 'token_request_denied' });
    expect(requests).toHaveLength(0);
  });
  it('reads DDL status from top-level status and normalizes tagged state', async () => {
    const { client } = harness([{ kind: 'ddl_op_status', status: null }, { kind: 'ddl_op_status', status: { op_id: 'op', kind: { other: { description: 'fixture' } }, state: 'in_progress' } }]);
    await expect(client.getDdlOpStatus('db', 'r', 't', 'op')).resolves.toBeNull();
    await expect(client.getDdlOpStatus('db', 'r', 't', 'op')).resolves.toMatchObject({ op_id: 'op', state: { kind: 'in_progress' } });
  });
  it('normalizes DDL state timestamps and rejects malformed required fields', async () => {
    const { client } = harness([
      { kind: 'ddl_op_status', status: { op_id: 'one', kind: { create_hash_index: { index_name: 'idx', table_name: 'items' } }, state: 'in_progress' } },
      { kind: 'ddl_op_status', status: { op_id: 'two', kind: { other: { description: 'fixture' } }, state: { succeeded: { completed_at: 9007199254740991n } } } },
      { kind: 'ddl_op_status', status: { op_id: 'three', kind: { drop_index2: { index_name: 'idx' } }, state: { failed: { detail: 'failure' } } } },
      { kind: 'ddl_op_status', status: { op_id: 'bad', kind: { create_hash_index: { index_name: 'idx' } }, state: 'in_progress' } },
      { kind: 'ddl_op_status', status: { op_id: 'bad', kind: { other: { description: 'fixture' } }, state: { succeeded: { completed_at: 9007199254740992n } } } },
    ]);
    await expect(client.getDdlOpStatus('db', 'r', 't', 'one')).resolves.toMatchObject({ kind: { kind: 'create_hash_index', index_name: 'idx', table_name: 'items' }, state: { kind: 'in_progress' } });
    await expect(client.getDdlOpStatus('db', 'r', 't', 'two')).resolves.toMatchObject({ state: { kind: 'succeeded', completed_at: 9007199254740991 } });
    await expect(client.getDdlOpStatus('db', 'r', 't', 'three')).resolves.toMatchObject({ kind: { kind: 'drop_index2' }, state: { kind: 'failed', detail: 'failure' } });
    await expect(client.getDdlOpStatus('db', 'r', 't', 'bad')).rejects.toMatchObject({ code: 'protocol_error' });
    await expect(client.getDdlOpStatus('db', 'r', 't', 'bad')).rejects.toMatchObject({ code: 'protocol_error' });
  });

  it('allows update where fields and posts them in the batch', async () => {
    const { client, requests } = harness();
    const update = Batch.create().add('change', { update: 'items', set: { count: 2 }, where: { op: 'eq', field: ['id'], value: 1 } }).build();
    await client.execute('db', update);
    const wire = requests[0]?.body as Record<string, { queries: Record<string, Record<string, unknown>> }>;
    expect(wire.batch.queries.change?.where).toBeDefined();
  });
  it('rejects malformed batch and visibility protocol responses', async () => {
    const { client } = harness([{ kind: 'batch', response: { id: 1 } }, { kind: 'visibility_observed', version: 3n, visibility: 'bad' }]);
    await expect(client.execute('db', request())).rejects.toMatchObject({ code: 'protocol_error' });
    await expect(client.waitForVisibility('db', 'r', 4n, { mode: 'bounded', timeout_ms: 1 })).rejects.toMatchObject({ code: 'protocol_error' });
  });

});
