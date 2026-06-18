/**
 * Unit tests for FieldMap and InternerCacheRegistry.
 *
 * Tests cover:
 *  - apply_dump idempotence
 *  - apply_delta monotonic merge (rejecting older epoch)
 *  - bidirectional lookup
 *  - missingNames filtering
 *  - InternerCacheRegistry.allEpochs
 */

import { describe, it, expect } from 'vitest';
import { FieldMap, InternerCacheRegistry } from '../field-map.js';
import type { InternerDelta, InternerDump } from '../field-map.js';

// ── FieldMap unit tests ───────────────────────────────────────────────────────

describe('FieldMap', () => {
  describe('insertEntry', () => {
    it('stores name→id and id→name', () => {
      const fm = new FieldMap();
      fm.insertEntry('age', 1n);
      expect(fm.getId('age')).toBe(1n);
      expect(fm.getName(1n)).toBe('age');
    });

    it('advances epoch to the inserted id', () => {
      const fm = new FieldMap();
      fm.insertEntry('x', 5n);
      expect(fm.epoch()).toBe(5n);
      fm.insertEntry('y', 3n);
      // epoch must not regress
      expect(fm.epoch()).toBe(5n);
    });

    it('first-writer-wins on name collision (forward direction)', () => {
      const fm = new FieldMap();
      fm.insertEntry('dup', 10n);
      fm.insertEntry('dup', 99n); // conflict — different id
      // Forward direction: first mapping kept (name → id = 10, not 99).
      expect(fm.getId('dup')).toBe(10n);
      // Reverse for id 10 is correctly 'dup'.
      expect(fm.getName(10n)).toBe('dup');
      // id 99 is a NEW id in the reverse map (idToName had no entry for 99
      // before the second call), so it gets 'dup' as the name. This is a
      // server contract violation scenario — the server should never reassign
      // an id — and the cache simply records both facts and keeps the first
      // name-to-id mapping stable.
      // We do NOT assert on getName(99n) since that's an implementation detail
      // of how the cache handles a server bug; the critical invariant is that
      // getId('dup') remains 10n.
      expect(fm.getId('dup')).toBe(10n);
    });

    it('is idempotent for the same (name, id) pair', () => {
      const fm = new FieldMap();
      fm.insertEntry('foo', 7n);
      fm.insertEntry('foo', 7n);
      expect(fm.size()).toBe(1);
      expect(fm.getId('foo')).toBe(7n);
    });
  });

  describe('applyDump idempotence', () => {
    it('applying the same dump twice yields identical state', () => {
      const fm = new FieldMap();
      const dump: InternerDump = {
        epoch: 3n,
        entries: [
          [1n, 'a'],
          [2n, 'b'],
          [3n, 'c'],
        ],
      };
      fm.applyDump(dump);
      fm.applyDump(dump); // second application — idempotent

      expect(fm.size()).toBe(3);
      expect(fm.getId('a')).toBe(1n);
      expect(fm.getId('b')).toBe(2n);
      expect(fm.getId('c')).toBe(3n);
      expect(fm.epoch()).toBe(3n);
      expect(fm.isPopulated()).toBe(true);
    });

    it('sets populated after a dump', () => {
      const fm = new FieldMap();
      expect(fm.isPopulated()).toBe(false);
      fm.applyDump({ epoch: 0n, entries: [] });
      expect(fm.isPopulated()).toBe(true);
    });

    it('CAS-maxes epoch from the dump field (even above highest entry id)', () => {
      const fm = new FieldMap();
      fm.applyDump({ epoch: 10n, entries: [[5n, 'x']] });
      // epoch from dump.epoch (10) > entry id (5) → epoch must be 10
      expect(fm.epoch()).toBe(10n);
    });
  });

  describe('applyDelta monotonic merge', () => {
    it('merges entries from a newer delta', () => {
      const fm = new FieldMap();
      fm.applyDump({ epoch: 3n, entries: [[1n, 'a'], [2n, 'b'], [3n, 'c']] });

      const delta: InternerDelta = {
        epoch: 5n,
        entries: [[4n, 'd'], [5n, 'e']],
      };
      fm.applyDelta(delta);

      expect(fm.getId('d')).toBe(4n);
      expect(fm.getId('e')).toBe(5n);
      expect(fm.epoch()).toBe(5n);
    });

    it('rejects (silently ignores) a delta with epoch ≤ local epoch and no entries', () => {
      const fm = new FieldMap();
      fm.applyDump({ epoch: 5n, entries: [[1n, 'a']] });

      // Stale delta: epoch 3 < 5, no entries
      fm.applyDelta({ epoch: 3n, entries: [] });

      // Nothing changed
      expect(fm.epoch()).toBe(5n);
      expect(fm.size()).toBe(1);
    });

    it('still merges entries even if delta epoch ≤ local epoch (rare race)', () => {
      const fm = new FieldMap();
      fm.applyDump({ epoch: 5n, entries: [[1n, 'a']] });

      // Delta with epoch ≤ local but HAS entries — merge the entries
      // (monotonic epoch guard only applies when entries is empty).
      fm.applyDelta({ epoch: 4n, entries: [[2n, 'b']] });

      expect(fm.getId('b')).toBe(2n);
      // Epoch must NOT regress
      expect(fm.epoch()).toBe(5n);
    });

    it('empty cache applies delta correctly', () => {
      const fm = new FieldMap();
      fm.applyDelta({ epoch: 2n, entries: [[1n, 'x'], [2n, 'y']] });
      expect(fm.getId('x')).toBe(1n);
      expect(fm.getId('y')).toBe(2n);
      expect(fm.epoch()).toBe(2n);
    });
  });

  describe('bidirectional lookup', () => {
    it('getId returns undefined for uncached names', () => {
      const fm = new FieldMap();
      expect(fm.getId('ghost')).toBeUndefined();
    });

    it('getName returns undefined for uncached ids', () => {
      const fm = new FieldMap();
      expect(fm.getName(42n)).toBeUndefined();
    });

    it('round-trip: name→id→name', () => {
      const fm = new FieldMap();
      fm.insertEntry('score', 7n);
      const id = fm.getId('score')!;
      expect(fm.getName(id)).toBe('score');
    });
  });

  describe('missingNames', () => {
    it('returns all names when cache is empty', () => {
      const fm = new FieldMap();
      expect(fm.missingNames(['a', 'b', 'c'])).toEqual(['a', 'b', 'c']);
    });

    it('returns only names not in cache', () => {
      const fm = new FieldMap();
      fm.insertEntry('known', 1n);
      const missing = fm.missingNames(['known', 'unknown', 'also-missing']);
      expect(missing).toEqual(['unknown', 'also-missing']);
    });

    it('deduplicates the input', () => {
      const fm = new FieldMap();
      const missing = fm.missingNames(['a', 'b', 'a', 'b', 'c']);
      expect(missing).toEqual(['a', 'b', 'c']);
    });

    it('returns empty array when all names are cached', () => {
      const fm = new FieldMap();
      fm.insertEntry('x', 1n);
      fm.insertEntry('y', 2n);
      expect(fm.missingNames(['x', 'y'])).toEqual([]);
    });

    it('preserves input order (minus duplicates)', () => {
      const fm = new FieldMap();
      const missing = fm.missingNames(['c', 'a', 'b']);
      expect(missing).toEqual(['c', 'a', 'b']);
    });
  });
});

// ── InternerCacheRegistry unit tests ─────────────────────────────────────────

describe('InternerCacheRegistry', () => {
  it('getOrCreate returns the same instance on subsequent calls', () => {
    const reg = new InternerCacheRegistry();
    const fm1 = reg.getOrCreate('mydb', 'main');
    const fm2 = reg.getOrCreate('mydb', 'main');
    expect(fm1).toBe(fm2);
  });

  it('getOrCreate returns distinct instances for different repos', () => {
    const reg = new InternerCacheRegistry();
    const fm1 = reg.getOrCreate('mydb', 'main');
    const fm2 = reg.getOrCreate('mydb', 'other');
    expect(fm1).not.toBe(fm2);
  });

  it('getOrCreate returns distinct instances for different dbs', () => {
    const reg = new InternerCacheRegistry();
    const fm1 = reg.getOrCreate('db1', 'main');
    const fm2 = reg.getOrCreate('db2', 'main');
    expect(fm1).not.toBe(fm2);
  });

  describe('allEpochs', () => {
    it('returns empty object when no repos have epoch > 0', () => {
      const reg = new InternerCacheRegistry();
      reg.getOrCreate('mydb', 'main'); // create but leave empty
      expect(reg.allEpochs('mydb')).toEqual({});
    });

    it('returns only repos with epoch > 0 for the given db', () => {
      const reg = new InternerCacheRegistry();
      const fm1 = reg.getOrCreate('mydb', 'main');
      const fm2 = reg.getOrCreate('mydb', 'aux');
      reg.getOrCreate('otherdb', 'main'); // different db, should not appear

      fm1.insertEntry('field', 5n);
      // fm2 stays at epoch 0 → excluded
      // otherdb/main → excluded (wrong db)

      const epochs = reg.allEpochs('mydb');
      expect(Object.keys(epochs)).toEqual(['main']);
      expect(epochs['main']).toBe(5n);
    });

    it('returns multiple repos when both have epoch > 0', () => {
      const reg = new InternerCacheRegistry();
      const fm1 = reg.getOrCreate('mydb', 'main');
      const fm2 = reg.getOrCreate('mydb', 'aux');
      fm1.insertEntry('a', 3n);
      fm2.insertEntry('b', 7n);

      const epochs = reg.allEpochs('mydb');
      expect(epochs['main']).toBe(3n);
      expect(epochs['aux']).toBe(7n);
    });
  });
});
