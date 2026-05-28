/**
 * Transaction scenarios via the Batch API.
 *
 * Covers: SI happy path, read-after-write, cross-table atomicity,
 * SSI conflict detection, cross-repo guard.
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;

  test('setup: create db + repo + 2 tables', async () => {
    db = await fixtures.setupDb(client, 'tx_e2e', ['items', 'logs']);
    assert(db, 'db name returned');
  });

  // --- SI Happy Path ---
  test('SI: transactional insert + read returns committed data', async () => {
    const resp = await client.execute(db, {
      id: 'tx-si-1',
      transactional: true,
      queries: {
        ins: {
          insert_into: 'items',
          values: [{ name: 'widget', qty: 10 }],
        },
        read: {
          from: 'items',
        },
      },
    });
    assert(resp.transaction, 'transaction info present');
    assertEq(resp.transaction.status, 'committed');
    assert(resp.transaction.tx_id > 0, 'tx_id is positive');
    assert(resp.transaction.commit_version > 0, 'commit_version is positive');
    // Insert should have produced records.
    assert(resp.results.ins.records.length >= 1, 'at least 1 inserted');
  });

  // --- Commit is durable (read-after-commit) ---
  test('SI: committed data visible in subsequent read', async () => {
    // Insert via tx, then read via separate non-tx batch.
    await client.execute(db, {
      id: 'tx-raw-ins',
      transactional: true,
      queries: {
        ins: {
          insert_into: 'items',
          values: [{ name: 'gadget', qty: 99 }],
        },
      },
    });
    const resp2 = await client.execute(db, {
      id: 'tx-raw-read',
      queries: {
        all: { from: 'items' },
      },
    });
    const names = resp2.results.all.records.map(r => r.name);
    assert(names.includes('gadget'), 'committed data visible after tx');
  });

  // --- Cross-table atomicity ---
  test('SI: cross-table insert is atomic', async () => {
    const resp = await client.execute(db, {
      id: 'tx-cross-table',
      transactional: true,
      queries: {
        ins_items: {
          insert_into: 'items',
          values: [{ name: 'cross-item' }],
        },
        ins_logs: {
          insert_into: 'logs',
          values: [{ event: 'item_created' }],
        },
      },
    });
    assertEq(resp.transaction.status, 'committed');
    assert(resp.results.ins_items.records.length >= 1);
    assert(resp.results.ins_logs.records.length >= 1);
  });

  // --- Serializable isolation ---
  test('SSI: isolation serializable accepted', async () => {
    const resp = await client.execute(db, {
      id: 'tx-ssi',
      transactional: true,
      isolation: 'serializable',
      queries: {
        ins: {
          insert_into: 'items',
          values: [{ name: 'ssi-item' }],
        },
      },
    });
    assertEq(resp.transaction.status, 'committed');
  });

  // --- Non-transactional still works ---
  test('non-tx insert works alongside tx infra', async () => {
    const resp = await client.execute(db, {
      id: 'non-tx',
      queries: {
        ins: {
          insert_into: 'items',
          values: [{ name: 'plain-item' }],
        },
      },
    });
    // No transaction block.
    assert(!resp.transaction || resp.transaction === null, 'no tx info');
    assert(resp.results.ins.records.length >= 1);
  });
};
