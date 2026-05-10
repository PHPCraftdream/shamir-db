/**
 * Typed wire-error surface — `DbResponse::Error { code, message }`
 * is mapped by the SDK into a `ClientError::Db { code, message }`,
 * which the napi binding stringifies to `db error [code]: message`.
 */

'use strict';

module.exports = async function ({ client, test, assertThrows, assert }) {
  test('unknown_db on read against a non-existent database', async () => {
    const err = await assertThrows(() =>
      client.execute('nonexistent_db_xyz', {
        id: 'q',
        queries: { x: { from: 'whatever' } },
      })
    );
    assert(
      /\[unknown_db\]/.test(err.message),
      `expected [unknown_db] tag in: ${err.message}`
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
    assert(
      /\[validation\]/.test(err.message) || /Unknown alias/i.test(err.message),
      `expected validation/Unknown alias in: ${err.message}`
    );
  });

  test('typed error class field is reachable on a real failure', async () => {
    // Trigger a real `unknown_db` and inspect the message tag; this
    // stands in for a schema-shape test. Empty `queries: {}` is in fact
    // accepted by the planner (zero-stage execution), so it's not a
    // good "validation" canary — pin to unknown_db instead.
    const err = await assertThrows(() =>
      client.execute('definitely_no_such_db_42', {
        id: 'q',
        queries: { x: { from: 'whatever' } },
      })
    );
    assert(
      /db error \[unknown_db\]/.test(err.message),
      `expected typed unknown_db tag, got: ${err.message}`
    );
  });
};
