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

  it('rejects malformed transaction visibility metadata as a protocol error', () => {
    expectProtocolError({ ...response(), transaction: { tx_id: 1, status: 'committed', materialized: true } });
    expectProtocolError(response({ receipt: { tx_id: 1, status: 'committed', materialized: true, visibility: { state: 'pending', reason: 'timeout', visible_through: 3, blocking_version: 5 } } }));
  });
});
