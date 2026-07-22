/**
 * Cursor request-builder wire-shape tests (FG-5a).
 */

import { describe, it, expect } from 'vitest';
import { createCursor, fetchNext, cancelCursor } from '../cursor.js';
import { Batch } from '../batch.js';
import { Query } from '../query.js';

describe('createCursor', () => {
  it('builds the expected DbRequest::CreateCursor wire shape from a Query builder', () => {
    const req = createCursor('app', Query.from('users'), 50);
    expect(req).toEqual({
      op: 'create_cursor',
      db: 'app',
      query: { from: 'users' },
      page_size: 50,
    });
  });

  it('accepts a raw ReadQuery object unchanged', () => {
    const req = createCursor('app', { from: 'users' }, 25);
    expect(req).toEqual({
      op: 'create_cursor',
      db: 'app',
      query: { from: 'users' },
      page_size: 25,
    });
  });
});

describe('fetchNext', () => {
  it('builds the expected DbRequest::FetchNext wire shape', () => {
    const req = fetchNext(7, 25);
    expect(req).toEqual({
      op: 'fetch_next',
      cursor_id: 7,
      page_size: 25,
    });
  });
});

describe('cancelCursor', () => {
  it('builds the expected DbRequest::CancelCursor wire shape', () => {
    const req = cancelCursor(9);
    expect(req).toEqual({
      op: 'cancel_cursor',
      cursor_id: 9,
    });
  });
});

describe('Batch static cursor helpers', () => {
  it('Batch.createCursor forwards to the createCursor builder', () => {
    const req = Batch.createCursor('app', Query.from('users'), 50);
    expect(req).toEqual({
      op: 'create_cursor',
      db: 'app',
      query: { from: 'users' },
      page_size: 50,
    });
  });

  it('Batch.fetchNext forwards to the fetchNext builder', () => {
    const req = Batch.fetchNext(7, 25);
    expect(req).toEqual({
      op: 'fetch_next',
      cursor_id: 7,
      page_size: 25,
    });
  });

  it('Batch.cancelCursor forwards to the cancelCursor builder', () => {
    const req = Batch.cancelCursor(9);
    expect(req).toEqual({
      op: 'cancel_cursor',
      cursor_id: 9,
    });
  });
});
