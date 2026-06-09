/**
 * Call-builder wire-shape tests.
 *
 * The authority for every shape is `crates/shamir-query-types/src/call/mod.rs`
 * (serde: default = "main" for repo, skip_serializing_if = "Vec::is_empty"
 * for params).
 */

import { describe, it, expect } from 'vitest';
import { call } from '../call.js';

describe('call', () => {
  it('name-only → repo always present, no params key', () => {
    const op = call('my_fn');
    expect(op).toEqual({
      call: 'my_fn',
      repo: 'main',
    });
    expect(op).not.toHaveProperty('params');
  });

  it('with params → includes params array', () => {
    const op = call('my_fn', [1, 'x', true]);
    expect(op).toEqual({
      call: 'my_fn',
      params: [1, 'x', true],
      repo: 'main',
    });
  });

  it('empty params + custom repo → params omitted, repo emitted', () => {
    const op = call('my_fn', [], { repo: 'hot' });
    expect(op).toEqual({
      call: 'my_fn',
      repo: 'hot',
    });
    expect(op).not.toHaveProperty('params');
  });

  it('custom repo + params → both emitted', () => {
    const op = call('my_fn', [42], { repo: 'analytics' });
    expect(op).toEqual({
      call: 'my_fn',
      params: [42],
      repo: 'analytics',
    });
  });

  it('$ref FilterValue param round-trips in params', () => {
    const op = call('my_fn', [{ $ref: ['result1', 'field'] }]);
    expect(op).toEqual({
      call: 'my_fn',
      params: [{ $ref: ['result1', 'field'] }],
      repo: 'main',
    });
  });

  it('$query FilterValue param round-trips in params', () => {
    const op = call('my_fn', [{ $query: 'alias1', path: 'name' }]);
    expect(op).toEqual({
      call: 'my_fn',
      params: [{ $query: 'alias1', path: 'name' }],
      repo: 'main',
    });
  });

  it('repo "main" explicit → still "main" string', () => {
    const op = call('my_fn', undefined, { repo: 'main' });
    expect(op.repo).toBe('main');
    expect(op).not.toHaveProperty('params');
  });

  it('undefined params → no params key', () => {
    const op = call('my_fn', undefined);
    expect(op).not.toHaveProperty('params');
  });
});
