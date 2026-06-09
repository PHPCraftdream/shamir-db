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

import type { Json } from '../types/write.js';
import type {
  BatchOpInput,
  QueryEntry,
  IsolationLevel,
  DurabilityLevel,
  BatchLimits,
  BatchRequest,
  BatchResponse,
} from '../types/batch.js';
import type { FilterValue } from '../types/filter.js';
import type { ExecCtx } from '../exec-ctx.js';

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
};

/** Minimal client interface needed by `.execute()`. */
interface BatchClient {
  execute(db: string, batch: object): Promise<BatchResponse>;
}

/** Fluent builder for a `BatchRequest`. */
export class Batch {
  private readonly idValue: Json;
  private nameValue: string | undefined;
  private transactionalValue = false;
  private isolationValue: IsolationLevel | undefined;
  private durabilityValue: DurabilityLevel | undefined;
  private readonly queriesMap: Record<string, QueryEntry> = {};
  private returnAllExplicit: boolean | undefined;
  private returnOnlyValue: string[] | undefined;
  private limitsValue: BatchLimits | undefined;
  private ctxValue: ExecCtx | null = null;

  private constructor(id: Json) {
    this.idValue = id;
  }

  /** Create a new batch with the given request `id` (defaults to `1`). */
  static create(id: Json = 1): Batch {
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
    this.isolationValue = isolation;
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
   * Build and send the batch via `client.execute(db, batch)`, which unwraps
   * the `DbResponse::Batch` envelope and returns the {@link BatchResponse}.
   */
  execute(client: BatchClient, db: string): Promise<BatchResponse> {
    return client.execute(db, this.build());
  }

  /** @internal Bind an execution context (set by `Db.batch()`). */
  bindCtx(ctx: ExecCtx): this {
    this.ctxValue = ctx;
    return this;
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
