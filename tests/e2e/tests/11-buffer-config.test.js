/**
 * Per-table buffer config DDL — get / set / alter.
 *
 * Wire format (admin/types.rs):
 *   {get_buffer_config:'<table>', repo?:'<repo>'}
 *     → records:[{ table, repo, config: {...} | null }]
 *
 *   {set_buffer_config:'<table>', repo?:'<repo>',
 *    config:{max_bytes, max_entries, ttl_ms?, flush_interval_ms,
 *            flush_batch_size}}
 *     → records:[{ set_buffer_config, repo, config }]
 *
 *   {alter_buffer_config:'<table>', repo?:'<repo>', patch:{...}}
 *     → records:[{ alter_buffer_config, repo, config }]
 *
 * Patch three-state contract for ttl_ms:
 *   - omit key             → keep current
 *   - explicit `null`      → clear TTL
 *   - numeric value (ms)   → set TTL
 *
 * The other knobs (max_bytes, max_entries, flush_interval_ms,
 * flush_batch_size) are plain optional.
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  function fullCfg(overrides = {}) {
    return Object.assign(
      {
        max_bytes: 1048576,
        max_entries: 500,
        ttl_ms: 7000,
        flush_interval_ms: 333,
        flush_batch_size: 48,
      },
      overrides
    );
  }

  test('get_buffer_config returns null when never set', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_unset', ['t']);
    const resp = await client.execute(dbName, {
      id: 'get-unset',
      queries: { g: { get_buffer_config: 't', repo: 'main' } },
    });
    const row = resp.results.g.records[0];
    assertEq(row.table, 't');
    assertEq(row.repo, 'main');
    assert(
      row.config === null,
      `expected null config, got ${JSON.stringify(row.config)}`
    );
  });

  test('set_buffer_config then get_buffer_config roundtrip', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_set', ['t']);

    const setResp = await client.execute(dbName, {
      id: 'set',
      queries: {
        s: { set_buffer_config: 't', repo: 'main', config: fullCfg() },
      },
    });
    const setRow = setResp.results.s.records[0];
    assertEq(setRow.set_buffer_config, 't');
    assertEq(setRow.config.max_bytes, 1048576);
    assertEq(setRow.config.ttl_ms, 7000);
    assertEq(setRow.config.flush_interval_ms, 333);

    const getResp = await client.execute(dbName, {
      id: 'get',
      queries: { g: { get_buffer_config: 't', repo: 'main' } },
    });
    const cfg = getResp.results.g.records[0].config;
    assert(cfg !== null, 'config must be present after set');
    assertEq(cfg.max_bytes, 1048576);
    assertEq(cfg.max_entries, 500);
    assertEq(cfg.ttl_ms, 7000);
    assertEq(cfg.flush_interval_ms, 333);
    assertEq(cfg.flush_batch_size, 48);
  });

  test('alter_buffer_config partial update keeps untouched knobs', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_alter', ['t']);

    // Seed with a known full config.
    await client.execute(dbName, {
      id: 'seed',
      queries: {
        s: { set_buffer_config: 't', repo: 'main', config: fullCfg() },
      },
    });

    // Bump just two knobs.
    const alterResp = await client.execute(dbName, {
      id: 'alter',
      queries: {
        a: {
          alter_buffer_config: 't',
          repo: 'main',
          patch: { flush_interval_ms: 1000, max_entries: 9999 },
        },
      },
    });
    const altered = alterResp.results.a.records[0].config;
    assertEq(altered.flush_interval_ms, 1000);
    assertEq(altered.max_entries, 9999);
    // Untouched knobs survive.
    assertEq(altered.max_bytes, 1048576);
    assertEq(altered.flush_batch_size, 48);
    // ttl_ms NOT in patch — must keep seeded value (three-state).
    assertEq(altered.ttl_ms, 7000);

    const getResp = await client.execute(dbName, {
      id: 'after',
      queries: { g: { get_buffer_config: 't', repo: 'main' } },
    });
    const cfg = getResp.results.g.records[0].config;
    assertEq(cfg.flush_interval_ms, 1000);
    assertEq(cfg.max_entries, 9999);
    assertEq(cfg.ttl_ms, 7000);
    assertEq(cfg.max_bytes, 1048576);
  });

  test('alter_buffer_config with ttl_ms:null clears TTL', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_ttlnull', ['t']);

    await client.execute(dbName, {
      id: 'seed',
      queries: {
        s: { set_buffer_config: 't', repo: 'main', config: fullCfg() },
      },
    });

    const alterResp = await client.execute(dbName, {
      id: 'alter',
      queries: {
        a: {
          alter_buffer_config: 't',
          repo: 'main',
          patch: { ttl_ms: null },
        },
      },
    });
    const cfg = alterResp.results.a.records[0].config;
    assert(
      cfg.ttl_ms === null || cfg.ttl_ms === undefined,
      `expected null/undefined ttl_ms, got ${JSON.stringify(cfg.ttl_ms)}`
    );
    // Everything else unchanged.
    assertEq(cfg.max_bytes, 1048576);
    assertEq(cfg.flush_interval_ms, 333);
  });

  test('alter from no prior config falls back to engine defaults', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_fresh', ['t']);

    const alterResp = await client.execute(dbName, {
      id: 'alter',
      queries: {
        a: {
          alter_buffer_config: 't',
          repo: 'main',
          patch: { max_entries: 42 },
        },
      },
    });
    const cfg = alterResp.results.a.records[0].config;
    assertEq(cfg.max_entries, 42);
    // Engine defaults from MemBufferConfig::default.
    assertEq(cfg.flush_interval_ms, 500);
    assertEq(cfg.flush_batch_size, 256);
  });

  test('set_buffer_config persists across batches', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_persist', ['t']);

    await client.execute(dbName, {
      id: 'set',
      queries: {
        s: { set_buffer_config: 't', repo: 'main', config: fullCfg() },
      },
    });

    // Brand-new batch, brand-new request id — has to land the same
    // value because it was written into info_store.
    const resp = await client.execute(dbName, {
      id: 'get-later',
      queries: { g: { get_buffer_config: 't', repo: 'main' } },
    });
    const cfg = resp.results.g.records[0].config;
    assertEq(cfg.max_bytes, 1048576);
    assertEq(cfg.ttl_ms, 7000);
  });

  test('per-table configs are independent', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_indep', ['a', 'b']);

    await client.execute(dbName, {
      id: 'set-a',
      queries: {
        s: {
          set_buffer_config: 'a',
          repo: 'main',
          config: fullCfg({ max_entries: 111 }),
        },
      },
    });

    const aResp = await client.execute(dbName, {
      id: 'get-a',
      queries: { g: { get_buffer_config: 'a', repo: 'main' } },
    });
    const bResp = await client.execute(dbName, {
      id: 'get-b',
      queries: { g: { get_buffer_config: 'b', repo: 'main' } },
    });

    assertEq(aResp.results.g.records[0].config.max_entries, 111);
    assert(
      bResp.results.g.records[0].config === null,
      `table b must have null config: ${JSON.stringify(bResp.results.g.records[0].config)}`
    );
  });

  test('repo defaults to main when omitted', async () => {
    const dbName = await fixtures.setupDb(client, 'bcfg_dfltrepo', ['t']);

    // No `repo` key — should target `main` per default_repo().
    const setResp = await client.execute(dbName, {
      id: 'set',
      queries: {
        s: { set_buffer_config: 't', config: fullCfg({ max_bytes: 222 }) },
      },
    });
    assertEq(setResp.results.s.records[0].config.max_bytes, 222);
    assertEq(setResp.results.s.records[0].repo, 'main');

    const getResp = await client.execute(dbName, {
      id: 'get',
      queries: { g: { get_buffer_config: 't' } },
    });
    assertEq(getResp.results.g.records[0].config.max_bytes, 222);
    assertEq(getResp.results.g.records[0].repo, 'main');
  });
};
