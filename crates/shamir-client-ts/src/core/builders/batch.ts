/**
 * `Batch` — fluent builder for a {@link BatchRequest}. The CODE that
 * assembles the batch wire object (declared in `../types/batch.ts`).
 * Mirrors `crates/shamir-query-types/src/batch/types.rs`.
 *
 * `.build()` returns a plain object matching the server's serde contract.
 * `.execute(client, db)` sends the batch and returns a typed response.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { WireValue } from '../types/write.js';
import type {
  BatchOpInput,
  QueryEntry,
  IsolationLevel,
  DurabilityLevel,
  BatchLimits,
  BatchRequest,
  BatchResponse,
} from '../types/batch.js';
import type { FilterValue, Filter } from '../types/filter.js';
import type { ExecCtx } from '../exec-ctx.js';
import type { SubscribeSource, SubscribeOpts } from './subscribe.js';
import { subscribe, unsubscribeOp } from './subscribe.js';
import { Handle } from './handle.js';

/** Something that has a `.build()` method returning a wire op. */
interface Buildable {
  build(): BatchOpInput;
}

/** Default Rust-side limits (batch/types.rs `BatchLimits::default`). */
const DEFAULT_LIMITS: BatchLimits = {
  max_queries: 50,
  max_dependency_depth: 10,
  max_execution_time_secs: 30,
  max_result_size: 10_485_760,
  max_nesting_depth: 4,
};

/** Minimal client interface needed by `.execute()`. */
interface BatchClient {
  execute(db: string, batch: object): Promise<BatchResponse>;
  executeWithTouch(db: string, batch: object): Promise<BatchResponse>;
}

/** Fluent builder for a `BatchRequest`. */
export class Batch {
  private readonly idValue: WireValue;
  private nameValue: string | undefined;
  private transactionalValue = false;
  private isolationValue: IsolationLevel | undefined;
  private durabilityValue: DurabilityLevel | undefined;
  private readonly queriesMap: Record<string, QueryEntry> = {};
  private returnAllExplicit: boolean | undefined;
  private returnOnlyValue: string[] | undefined;
  private limitsValue: BatchLimits | undefined;
  private ctxValue: ExecCtx | null = null;

  private constructor(id: WireValue) {
    this.idValue = id;
  }

  /** Create a new batch with the given request `id` (defaults to `1`). */
  static create(id: WireValue = 1): Batch {
    return new Batch(id);
  }

  /**
   * Add an operation under `alias`.
   *
   * `op` may be a raw wire object (`BatchOpInput`) or any builder that
   * has a `.build()` method (e.g. `Query`, `UpdateBuilder`).
   *
   * `opts.returnResult` defaults to `true`; the builder omits
   * `return_result` when true and only emits `return_result: false` when
   * explicitly set to `false`.
   * `opts.after` is emitted only when non-empty.
   */
  add(
    alias: string,
    op: BatchOpInput | Buildable,
    opts?: { returnResult?: boolean; after?: string[] },
  ): this {
    const resolved: BatchOpInput =
      typeof (op as Partial<Buildable>).build === 'function'
        ? (op as Buildable).build()
        : (op as BatchOpInput);

    const entry: QueryEntry = { ...resolved };

    if (opts?.returnResult === false) {
      entry.return_result = false;
    }

    if (opts?.after && opts.after.length > 0) {
      entry.after = opts.after;
    }

    this.queriesMap[alias] = entry;
    return this;
  }

  /**
   * Add a nested sub-batch operation under `alias`.
   *
   * `inner` may be a `Batch` instance (`.build()` is called automatically)
   * or a raw `BatchRequest` object.
   *
   * `opts.bind` maps parameter names to `FilterValue`s that the inner batch
   * can reference via `{ "$param": "name" }`. The `bind` field is omitted
   * when empty or not provided (matches the server's skip-if-empty rule).
   *
   * `opts.returnResult` and `opts.after` behave identically to `.add()`.
   */
  subBatch(
    alias: string,
    inner: Batch | BatchRequest,
    opts?: {
      bind?: Record<string, FilterValue>;
      returnResult?: boolean;
      after?: string[];
    },
  ): this {
    const resolved: BatchRequest =
      typeof (inner as Partial<Batch>).build === 'function'
        ? (inner as Batch).build()
        : (inner as BatchRequest);

    const op: BatchOpInput = { batch: resolved };

    if (opts?.bind && Object.keys(opts.bind).length > 0) {
      (op as { batch: BatchRequest; bind?: Record<string, FilterValue> }).bind =
        opts.bind;
    }

    const entry: QueryEntry = { ...op };

    if (opts?.returnResult === false) {
      entry.return_result = false;
    }

    if (opts?.after && opts.after.length > 0) {
      entry.after = opts.after;
    }

    this.queriesMap[alias] = entry;
    return this;
  }

  /** Optional batch name for logging/debugging. */
  name(n: string): this {
    this.nameValue = n;
    return this;
  }

  /**
   * Enable transactional mode. Optionally set the isolation level.
   * Sets `transactional = true`; `isolation` is omitted when not
   * provided.
   */
  transactional(isolation?: IsolationLevel): this {
    this.transactionalValue = true;
    if (isolation !== undefined) this.isolationValue = isolation;
    return this;
  }

  /** Per-request durability level. */
  durability(level: DurabilityLevel): this {
    this.durabilityValue = level;
    return this;
  }

  /**
   * Override `return_all`. Omitted when true (default); emitted as
   * `false` only when explicitly set to `false`.
   */
  returnAll(b: boolean): this {
    this.returnAllExplicit = b;
    return this;
  }

  /** Specific aliases to return (overrides `return_all`). */
  returnOnly(aliases: string[]): this {
    this.returnOnlyValue = aliases;
    return this;
  }

  /**
   * Set execution limits. Accepts a partial object; missing fields are
   * filled from the Rust defaults.
   */
  limits(partial: Partial<BatchLimits>): this {
    this.limitsValue = {
      max_queries: partial.max_queries ?? DEFAULT_LIMITS.max_queries,
      max_dependency_depth:
        partial.max_dependency_depth ?? DEFAULT_LIMITS.max_dependency_depth,
      max_execution_time_secs:
        partial.max_execution_time_secs ??
        DEFAULT_LIMITS.max_execution_time_secs,
      max_result_size:
        partial.max_result_size ?? DEFAULT_LIMITS.max_result_size,
      max_nesting_depth:
        partial.max_nesting_depth ?? DEFAULT_LIMITS.max_nesting_depth,
    };
    return this;
  }

  /** Assemble the wire `BatchRequest`. */
  build(): BatchRequest {
    const req: BatchRequest = {
      id: this.idValue,
      queries: this.queriesMap,
    };

    if (this.nameValue !== undefined) req.name = this.nameValue;
    if (this.transactionalValue) req.transactional = true;
    if (this.isolationValue !== undefined)
      req.isolation = this.isolationValue;
    if (this.durabilityValue !== undefined)
      req.durability = this.durabilityValue;
    if (this.returnAllExplicit === false) req.return_all = false;
    if (this.returnOnlyValue !== undefined) req.return_only = this.returnOnlyValue;
    if (this.limitsValue !== undefined) req.limits = this.limitsValue;

    return req;
  }

  /**
   * Validated build — assembles the wire `BatchRequest` (like {@link build})
   * and then checks every entry for dangling references:
   *
   *  (a) a `$query` ref whose `alias` is not declared in this batch;
   *  (b) an `after` entry that names an undeclared alias.
   *
   * Throws a descriptive `Error` on the first violation. On success returns
   * the built request (identical to `build()`).
   *
   * `build()` itself remains unchecked for backward compatibility.
   */
  tryBuild(): BatchRequest {
    const req = this.build();
    const declared = new Set(Object.keys(req.queries));

    for (const [alias, entry] of Object.entries(req.queries)) {
      // (a) $query refs anywhere in the entry's filter/value tree.
      for (const ref of collectQueryRefs(entry)) {
        if (!declared.has(ref)) {
          throw new Error(
            `Batch.tryBuild: entry '${alias}' references unknown $query alias '${ref}' ` +
              `(declared: ${[...declared].sort().join(', ') || '∅'})`,
          );
        }
      }

      // (b) `after` dependencies must name declared aliases.
      if (entry.after) {
        for (const dep of entry.after) {
          if (!declared.has(dep)) {
            throw new Error(
              `Batch.tryBuild: entry '${alias}' has after-dependency '${dep}' ` +
                `that is not declared in this batch`,
            );
          }
        }
      }
    }

    return req;
  }

  /**
   * Build and send the batch via `client.executeWithTouch(db, batch)`, which
   * handles smart-write (id-on-wire) transparently on v2 servers and falls
   * back to plain execute on v1.
   */
  execute(client: BatchClient, db: string): Promise<BatchResponse> {
    return client.executeWithTouch(db, this.build());
  }

  /** @internal Bind an execution context (set by `Db.batch()`). */
  bindCtx(ctx: ExecCtx): this {
    this.ctxValue = ctx;
    return this;
  }

  /**
   * Add a subscribe operation under `alias`.
   * Builds a `SubscribeOp` from user-friendly source config(s).
   */
  subscribe(
    alias: string,
    source: SubscribeSource | SubscribeSource[],
    opts?: SubscribeOpts,
  ): this {
    return this.add(alias, subscribe(source, opts));
  }

  /**
   * Add an unsubscribe operation under `alias`.
   */
  unsubscribe(alias: string, subId: number): this {
    return this.add(alias, unsubscribeOp(subId));
  }

  /**
   * Return a typed {@link Handle} for an already-registered `alias`, enabling
   * type-safe `$query` result references (`.column()`, `.row(i)`, `.first()`,
   * `.all()`). Does NOT mutate the batch — it is an accessor only, so
   * `Batch.add()` chaining (which returns `this`) is unaffected.
   *
   * Throws if `alias` has not been declared via `.add()` / `.subBatch()` /
   * `.subscribe()`.
   */
  handle(alias: string): Handle {
    if (!(alias in this.queriesMap)) {
      throw new Error(
        `Batch.handle('${alias}'): alias not registered — add it first via .add('${alias}', …)`,
      );
    }
    return new Handle(alias);
  }

  /** Alias for {@link handle}. */
  ref(alias: string): Handle {
    return this.handle(alias);
  }

  /**
   * Build and send via the bound context. Throws if not bound.
   * Layer-2 counterpart to `execute(client, db)`.
   */
  async run(): Promise<BatchResponse> {
    if (!this.ctxValue) {
      throw new Error(
        'Batch is not bound to a Db; use db.batch() or batch.execute(client, db)',
      );
    }
    return this.ctxValue.exec(this.build());
  }
}

// ── $query-ref collection (for tryBuild validation) ───────────────────
//
// These helpers recursively walk a built `QueryEntry` looking for
// `{ $query: alias, path? }` values. They cover every position a
// `FilterValue` can appear: filter leaves (`value`, `values[]`, `from`,
// `to`), composite filters (`and`/`or`/`not`), and value composites
// (`$fn.args`, `$expr.args`, `$cond.then`/`$cond.else`, `$param`→none,
// plain arrays, and `SubBatchOp.bind` maps).

/** Recursive collector: yields every `$query` alias referenced in `fv`. */
function collectFromFilterValue(fv: FilterValue, out: string[]): void {
  if (fv === null || fv === undefined) return;
  if (typeof fv === 'boolean' || typeof fv === 'number' || typeof fv === 'string') {
    return;
  }
  if (fv instanceof Uint8Array) return;
  if (Array.isArray(fv)) {
    for (const item of fv) collectFromFilterValue(item, out);
    return;
  }
  // Object-shaped FilterValue.
  const obj = fv as Record<string, unknown>;
  if ('$query' in obj && typeof obj.$query === 'string') {
    out.push(obj.$query as string);
    return;
  }
  if ('$ref' in obj) return; // field ref — no nested query alias.
  if ('$param' in obj) return; // param name — not a query alias.
  if ('$fn' in obj) {
    const fn = obj.$fn;
    if (typeof fn === 'string') return; // FnCall::Simple — no args.
    if (fn && typeof fn === 'object') {
      const args = (fn as { args?: unknown[] }).args;
      if (Array.isArray(args)) for (const a of args) collectFromFilterValue(a as FilterValue, out);
    }
    return;
  }
  if ('$expr' in obj) {
    const args = (obj.$expr as { args?: unknown[] }).args;
    if (Array.isArray(args)) for (const a of args) collectFromFilterValue(a as FilterValue, out);
    return;
  }
  if ('$cond' in obj) {
    const c = obj.$cond as { if?: unknown; then?: unknown; else?: unknown };
    if (c.if) collectFromFilter(c.if as Filter, out);
    if (c.then !== undefined) collectFromFilterValue(c.then as FilterValue, out);
    if (c.else !== undefined) collectFromFilterValue(c.else as FilterValue, out);
    return;
  }
}

/** Recursive collector over a `Filter` tree. */
function collectFromFilter(f: Filter, out: string[]): void {
  switch (f.op) {
    case 'and':
    case 'or':
      for (const sub of f.filters) collectFromFilter(sub, out);
      return;
    case 'not':
      collectFromFilter(f.filter, out);
      return;
    case 'eq':
    case 'ne':
    case 'gt':
    case 'gte':
    case 'lt':
    case 'lte':
    case 'field':
    case 'contains':
      collectFromFilterValue(f.value, out);
      return;
    case 'in':
    case 'not_in':
    case 'contains_any':
    case 'contains_all':
      for (const v of f.values) collectFromFilterValue(v, out);
      return;
    case 'between':
      collectFromFilterValue(f.from, out);
      collectFromFilterValue(f.to, out);
      return;
    case 'computed':
      collectFromFilterValue(f.value, out);
      if (f.expr_args) for (const v of f.expr_args) collectFromFilterValue(v, out);
      return;
    // leaf filters with no FilterValue positions.
    case 'like':
    case 'i_like':
    case 'regex':
    case 'is_null':
    case 'is_not_null':
    case 'exists':
    case 'not_exists':
    case 'fts':
    case 'vector_similarity':
      return;
    default: {
      // Exhaustiveness guard — if a new variant appears, this fails at compile
      // time when the switch is tightened; at runtime we no-op.
      const _exhaustive: never = f;
      void _exhaustive;
      return;
    }
  }
}

/**
 * Collect every `$query` alias referenced inside a built `QueryEntry`.
 * Covers: top-level `ReadQuery.where`, `SubBatchOp` (bind map + nested
 * queries), and `SubscribeOp.deliver.batch.bind`. Recurses into nested
 * sub-batches so deeply-nested refs are caught.
 */
function collectQueryRefs(entry: QueryEntry): string[] {
  const out: string[] = [];
  collectFromEntry(entry as Record<string, unknown>, out);
  return out;
}

/** Recursive walker over a single entry object. */
function collectFromEntry(e: Record<string, unknown>, out: string[]): void {
  // ReadQuery.where
  if (e.where && typeof e.where === 'object') {
    collectFromFilter(e.where as Filter, out);
  }

  // SubBatchOp: { batch: BatchRequest, bind?: Record<string, FilterValue> }
  // — `bind` lives on the entry itself, not inside `batch`.
  if (e.batch && typeof e.batch === 'object') {
    // bind map on this entry.
    if (e.bind && typeof e.bind === 'object') {
      for (const v of Object.values(e.bind as Record<string, unknown>)) {
        collectFromFilterValue(v as FilterValue, out);
      }
    }
    // recurse into the inner batch's queries.
    const sub = e.batch as { queries?: Record<string, unknown> };
    if (sub.queries) {
      for (const inner of Object.values(sub.queries)) {
        if (inner && typeof inner === 'object') {
          collectFromEntry(inner as Record<string, unknown>, out);
        }
      }
    }
  }

  // DeliverMode.batch (inside SubscribeOp.deliver) — shape:
  //   { batch: { batch: BatchRequest, bind?: Record<string, FilterValue> } }
  if (e.deliver && typeof e.deliver === 'object') {
    const d = e.deliver as { batch?: { bind?: Record<string, unknown> } };
    if (d.batch?.bind) {
      for (const v of Object.values(d.batch.bind)) {
        collectFromFilterValue(v as FilterValue, out);
      }
    }
  }
}
