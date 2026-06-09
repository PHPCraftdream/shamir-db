/**
 * `Db` / `Tx` — Layer-2 bound database handle and interactive-transaction
 * wrapper. Created via `client.db('my_app')`.
 *
 * Captures the client + database name so callers never re-thread the
 * connection. Provides convenience methods (`run`, `query`, `batch`,
 * HMAC-gated DDL wrappers) over the pure Layer-1 builders.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ShamirClient } from './client.js';
import type { BatchResponse, QueryResult, TransactionInfo } from './types/batch.js';
import type { Json } from './types/write.js';
import type { ExecCtx } from './exec-ctx.js';
import { Batch } from './builders/batch.js';
import { Query } from './builders/query.js';
import * as ddl from './builders/ddl.js';

/** Something that has a `.build()` method returning a wire op. */
interface Buildable {
  build(): object;
}

/** Execute a single op through an `ExecCtx`, returning the unwrapped `QueryResult`. */
async function runOne(ctx: ExecCtx, op: object): Promise<QueryResult> {
  const resolved: object =
    typeof op === 'object' && op !== null && 'build' in op
      ? (op as Buildable).build()
      : op;
  const batch = { id: 1, queries: { _: resolved } };
  const resp: BatchResponse = await ctx.exec(batch);
  return resp.results['_']!;
}

export class Db {
  constructor(
    private readonly client: ShamirClient,
    readonly name: string,
  ) {}

  private get ctx(): ExecCtx {
    return { exec: (b) => this.client.execute(this.name, b) };
  }

  /**
   * Execute a single operation and return the unwrapped `QueryResult`.
   *
   * Accepts a raw wire op (`BatchOpInput`), a builder with `.build()`
   * (e.g. `Query`, `UpdateBuilder`), or a bound `Query` (already has
   * `.build()`). Wraps as `{ id: 1, queries: { _: <op> } }`.
   */
  async run(op: object): Promise<QueryResult> {
    return runOne(this.ctx, op);
  }

  /** Shortcut: run a single op and return just the `.records` array. */
  async rows(op: object): Promise<Array<Record<string, Json>>> {
    return (await this.run(op)).records;
  }

  /** Create a bound `Query` targeting `table` in the default repo. */
  query(table: string): Query {
    return Query.from(table).bindCtx(this.ctx);
  }

  /** Create a bound `Query` targeting `repo.table`. */
  withRepo(repo: string, table: string): Query {
    return Query.withRepo(repo, table).bindCtx(this.ctx);
  }

  /** Create a bound `Batch`. */
  batch(id?: Json): Batch {
    return Batch.create(id).bindCtx(this.ctx);
  }

  /**
   * Execute an auto-managed interactive transaction. The callback receives
   * a `Tx` handle whose operations route through `txExecute`. If the
   * callback resolves, the transaction is committed; if it throws (or the
   * commit reports `status === 'aborted'`), the transaction is rolled back
   * and the error rethrown.
   */
  async tx<T>(
    fn: (t: Tx) => Promise<T>,
    opts?: { repo?: string; isolation?: 'snapshot' | 'serializable' },
  ): Promise<T> {
    const opened = await this.client.txBegin(
      this.name,
      opts?.repo ?? 'main',
      opts?.isolation,
    );
    let committed = false;
    try {
      const out = await fn(new Tx(this.client, this.name, opened.tx_handle));
      const info = await this.client.txCommit(this.name, opened.tx_handle);
      // The commit attempt FINALISES the tx server-side (committed OR aborted) —
      // either way the handle is gone, so no rollback is needed past this point.
      committed = true;
      if (info.status === 'aborted') {
        throw new Error(`transaction aborted: ${info.reason ?? 'unknown'}`);
      }
      return out;
    } finally {
      // Rollback only when we never reached a finalising commit — i.e. `fn`
      // threw, or `txCommit` itself failed (transport error, tx may still be open).
      if (!committed) {
        await this.client.txRollback(this.name, opened.tx_handle).catch(() => {});
      }
    }
  }

  // ── HMAC convenience wrappers (client IS the signer; dbInUse = this.name) ─

  /** Drop a table (HMAC-signed via the bound client). */
  dropTable(repo: string, table: string): Promise<QueryResult> {
    return this.run(ddl.dropTable(this.client, this.name, repo, table));
  }

  /** Drop an index (HMAC-signed via the bound client). */
  dropIndex(
    repo: string,
    table: string,
    index: string,
    opts?: { unique?: boolean },
  ): Promise<QueryResult> {
    return this.run(ddl.dropIndex(this.client, this.name, repo, table, index, opts));
  }

  /** Drop a repository (HMAC-signed via the bound client). */
  dropRepo(repo: string, opts?: { cascade?: boolean }): Promise<QueryResult> {
    return this.run(ddl.dropRepo(this.client, this.name, repo, opts));
  }

  /** Drop this database (HMAC-signed via the bound client). */
  dropDb(opts?: { cascade?: boolean }): Promise<QueryResult> {
    return this.run(ddl.dropDb(this.client, this.name, opts));
  }

  /** Start an online table migration (HMAC-signed). */
  startMigration(
    srcRepo: string,
    table: string,
    dstRepo: string,
    dstEngine: string,
    opts?: { dst_path?: string },
  ): Promise<QueryResult> {
    return this.run(
      ddl.startMigration(this.client, this.name, srcRepo, table, dstRepo, dstEngine, opts),
    );
  }

  /** Commit a running migration (HMAC-signed). */
  commitMigration(migrationId: string): Promise<QueryResult> {
    return this.run(ddl.commitMigration(this.client, this.name, migrationId));
  }

  /** Rollback a running migration (HMAC-signed). */
  rollbackMigration(migrationId: string): Promise<QueryResult> {
    return this.run(ddl.rollbackMigration(this.client, this.name, migrationId));
  }
}

/**
 * `Tx` — a scoped interactive-transaction handle created by `Db.tx()`.
 *
 * Mirrors the data-operation subset of `Db` (`run`, `rows`, `query`,
 * `withRepo`, `batch`) but routes through `txExecute` instead of
 * `execute`. No HMAC/DDL wrappers — DDL inside a transaction is out of
 * scope.
 */
export class Tx {
  constructor(
    private readonly client: ShamirClient,
    private readonly db: string,
    private readonly handle: number,
  ) {}

  private get ctx(): ExecCtx {
    return { exec: (b) => this.client.txExecute(this.db, this.handle, b) };
  }

  /** Execute a single op inside the transaction. */
  async run(op: object): Promise<QueryResult> {
    return runOne(this.ctx, op);
  }

  /** Shortcut: run a single op and return just the `.records` array. */
  async rows(op: object): Promise<Array<Record<string, Json>>> {
    return (await this.run(op)).records;
  }

  /** Create a bound `Query` targeting `table` in the default repo. */
  query(table: string): Query {
    return Query.from(table).bindCtx(this.ctx);
  }

  /** Create a bound `Query` targeting `repo.table`. */
  withRepo(repo: string, table: string): Query {
    return Query.withRepo(repo, table).bindCtx(this.ctx);
  }

  /** Create a bound `Batch` inside the transaction. */
  batch(id?: Json): Batch {
    return Batch.create(id).bindCtx(this.ctx);
  }
}
