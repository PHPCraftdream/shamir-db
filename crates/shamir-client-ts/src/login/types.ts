export interface LoginScope {
  label?: string;
  mode?: 'read_only' | 'read_write';
  channels?: 'rest' | 'ws' | 'both';
  databases?: readonly string[] | null;
}

export interface LoginOptions {
  username: string;
  password: string;
  url: string;
  label?: string;
  mode?: 'read_only' | 'read_write';
  channels?: 'rest' | 'ws' | 'both';
  databases?: readonly string[] | null;
  scope?: LoginScope;
  externalTls?: boolean;
  listenerId?: Uint8Array;
  expectedKdf?: { memoryKb: number; time: number; parallelism: number };
  signal?: AbortSignal;
  timeoutMs?: number;
  connectTimeoutMs?: number;
  requestTimeoutMs?: number;
  ttl?: never;
}

export interface LoginMetadata {
  id: Uint8Array;
  label: string;
  owner: string;
  mode: 'read_only' | 'read_write';
  channels: 'rest' | 'ws' | 'both';
  databases: readonly string[] | null;
  createdAtNs: bigint;
  expiresAtNs: bigint;
}

export interface LoginResult {
  token: import('../entry/api-token.js').ApiToken;
  metadata: LoginMetadata;
  connect(options: {
    channel: 'http';
    signal?: AbortSignal;
    requestTimeoutMs?: number;
  }): Promise<import('../entry/token-client.js').TokenHttpClient>;
  connect(options: {
    channel: 'ws';
    signal?: AbortSignal;
    requestTimeoutMs?: number;
  }): Promise<import('../entry/ws/token-client.js').TokenWsClient>;
  connect(options: {
    channel: 'http' | 'ws';
    signal?: AbortSignal;
    requestTimeoutMs?: number;
  }): Promise<import('../entry/ws/token-client.js').PublicTokenClient>;
}
