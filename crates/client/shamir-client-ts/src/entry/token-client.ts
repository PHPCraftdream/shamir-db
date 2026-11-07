import type { TokenHttpConnectOptions } from '../core/types/connection.js';
import { validateTokenBatchResponse } from './token-response.js';
import type { BatchRequest, BatchResponse, DdlOpStatus } from '../core/types/write/batch.js';
import type { VisibilityState, VisibilityWait } from '../core/visibility/types.js';
import { normalizeVisibility, position } from '../core/visibility/positions.js';
import { normalizeTokenHttpOptions, type NormalizedTokenHttpOptions } from '../core/protocol/token/options.js';
import { TokenHttpTransport } from '../core/protocol/token/http.js';
import type { TokenResponseMap } from '../core/protocol/token/transport.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';
import type { TokenPlatform } from '../platform/token/types.js';
import { TokenHttpDb } from './token-db.js';
import { normalizeTokenDdlState } from './token-ddl-state.js';

export interface TokenHttpClient {
  readonly channel: 'http'; readonly sid: undefined; readonly expiresAtNs: undefined;
  ping(): Promise<{ kind: 'pong' }>;
  execute(db: string, batch: object, visibilityWait?: VisibilityWait): Promise<BatchResponse>;
  executeWithTouch(db: string, batch: object): Promise<BatchResponse>;
  waitForVisibility(db: string, repo: string, version: bigint, wait: VisibilityWait): Promise<VisibilityState>;
  getDdlOpStatus(db: string, repo: string, table: string, opId: string): Promise<DdlOpStatus | null>;
  db(name: string): TokenHttpDb;
  close(): Promise<void>;
}
export type TokenPlatformFactory<T extends TokenHttpConnectOptions = TokenHttpConnectOptions> = (options: T) => TokenPlatform;
function isRecord(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
function deny(code: 'token_request_denied' | 'not_supported_over_rest'): never { throw new TokenTransportError(code, 'Token operation denied'); }
const ALLOWED = new Set(['from','insert_into','update','set','delete_from','call','batch','for_each','list','access_tree','describe_table','get_table_schema','subscribe','unsubscribe']);
const UNSUPPORTED = new Set(['create_cursor','fetch_next','cancel_cursor','tx_begin','tx_execute','tx_commit','tx_rollback']);
const DENIED = new Set(['create_db','drop_db','create_repo','drop_repo','rename_repo','alter_repo','rename_db','create_table','drop_table','rename_table','create_index','drop_index','rename_index','set_buffer_config','get_buffer_config','alter_buffer_config','start_migration','commit_migration','rollback_migration','migration_status','create_user','drop_user','grant_role','revoke_role','chmod','chown','chgrp','create_group','drop_group','rename_group','add_group_member','remove_group_member','create_function','drop_function','rename_function','describe_function','export_function','check_function','alter_function','create_validator','drop_validator','rename_validator','bind_validator','unbind_validator','list_validators','set_table_schema','add_schema_rule','remove_schema_rule','describe_repo','create_function_folder','rename_function_folder','interner_dump','interner_touch','purge_history','set_retention','changes_since','create_replication_profile','drop_replication_profile','create_publication','drop_publication','create_subscription','drop_subscription','alter_subscription','list_publications','list_subscriptions','replication_status','admin','admin_op']);
const INTRO = new Set(['list','access_tree','describe_table','get_table_schema']);
const FIELDS: Record<string, string[]> = {
  from: ['from','select','where','group_by','order_by','pagination','count_total','temporal','with_version','explain'],
  insert_into: ['insert_into','values','select'], update: ['update','set','where','select','expected_version'],
  set: ['set','key','value'], delete_from: ['delete_from','where','select','expected_version'],
  call: ['call','params','repo'], batch: ['batch','bind'], for_each: ['for_each','over','bind_row'],
  list: ['list','repo','table','folder','parent'], access_tree: ['access_tree','depth','db'],
  describe_table: ['describe_table','repo'], get_table_schema: ['get_table_schema','repo'],
  subscribe: ['subscribe', 'deliver', 'initial', 'from_version'], unsubscribe: ['unsubscribe'],
};
const BATCH_FIELDS = new Set(['id','name','transactional','isolation','visibility_wait','durability','index_visibility','queries','return_all','return_only','limits','result_encoding']);
function classify(op: Record<string, unknown>, allowSubscriptions: boolean): string {
  const allKeys = Object.keys(op); const metadata = new Set(['return_result','after','when']); const keys = allKeys.filter((key) => !metadata.has(key));
  if (allKeys.some((key) => key === 'return_result' && typeof op[key] !== 'boolean')) deny('token_request_denied');
  const matches = keys.filter((key) => ALLOWED.has(key) || UNSUPPORTED.has(key) || DENIED.has(key));
  if (keys.some((key) => DENIED.has(key))) deny('token_request_denied');
  if (keys.some((key) => UNSUPPORTED.has(key))) deny('not_supported_over_rest');
  if (keys.includes('subscribe') || keys.includes('unsubscribe')) {
    if (!allowSubscriptions) deny('not_supported_over_rest');
    if (matches.length !== 1 || allKeys.some((key) => !metadata.has(key) && !FIELDS[keys.includes('subscribe') ? 'subscribe' : 'unsubscribe']!.includes(key))) deny('token_request_denied');
    return keys.includes('subscribe') ? 'subscribe' : 'unsubscribe';
  }
  if (keys.includes('update')) {
    if (keys.some((key) => !FIELDS.update.includes(key))) deny('token_request_denied');
    return 'update';
  }
  const kind = matches.length === 1 ? matches[0]! : deny('token_request_denied');
  if ((kind === 'subscribe' || kind === 'unsubscribe') && !allowSubscriptions) deny('not_supported_over_rest');
  if (!FIELDS[kind] || allKeys.some((key) => !metadata.has(key) && !FIELDS[kind]!.includes(key))) deny('token_request_denied');
  return kind;
}
export function validateBatch(batch: unknown, allowSubscriptions: boolean): asserts batch is BatchRequest {
  const seen = new WeakSet<object>();
  const walk = (value: unknown, nested: boolean, depth: number): void => {
    if (!isRecord(value) || !isRecord(value.queries) || !Object.prototype.hasOwnProperty.call(value, 'id') || depth > 32 || seen.has(value) || Object.keys(value).some((key) => !BATCH_FIELDS.has(key))) deny('token_request_denied');
    if (['interner_epochs','records_idmsgpack'].some((key) => Object.prototype.hasOwnProperty.call(value, key)) || value.result_encoding !== undefined && value.result_encoding !== 'name') deny('token_request_denied');
    seen.add(value);
    for (const entry of Object.values(value.queries)) {
      if (!isRecord(entry)) deny('token_request_denied');
      const kind = classify(entry, allowSubscriptions);
      if (kind === 'subscribe' || kind === 'unsubscribe') { if (nested) deny('token_request_denied'); continue; }
      if (nested && INTRO.has(kind)) deny('token_request_denied');
      if (kind === 'batch' || kind === 'for_each') walk(entry[kind], true, depth + 1);
    }
    seen.delete(value);
  };
  walk(batch, false, 0);
}
function envelope(response: TokenResponseMap, kind: string): Record<string, unknown> {
  if (response.kind !== kind || !isRecord(response.response)) throw new TokenTransportError('protocol_error', 'Malformed response envelope');
  return response.response;
}
class TokenHttpClientImpl implements TokenHttpClient {
  readonly channel = 'http' as const; readonly sid = undefined; readonly expiresAtNs = undefined;
  private closed = false;
  constructor(private readonly options: NormalizedTokenHttpOptions, private readonly transport: { request(request: import('../core/protocol/token/options.js').TokenHttpRequest, options?: import('../core/protocol/token/options.js').TokenRequestOptions): Promise<TokenResponseMap>; close(): Promise<void> }, private readonly allowSubscriptions = false) {}
  async ping(): Promise<{ kind: 'pong' }> { const result = await this.transport.request({ op: 'ping' }, this.requestOptions()); if (result.kind !== 'pong') throw new TokenTransportError('protocol_error', 'Malformed ping response'); return { kind: 'pong' }; }
  async execute(db: string, batch: object, wait?: VisibilityWait): Promise<BatchResponse> {
    validateBatch(batch, this.allowSubscriptions); let normalized: BatchRequest;
    try { normalized = structuredClone(batch) as BatchRequest; } catch { deny('token_request_denied'); }
    if (wait !== undefined) normalized.visibility_wait = safeWait(wait);
    const result = envelope(await this.transport.request({ op: 'execute', db, batch: normalized, query_version: 4 }, this.requestOptions()), 'batch');
    try { return validateTokenBatchResponse(result); } catch { throw new TokenTransportError('protocol_error', 'Malformed batch response'); }
  }
  executeWithTouch(db: string, batch: object): Promise<BatchResponse> { return this.execute(db, batch); }
  async waitForVisibility(db: string, repo: string, version: bigint, wait: VisibilityWait): Promise<VisibilityState> {
    const requested = position(version); const result = await this.transport.request({ op: 'wait_for_visibility', db, repo, version: requested, visibility_wait: safeWait(wait), query_version: 4 }, this.requestOptions());
    try { if (result.kind !== 'visibility_observed' || position(result.version) !== requested || !isRecord(result.visibility) || typeof result.visibility.state !== 'string') throw new Error(); return normalizeVisibility(result.visibility as VisibilityState); } catch { throw new TokenTransportError('protocol_error', 'Malformed visibility response'); }
  }
  async getDdlOpStatus(db: string, repo: string, table: string, opId: string): Promise<DdlOpStatus | null> {
    const result = await this.transport.request({ op: 'get_ddl_op_status', db, repo, table, op_id: opId }, this.requestOptions());
    if (result.kind !== 'ddl_op_status' || !Object.prototype.hasOwnProperty.call(result, 'status')) throw new TokenTransportError('protocol_error', 'Malformed DDL status');
    const status = result.status; if (status === null) return null; if (!isRecord(status) || typeof status.op_id !== 'string') throw new TokenTransportError('protocol_error', 'Malformed DDL status');
    return { op_id: status.op_id, kind: normalizeKind(status.kind) as DdlOpStatus['kind'], state: normalizeTokenDdlState(status.state) };
  }
  db(name: string): TokenHttpDb { return new TokenHttpDb(this, name); }
  async close(): Promise<void> { if (!this.closed) { this.closed = true; await this.transport.close(); } }
  private requestOptions(): { signal?: AbortSignal; timeoutMs: number } { if (this.closed) throw new TokenTransportError('connection_closed', 'Token client is closed'); if (this.options.signal?.aborted) throw new TokenTransportError('client_aborted', 'Token client was aborted'); return { ...(this.options.signal ? { signal: this.options.signal } : {}), timeoutMs: this.options.requestTimeoutMs }; }
}
function safeWait(wait: VisibilityWait): VisibilityWait {
  try {
    if (!isRecord(wait) || (wait.mode !== 'strict' && wait.mode !== 'bounded') ||
      (wait.mode === 'strict' && Object.keys(wait).some((key) => key !== 'mode')) ||
      (wait.mode === 'bounded' && Object.keys(wait).some((key) => key !== 'mode' && key !== 'timeout_ms'))) throw new Error();
    return wait.mode === 'strict' ? { mode: 'strict' } : { mode: 'bounded', timeout_ms: position(wait.timeout_ms) };
  } catch { throw new TokenTransportError('invalid_request', 'Invalid visibility wait'); }
}
function tagged(value: unknown, allowed: string[]): Record<string, unknown> { if (typeof value === 'string' && allowed.includes(value)) return { kind: value }; if (!isRecord(value)) throw new TokenTransportError('protocol_error', 'Malformed DDL status'); if (typeof value.kind === 'string' && allowed.includes(value.kind)) return value; const keys = Object.keys(value); if (keys.length !== 1 || !allowed.includes(keys[0] ?? '')) throw new TokenTransportError('protocol_error', 'Malformed DDL status'); const key = keys[0]!; const inner = value[key]; return { kind: key, ...(isRecord(inner) ? inner : {}) }; }
function normalizeKind(value: unknown): DdlOpStatus['kind'] { const kind = tagged(value, ['create_hash_index','create_unique_hash_index','drop_hash_index','drop_unique_hash_index','rename_hash_index','rename_unique_hash_index','drop_index2','drop_sorted_index','rename_sorted_index','rename_index2','other']); const required: Record<string, string[]> = { create_hash_index: ['index_name','table_name'], create_unique_hash_index: ['index_name','table_name'], drop_hash_index: ['index_name'], drop_unique_hash_index: ['index_name'], rename_hash_index: ['old_name','new_name'], rename_unique_hash_index: ['old_name','new_name'], drop_index2: ['index_name'], drop_sorted_index: ['index_name'], rename_sorted_index: ['old_name','new_name'], rename_index2: ['old_name','new_name'], other: ['description'] }; for (const field of required[String(kind.kind)] ?? []) if (typeof kind[field] !== 'string') throw new TokenTransportError('protocol_error', 'Malformed DDL status'); return kind as unknown as DdlOpStatus['kind']; }
export function createTokenHttpClient<T extends TokenHttpConnectOptions>(options: T, platformFactory: TokenPlatformFactory<T>): TokenHttpClient { const normalized = normalizeTokenHttpOptions(options); const platformOptions = { ...options, url: normalized.url.href }; return new TokenHttpClientImpl(normalized, new TokenHttpTransport(normalized.url, normalized.token, platformFactory(platformOptions), { requestTimeoutMs: normalized.requestTimeoutMs, ...(normalized.signal ? { signal: normalized.signal } : {}) })); }
export function createTokenOperations(options: TokenHttpConnectOptions, transport: { request(request: import('../core/protocol/token/options.js').TokenHttpRequest, options?: import('../core/protocol/token/options.js').TokenRequestOptions): Promise<TokenResponseMap>; close(): Promise<void> }, allowSubscriptions = true): TokenHttpClient { const normalized = normalizeTokenHttpOptions({ ...options, channel: 'http' }); return new TokenHttpClientImpl(normalized, transport, allowSubscriptions); }
