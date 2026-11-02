import { Batch } from '../core/builders/write/batch.js';
import { Query } from '../core/builders/read/query.js';
import type { BatchOpInput, BatchRequest, BatchResponse, QueryResult } from '../core/types/write/batch.js';
import type { WireValue } from '../core/types/write/write.js';
import type { TokenHttpClient } from './token-client.js';
import type { VisibilityState, VisibilityWait } from '../core/visibility/types.js';

interface Buildable { build(): object; }

export class TokenHttpDb {
  constructor(private readonly client: TokenHttpClient, readonly name: string) {}

  async run(op: object): Promise<QueryResult> {
    const built = typeof (op as Partial<Buildable>).build === 'function' ? (op as Buildable).build() : op;
    const batch = Batch.create().add('_', built as BatchOpInput).build();
    const response = await this.client.execute(this.name, batch);
    const result = response.results['_'];
    if (result === undefined) throw new Error("server returned no result for operation alias '_'");
    return result;
  }

  async rows(op: object): Promise<Array<Record<string, WireValue>>> { return (await this.run(op)).records; }
  query(table: string): Query { return Query.from(table).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  withRepo(repo: string, table: string): Query { return Query.withRepo(repo, table).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  batch(id?: WireValue): Batch { return Batch.create(id).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  execute(batch: BatchRequest): Promise<BatchResponse> { return this.client.execute(this.name, batch); }
  waitForVisibility(repo: string, version: bigint, wait: VisibilityWait): Promise<VisibilityState> { return this.client.waitForVisibility(this.name, repo, version, wait); }
  getDdlOpStatus(repo: string, table: string, opId: string) { return this.client.getDdlOpStatus(this.name, repo, table, opId); }
}
