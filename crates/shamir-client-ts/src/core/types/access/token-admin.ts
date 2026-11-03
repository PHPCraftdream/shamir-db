import type { ApiToken } from '../../../entry/api-token.js';

export type ApiTokenMode = 'read_only' | 'read_write';
export type ApiTokenChannels = 'rest' | 'ws' | 'both';

export interface CreateApiTokenSpec {
  label: string;
  ttl_secs?: number | bigint;
  mode?: ApiTokenMode;
  channels?: ApiTokenChannels;
  databases?: string[];
  for_user?: string;
}

export interface CreateApiTokenOp extends CreateApiTokenSpec {
  op: 'create_api_token';
  hmac: string;
}

export interface ListApiTokensOp {
  op: 'list_api_tokens';
  for_user?: string;
  hmac: string;
}

export interface RevokeApiTokenOp {
  op: 'revoke_api_token';
  id: string;
  hmac: string;
}

export interface RevokeUserTokensOp {
  op: 'revoke_user_tokens';
  user?: string;
  hmac: string;
}

export interface CreatedApiToken {
  id: string;
  token: ApiToken;
  label: string;
  owner: string;
  mode: ApiTokenMode;
  channels: ApiTokenChannels;
  databases: string[] | null;
  created_at_ns: bigint;
  expires_at_ns: bigint;
}

export interface ApiTokenInfo {
  id: string;
  label: string;
  owner: string;
  mode: ApiTokenMode;
  channels: ApiTokenChannels;
  databases: string[] | null;
  created_at_ns: bigint;
  expires_at_ns: bigint;
  revoked_at_ns: bigint | null;
}

export interface ApiTokenAdmin {
  /** Creates a token. Browser-bound SCRAM sessions are refused by the server; no channels default is applied here. */
  create(spec: CreateApiTokenSpec): Promise<CreatedApiToken>;
  list(opts?: { for_user?: string }): Promise<ApiTokenInfo[]>;
  revoke(id: string): Promise<void>;
  revokeUser(opts?: { user?: string }): Promise<bigint>;
}
