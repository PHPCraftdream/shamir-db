/**
 * Client-side field-name ↔ interner-id cache (Stage 5-wire, Part A).
 *
 * The server interns field names to monotonic `bigint` ids per repo. Ids are
 * append-only and never reused or deleted, so the client can mirror a repo's
 * `(name ↔ id)` mapping without invalidation logic — only fill-missing.
 *
 * Two classes:
 *   - {@link FieldMap}               — one per `(db, repo)`, bidirectional lookup.
 *   - {@link InternerCacheRegistry}  — map of `(db, repo)` → FieldMap.
 *
 * Monotonic invariant: `apply_delta` rejects any delta whose `epoch` is
 * ≤ the local epoch. A delta with a higher epoch is merged unconditionally
 * (ids are stable so re-inserting an existing pair is always a no-op).
 *
 * PLATFORM-AGNOSTIC.
 */

// ── Wire shapes (mirrored from Rust InternerDelta / admin_interner response) ──

/**
 * A per-repo interner delta returned by the server in a `BatchResponse`.
 *
 * - `epoch`   — server's new gap-free high-water id after capturing the delta.
 * - `entries` — `[id, name]` pairs the client does not yet have.
 *
 * Mirrors `shamir-query-types::batch::interner_delta::InternerDelta`.
 */
export interface InternerDelta {
  epoch: bigint;
  /** `[id, name]` pairs — id-first, matching the `interner_dump` shape. */
  entries: [bigint, string][];
}

/**
 * Full interner dump payload from `interner_dump` admin op.
 *
 * Wire shape (server handler `admin_interner.rs`):
 * ```text
 * { "interner_dump": "<repo>", "epoch": <u64>, "entries": [[id, name], ...] }
 * ```
 *
 * `epoch` and `entries[*][0]` arrive as `bigint` from msgpack (the server
 * encodes u64 as msgpack uint64 which @msgpack/msgpack decodes as BigInt
 * when the value > Number.MAX_SAFE_INTEGER, and as number otherwise). We
 * normalise both to `bigint` here for consistency.
 */
export interface InternerDump {
  epoch: bigint;
  entries: [bigint, string][];
}

// ── FieldMap ───────────────────────────────────────────────────────────────────

/**
 * Bidirectional name ↔ id cache for one `(db, repo)`.
 *
 * The cache is monotonic: ids are append-only (the server never reuses or
 * removes an id), so no invalidation is needed. Merging a newer delta simply
 * adds entries.
 *
 * Thread-safety: JavaScript is single-threaded; no locking needed. All
 * mutations are synchronous and instant.
 */
export class FieldMap {
  private readonly nameToId = new Map<string, bigint>();
  private readonly idToName = new Map<bigint, string>();
  private _epoch: bigint = 0n;
  /** True once the first full dump has been applied. */
  private _populated = false;

  /** Current gap-free high-water epoch (highest id the cache has observed). */
  epoch(): bigint {
    return this._epoch;
  }

  /** True once a full `interner_dump` has been applied. */
  isPopulated(): boolean {
    return this._populated;
  }

  /** Number of cached `(name, id)` pairs. */
  size(): number {
    return this.nameToId.size;
  }

  /**
   * Look up the interner id for a field name. Returns `undefined` if not cached.
   *
   * §9.4 guard: `name` is an opaque STRING. `"42"` resolves to the field whose
   * name is the two characters '4' and '2'; it is NEVER parsed into the integer
   * 42n and used as an id directly. Ids come ONLY from server responses.
   */
  getId(name: string): bigint | undefined {
    return this.nameToId.get(name);
  }

  /** Reverse lookup: id → name. Returns `undefined` if not cached. */
  getName(id: bigint): string | undefined {
    return this.idToName.get(id);
  }

  /**
   * Insert one `(name, id)` pair into both directions. Idempotent: re-inserting
   * an existing pair is a no-op. A conflicting re-insert (same name, different id)
   * keeps the FIRST mapping — ids are monotonic + append-only, so a conflict
   * indicates a server contract violation; we surface it by keeping the
   * stable first-seen value.
   *
   * Also CAS-maxes `epoch` against `id`.
   */
  insertEntry(name: string, id: bigint): void {
    if (!this.nameToId.has(name)) {
      this.nameToId.set(name, id);
    }
    if (!this.idToName.has(id)) {
      this.idToName.set(id, name);
    }
    if (id > this._epoch) {
      this._epoch = id;
    }
  }

  /**
   * Apply a full `InternerDump`. Marks the map as populated; CAS-maxes epoch.
   *
   * Idempotent: calling with the same dump twice is safe (first-writer-wins
   * per `insertEntry`).
   */
  applyDump(dump: InternerDump): void {
    for (const [id, name] of dump.entries) {
      const bigId = BigInt(id);
      this.insertEntry(name, bigId);
    }
    const bigEpoch = BigInt(dump.epoch);
    if (bigEpoch > this._epoch) {
      this._epoch = bigEpoch;
    }
    this._populated = true;
  }

  /**
   * Apply an `InternerDelta` from a batch response. Monotonic merge: a delta
   * whose epoch is ≤ the local epoch is silently ignored (the client already
   * has at least as much as the server sent). A newer delta is merged
   * unconditionally.
   *
   * §9.4 invariant: ids enter the cache ONLY via `insertEntry`, which is
   * called from here and from `applyDump`.
   */
  applyDelta(delta: InternerDelta): void {
    const bigEpoch = BigInt(delta.epoch);
    // Monotonic guard: reject stale deltas.
    if (bigEpoch <= this._epoch && delta.entries.length === 0) {
      return;
    }
    for (const [id, name] of delta.entries) {
      this.insertEntry(name, BigInt(id));
    }
    if (bigEpoch > this._epoch) {
      this._epoch = bigEpoch;
    }
  }

  /**
   * Collect the names from `input` that are NOT yet cached.
   *
   * Preserves input order; deduplicates. Returns the candidate set for a
   * single `interner_touch` roundtrip.
   */
  missingNames(input: string[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const name of input) {
      if (!seen.has(name)) {
        seen.add(name);
        if (!this.nameToId.has(name)) {
          out.push(name);
        }
      }
    }
    return out;
  }
}

// ── InternerCacheRegistry ─────────────────────────────────────────────────────

/**
 * Registry of `FieldMap`s keyed by `(db, repo)` strings.
 *
 * `getOrCreate(db, repo)` is the sole entry point; it creates an empty
 * `FieldMap` on first access.
 */
export class InternerCacheRegistry {
  private readonly maps = new Map<string, FieldMap>();

  private static key(db: string, repo: string): string {
    // Simple delimiter-based key. db and repo names are restricted to
    // alphanumeric + underscore on the server side, so '\0' is safe as a
    // separator that cannot appear in either component.
    return `${db}\0${repo}`;
  }

  /**
   * Get the `FieldMap` for `(db, repo)`, creating an empty one on first
   * access. Subsequent calls for the same pair return the SAME instance.
   */
  getOrCreate(db: string, repo: string): FieldMap {
    const k = InternerCacheRegistry.key(db, repo);
    let fm = this.maps.get(k);
    if (fm === undefined) {
      fm = new FieldMap();
      this.maps.set(k, fm);
    }
    return fm;
  }

  /**
   * Snapshot of all `(db, repo)` → epoch pairs currently in the registry.
   * Used to build `BatchRequest.interner_epochs`.
   *
   * Only repos with epoch > 0 are included (an epoch of 0 means the client
   * has nothing cached, so there is no point in advertising it).
   */
  allEpochs(db: string): Record<string, bigint> {
    const prefix = `${db}\0`;
    const result: Record<string, bigint> = {};
    for (const [k, fm] of this.maps) {
      if (k.startsWith(prefix)) {
        const repo = k.slice(prefix.length);
        const e = fm.epoch();
        if (e > 0n) {
          result[repo] = e;
        }
      }
    }
    return result;
  }
}
