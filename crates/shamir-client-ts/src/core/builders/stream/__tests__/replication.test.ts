/**
 * Replication-builder wire-shape tests.
 *
 * The authority for every shape is
 * `crates/shamir-query-types/src/admin/types/repl_ops.rs` (serde:
 * skip_serializing_if, default, rename_all, externally-tagged enum) and the
 * Rust builders in
 * `crates/shamir-query-builder/src/ddl/replication.rs`.
 *
 * Each test asserts deep equality (`toEqual`) with the exact expected wire
 * object — verifying snake_case keys, omission of optional keys, and the
 * three SubAction forms. Cross-language byte-parity is exercised by the
 * separate fixture test (#375/#376); here we pin the TS-side shapes.
 */

import { describe, it, expect } from 'vitest';
import { replication } from '../replication.js';
import {
  replScope,
  replStream,
  replicationProfile,
  dropReplicationProfile,
  publication,
  dropPublication,
  subscription,
  dropSubscription,
  alterSubscription,
  listPublications,
  listSubscriptions,
  replicationStatus,
} from '../replication.js';

// ── replScope helper ────────────────────────────────────────────────

describe('replScope', () => {
  it('db-only — omits repo/table keys (skip_serializing_if = Option::is_none)', () => {
    const scope = replScope('app');
    expect(scope).toEqual({ db: 'app' });
    expect(scope).not.toHaveProperty('repo');
    expect(scope).not.toHaveProperty('table');
  });

  it('db + repo — omits table only', () => {
    const scope = replScope('app', { repo: 'main' });
    expect(scope).toEqual({ db: 'app', repo: 'main' });
    expect(scope).not.toHaveProperty('table');
  });

  it('db + repo + table — all three keys present', () => {
    const scope = replScope('app', { repo: 'edge_42', table: 'users' });
    expect(scope).toEqual({ db: 'app', repo: 'edge_42', table: 'users' });
  });

  it('via namespace alias', () => {
    expect(replication.replScope('system')).toEqual({ db: 'system' });
  });
});

// ── replStream helper ───────────────────────────────────────────────

describe('replStream', () => {
  it('defaults direction=pull, mode=read_only (serde default, ALWAYS on wire)', () => {
    const stream = replStream(replScope('app'));
    expect(stream).toEqual({
      scope: { db: 'app' },
      direction: 'pull',
      mode: 'read_only',
    });
  });

  it('explicit direction=push, mode=read_write', () => {
    const stream = replStream(
      { db: 'app', repo: 'main', table: 'users' },
      'push',
      'read_write',
    );
    expect(stream).toEqual({
      scope: { db: 'app', repo: 'main', table: 'users' },
      direction: 'push',
      mode: 'read_write',
    });
  });

  it('direction=both (R4/CRDT)', () => {
    const stream = replStream({ db: 'app' }, 'both');
    expect(stream.direction).toBe('both');
  });
});

// ── replicationProfile / dropReplicationProfile ─────────────────────

describe('replicationProfile', () => {
  it('single stream', () => {
    const op = replicationProfile('cluster', [
      replStream(replScope('app')),
    ]);
    expect(op).toEqual({
      create_replication_profile: 'cluster',
      streams: [
        { scope: { db: 'app' }, direction: 'pull', mode: 'read_only' },
      ],
    });
  });

  it('multiple streams with mixed directions/modes', () => {
    const op = replicationProfile('edge', [
      replStream(replScope('app', { repo: 'main' }), 'pull', 'read_only'),
      replStream(
        replScope('app', { repo: 'edge_42', table: 'sensors' }),
        'push',
        'read_write',
      ),
      replStream(replScope('system'), 'both'),
    ]);
    expect(op).toEqual({
      create_replication_profile: 'edge',
      streams: [
        {
          scope: { db: 'app', repo: 'main' },
          direction: 'pull',
          mode: 'read_only',
        },
        {
          scope: { db: 'app', repo: 'edge_42', table: 'sensors' },
          direction: 'push',
          mode: 'read_write',
        },
        {
          scope: { db: 'system' },
          direction: 'both',
          mode: 'read_only',
        },
      ],
    });
  });

  it('empty streams array is preserved on wire (Vec, no skip)', () => {
    const op = replicationProfile('empty', []);
    expect(op).toEqual({
      create_replication_profile: 'empty',
      streams: [],
    });
  });
});

describe('dropReplicationProfile', () => {
  it('emits {drop_replication_profile: name}', () => {
    expect(dropReplicationProfile('cluster')).toEqual({
      drop_replication_profile: 'cluster',
    });
  });
});

// ── publication / dropPublication ───────────────────────────────────

describe('publication', () => {
  it('single scope', () => {
    const op = publication('pub_main', [replScope('app')]);
    expect(op).toEqual({
      create_publication: 'pub_main',
      scopes: [{ db: 'app' }],
    });
  });

  it('multiple scopes with varying granularity', () => {
    const op = publication('pub_all', [
      replScope('app'),
      replScope('app', { repo: 'main' }),
      replScope('system', { repo: 'auth', table: 'users' }),
    ]);
    expect(op).toEqual({
      create_publication: 'pub_all',
      scopes: [
        { db: 'app' },
        { db: 'app', repo: 'main' },
        { db: 'system', repo: 'auth', table: 'users' },
      ],
    });
  });
});

describe('dropPublication', () => {
  it('emits {drop_publication: name}', () => {
    expect(dropPublication('pub_main')).toEqual({
      drop_publication: 'pub_main',
    });
  });
});

// ── subscription / dropSubscription ─────────────────────────────────

describe('subscription', () => {
  it('all four fields present', () => {
    const op = subscription('sub1', {
      upstream: 'leader://node-1',
      publication: 'pub_main',
      profile: 'cluster',
    });
    expect(op).toEqual({
      create_subscription: 'sub1',
      upstream: 'leader://node-1',
      publication: 'pub_main',
      profile: 'cluster',
    });
  });
});

describe('dropSubscription', () => {
  it('emits {drop_subscription: name}', () => {
    expect(dropSubscription('sub1')).toEqual({
      drop_subscription: 'sub1',
    });
  });
});

// ── alterSubscription (SubAction externally-tagged enum) ────────────

describe('alterSubscription', () => {
  it('pause action — bare string', () => {
    expect(alterSubscription('sub1', 'pause')).toEqual({
      alter_subscription: 'sub1',
      action: 'pause',
    });
  });

  it('resume action — bare string', () => {
    expect(alterSubscription('sub1', 'resume')).toEqual({
      alter_subscription: 'sub1',
      action: 'resume',
    });
  });

  it('set_profile action — {set_profile: name} object', () => {
    expect(alterSubscription('sub1', { set_profile: 'cluster2' })).toEqual({
      alter_subscription: 'sub1',
      action: { set_profile: 'cluster2' },
    });
  });
});

// ── read-only introspection ops (presence-only boolean = true) ──────

describe('read-only introspection ops', () => {
  it('listPublications → {list_publications: true}', () => {
    expect(listPublications()).toEqual({ list_publications: true });
  });

  it('listSubscriptions → {list_subscriptions: true}', () => {
    expect(listSubscriptions()).toEqual({ list_subscriptions: true });
  });

  it('replicationStatus → {replication_status: true}', () => {
    expect(replicationStatus()).toEqual({ replication_status: true });
  });
});

// ── namespace object surface ────────────────────────────────────────

describe('replication namespace', () => {
  it('exposes every constructor as a property', () => {
    expect(replication.replScope).toBe(replScope);
    expect(replication.replStream).toBe(replStream);
    expect(replication.replicationProfile).toBe(replicationProfile);
    expect(replication.dropReplicationProfile).toBe(dropReplicationProfile);
    expect(replication.publication).toBe(publication);
    expect(replication.dropPublication).toBe(dropPublication);
    expect(replication.subscription).toBe(subscription);
    expect(replication.dropSubscription).toBe(dropSubscription);
    expect(replication.alterSubscription).toBe(alterSubscription);
    expect(replication.listPublications).toBe(listPublications);
    expect(replication.listSubscriptions).toBe(listSubscriptions);
    expect(replication.replicationStatus).toBe(replicationStatus);
  });

  it('composes a full profile + subscription workflow end-to-end', () => {
    const profile = replication.replicationProfile('cluster', [
      replication.replStream(replication.replScope('app')),
    ]);
    const sub = replication.subscription('sub1', {
      upstream: 'leader://1',
      publication: 'pub',
      profile: 'cluster',
    });
    const alter = replication.alterSubscription('sub1', 'pause');
    expect(profile.streams).toHaveLength(1);
    expect(sub.profile).toBe('cluster');
    expect(alter.action).toBe('pause');
  });
});
