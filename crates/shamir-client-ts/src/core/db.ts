/**
 * `Db` — Layer-2 bound database handle. Created via `client.db('my_app')`.
 *
 * Captures the client + database name so callers never re-thread the
 * connection. Provides convenience methods (`run`, `query`, `batch`,
 * HMAC-gated DDL wrappers) over the pure Layer-1 builders.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ShamirClient } from './client.js';
import type { BatchResponse, QueryResult } from './types/batch.js';
import type { Json } from './types/write.js';
import type { ExecCtx } from './exec-ctx.js';
import { Batch } from './builders/batch.js';
import { Query } from './builders/query.js';
import * as ddl from './builders/ddl.js';

/** Something that has a `.build()` method returning a wire op. */
interface Buildable {
  build(): object;
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
    const resolved: object =
      typeof op === 'object' && op !== null && 'build' in op
        ? (op as Buildable).build()
        : op;
    const batch = { id: 1, queries: { _: resolved } };
    const resp: BatchResponse = await this.ctx.exec(batch);
    return resp.results['_']!;
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
