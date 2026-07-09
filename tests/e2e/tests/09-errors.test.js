/**
 * Typed wire-error surface (Finding 2.1) — `DbResponse::Error { code, message }`
 * is mapped by the SDK into a `ClientError::Db { code, message }`, whose Display
 * is `db error [code]: message`. The napi binding surfaces this via the JS
 * error's `.message`.
 *
 * NOTE (Finding 2.1, node-binding scope): a first-class typed `.code`/
 * `.retryable` PROPERTY on the native binding's error is blocked on napi-rs 2.x
 * (its `Status` is a fixed enum, and the `#[napi]` async signatures are wired to
 * `Result<T, Error<Status>>`), so callers of the NATIVE binding still recover
 * the code from the message tag. The TS ws-client (the primary SDK) DOES expose
 * a fully-typed `ShamirDbError { code, retryable }` (see
 * `shamir-client-ts/src/core/errors.ts`, covered by
 * `src/core/__tests__/client.test.ts`). This e2e therefore parses the code out
 * of the message and classifies it against the SAME shared retryable table the
 * TS SDK uses, proving the code round-trips end-to-end from the server.
 */

'use strict';

// Mirror of the TS SDK's RETRYABLE_ERROR_CODES (core/errors.ts) so this
// binding-level e2e can classify a code without importing the TS module.
const RETRYABLE = new Set([
  'timeout',
  'lock_timeout',
  'tx_conflict',
  'read_only_replica',
]);

/** Recover the typed server code from the `db error [code]: message` string. */
function codeOf(err) {
  const m = /db error \[([a-z_]+)\]/.exec(err.message);
  return m ? m[1] : undefined;
}

module.exports = async function ({ client, test, assertThrows, assert }) {
  test('unknown_db code round-trips and classifies as non-retryable', async () => {
    const err = await assertThrows(() =>
      client.execute('nonexistent_db_xyz', {
        id: 'q',
        queries: { x: { from: 'whatever' } },
      })
    );
    const code = codeOf(err);
    assert(
      code === 'unknown_db',
      `expected code 'unknown_db', got code=${code} message=${err.message}`
    );
    assert(
      RETRYABLE.has(code) === false,
      `unknown_db must classify as non-retryable`
    );
  });

  test('validation error: $query reference to unknown alias', async () => {
    const err = await assertThrows(() =>
      client.execute('default', {
        id: 'bad-ref',
        queries: {
          a: { from: '__databases', where: { op: 'eq', field: ['name'], value: 'default' } },
          b: {
            from: '__databases',
            where: {
              op: 'eq',
              field: ['name'],
              value: { $query: 'nonexistent_alias', path: '[0].name' },
            },
          },
        },
      })
    );
    const code = codeOf(err);
    assert(
      code === 'validation' || /Unknown alias/i.test(err.message),
      `expected validation/Unknown alias in: ${err.message}`
    );
    if (code !== undefined) {
      assert(
        RETRYABLE.has(code) === false,
        `validation must classify as non-retryable`
      );
    }
  });

  test('typed code is recoverable on a real failure', async () => {
    const err = await assertThrows(() =>
      client.execute('definitely_no_such_db_42', {
        id: 'q',
        queries: { x: { from: 'whatever' } },
      })
    );
    assert(
      codeOf(err) === 'unknown_db',
      `expected typed unknown_db code, got: ${err.message}`
    );
  });
};
