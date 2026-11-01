import { describe, expect, it } from 'vitest';
import { validateTokenBatchResponse } from '../token-response.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';

function response(result: Record<string, unknown> = {}) {
  return {
    id: 7,
    results: { read: { records: [], ...result } },
    execution_plan: [['read']],
    execution_time_us: 12,
  };
}

function expectProtocolError(value: Record<string, unknown>): void {
  try {
    validateTokenBatchResponse(value);
    throw new Error('expected validation to fail');
  } catch (error) {
    expect(error).toBeInstanceOf(TokenTransportError);
    expect(error).toMatchObject({ code: 'protocol_error' });
  }
}

describe('validateTokenBatchResponse', () => {
  it('normalizes Rust externally tagged DDL states in batch results', () => {
    const cases: Array<[unknown, unknown]> = [
      ['in_progress', { kind: 'in_progress' }],
      ['unknown', { kind: 'unknown' }],
      [{ succeeded: { completed_at: 7n } }, { kind: 'succeeded', completed_at: 7 }],
      [{ succeeded_via_crash_recovery: { completed_at_restart: 8 } }, { kind: 'succeeded_via_crash_recovery', completed_at_restart: 8 }],
      [{ failed: { detail: 'failure' } }, { kind: 'failed', detail: 'failure' }],
      [{ kind: 'succeeded', completed_at: 9 }, { kind: 'succeeded', completed_at: 9 }],
    ];
    for (const [state, normalized] of cases) {
      const fixture = response({ ddl_status: state });
      expect(validateTokenBatchResponse(fixture).results.read?.ddl_status).toEqual(normalized);
    }
  });

  it('rejects malformed externally tagged DDL variants', () => {
    for (const state of [
      'missing', { succeeded: {} }, { succeeded: { completed_at: -1 } },
      { succeeded: { completed_at: 1, extra: true } }, { failed: {} },
      { succeeded: { completed_at: 1 }, failed: { detail: 'x' } },
      [], Object.create({ succeeded: { completed_at: 1 } }),
    ]) expectProtocolError(response({ ddl_status: state }));
  });

  it('accepts full query stats, pagination, explain, versions, corrupt records, and receipt metadata', () => {
    const fixture = response({
      records: [{ id: 3 }],
      stats: { records_scanned: 4, records_returned: 1, execution_time_us: 8, index_used: 'items_by_id' },
      pagination: { total_count: 10, total_pages: 2, current_page: 1, page_size: 5, has_next: true, has_prev: false },
      explain: { plan_type: 'IndexScan', index_used: 'items_by_id', estimated_rows: 10 },
      versions: [1, 9007199254740992n],
      corrupt_records: [{ table: 'items', id: 'abc' }],
      receipt: { tx_id: 12, status: 'committed', materialized: true, visibility: { state: 'visible' } },
    });
    expect(validateTokenBatchResponse(fixture)).toBe(fixture);
  });

  it('accepts a valid top-level transaction receipt', () => {
    const fixture = { ...response(), transaction: { tx_id: 9n, status: 'aborted', materialized: false } };
    expect(validateTokenBatchResponse(fixture)).toBe(fixture);
  });

  it('rejects invalid and missing required stats fields', () => {
    expectProtocolError(response({ stats: { records_scanned: 'invalid', records_returned: 1, execution_time_us: 1, index_used: null } }));
    expectProtocolError(response({ stats: { records_scanned: 1, records_returned: 1, index_used: null } }));
  });

  it('rejects invalid pagination flags', () => {
    expectProtocolError(response({ pagination: { has_next: 1, has_prev: false } }));
    expectProtocolError(response({ pagination: { has_next: false } }));
  });

  it('rejects invalid explain plans', () => {
    expectProtocolError(response({ explain: { plan_type: 'UnknownPlan' } }));
    expectProtocolError(response({ explain: { plan_type: 'FullScan', estimated_rows: -1 } }));
  });

  it('rejects negative versions', () => {
    expectProtocolError(response({ versions: [-1] }));
    expectProtocolError(response({ versions: [-1n] }));
  });

  it('rejects non-wire row values and binary row records', () => {
    expectProtocolError(response({ records: [{ id: Symbol('hidden') }] }));
    expectProtocolError(response({ records: [new Uint8Array([1, 2])] }));
  });

  it('rejects non-string transaction reason and error fields on transactions and receipts', () => {
    const malformed = [1, null, {}, new Uint8Array([1])];
    for (const field of ['reason', 'error']) {
      for (const invalid of malformed) {
        expectProtocolError({ ...response(), transaction: { tx_id: 1, status: 'aborted', materialized: false, [field]: invalid } });
        expectProtocolError(response({ receipt: { tx_id: 1, status: 'aborted', materialized: false, [field]: invalid } }));
      }
    }
  });

  it('preserves valid transaction reason and error strings and legacy nested wire values', () => {
    const bytes = new Uint8Array([1, 2]);
    const large = 9007199254740992n;
    const fixture = {
      ...response({ records: [{ id: 'record', nested: { bytes, large } }], value: { bytes, large } }),
      transaction: { tx_id: 9n, status: 'aborted', materialized: false, reason: 'cancelled' },
    };
    const result = validateTokenBatchResponse(fixture);
    expect(result).toBe(fixture);
    const data = result as unknown as {
      transaction: { reason: string };
      results: { read: { records: Array<{ nested: { bytes: Uint8Array; large: bigint } }>; value: { bytes: Uint8Array; large: bigint } } };
    };
    expect(data.transaction.reason).toBe('cancelled');
    expect(data.results.read.records[0].nested.bytes).toBe(bytes);
    expect(data.results.read.records[0].nested.large).toBe(large);
    expect(data.results.read.value.bytes).toBe(bytes);
    expect(data.results.read.value.large).toBe(large);

    const receiptFixture = response({ receipt: { tx_id: 10, status: 'committed', materialized: true, visibility: { state: 'visible' }, error: 'retained' } });
    const receiptResult = validateTokenBatchResponse(receiptFixture);
    expect(receiptResult).toBe(receiptFixture);
    expect(receiptResult.results.read).toMatchObject({ receipt: { error: 'retained' } });
  });

  it('rejects malformed transaction visibility metadata as a protocol error', () => {
    expectProtocolError({ ...response(), transaction: { tx_id: 1, status: 'committed', materialized: true } });
    expectProtocolError(response({ receipt: { tx_id: 1, status: 'committed', materialized: true, visibility: { state: 'pending', reason: 'timeout', visible_through: 3, blocking_version: 5 } } }));
  });
});
