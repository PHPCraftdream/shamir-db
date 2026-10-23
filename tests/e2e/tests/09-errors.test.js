/**
 * Typed wire-error surface — the napi binding's wrapper maps every
 * `DbResponse::Error { code, message }` the server returns into a
 * `ShamirDbError` carrying `.code` and `.retryable` (see
 * `crates/shamir-client-node/js/wrapper.js`).
 *
 * These assertions go through the PUBLIC `shamir-client` package: the shared
 * `ctx.client` connected by the orchestrator (`e2e.test.js`) IS a
 * `shamir-client` `ShamirClient`, so a real server error round-trips the
 * typed surface end-to-end. Queries are built with the typed query builder
 * from the TS SDK (`@shamir/client`), exactly as in the neighbouring tests.
 */

'use strict';

const { ShamirDbError, isRetryableCode } = require('shamir-client');
const { Query, filter } = require('@shamir/client');

module.exports = async function ({ client, test, assertThrows, assert }) {
  test('unknown_db surfaces as ShamirDbError with code and retryable=false', async () => {
    const err = await assertThrows(() =>
      client.execute('nonexistent_db_xyz', {
        id: 'q',
        queries: { x: Query.from('whatever').build() },
      })
    );
    assert(
      err instanceof ShamirDbError,
      `expected ShamirDbError, got ${err && err.constructor && err.constructor.name}`
    );
    assert(
      err.code === 'unknown_db',
      `expected code 'unknown_db', got code=${err.code}`
    );
    assert(
      err.retryable === false,
      `unknown_db must be non-retryable, got retryable=${err.retryable}`
    );
  });

  test('non-retryable validation error is a typed ShamirDbError', async () => {
    const err = await assertThrows(() =>
      client.execute('default', {
        id: 'bad-ref',
        queries: {
          a: Query.from('__databases')
            .where(filter.eq('name', 'default'))
            .build(),
          b: Query.from('__databases')
            .where(
              filter.eq('name', filter.queryRef('nonexistent_alias', '[0].name'))
            )
            .build(),
        },
      })
    );
    assert(err instanceof ShamirDbError, 'expected typed ShamirDbError');
    assert(
      err.code === 'validation' || /Unknown alias/i.test(err.message),
      `expected validation/Unknown alias, got code=${err.code} message=${err.message}`
    );
    assert(
      err.retryable === false,
      `validation must be non-retryable, got code=${err.code}`
    );
  });

  test('isRetryableCode matches the wrapper retryable table', () => {
    for (const code of ['timeout', 'lock_timeout', 'tx_conflict', 'read_only_replica']) {
      assert(isRetryableCode(code) === true, `${code} must be retryable`);
    }
    for (const code of ['unknown_db', 'validation', 'permission_denied', 'not_found', 'user_exists']) {
      assert(isRetryableCode(code) === false, `${code} must be non-retryable`);
    }
  });

  test('success path: typed query through the public package resolves', async () => {
    const resp = await client.execute('default', {
      id: 'ok-read',
      queries: {
        dbs: Query.from('__databases')
          .where(filter.eq('name', 'default'))
          .build(),
      },
    });
    const recs = resp.results.dbs.records;
    assert(recs.length >= 1, 'expected at least the default database');
  });
};
