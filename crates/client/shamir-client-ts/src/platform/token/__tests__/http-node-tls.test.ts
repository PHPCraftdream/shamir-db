import type { EventEmitter } from 'node:events';
import type { RequestOptions } from 'node:http';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiToken } from '../../../entry/api-token.js';
import type { NodeTokenHttpConnectOptions } from '../../../core/types/connection.js';

const captured = vi.hoisted(() => ({ agents: [] as Array<{ kind: string; options: Record<string, unknown>; destroy: ReturnType<typeof vi.fn> }>, requests: [] as Array<{ options: RequestOptions; request: EventEmitter & { write: ReturnType<typeof vi.fn>; end: ReturnType<typeof vi.fn>; destroy: ReturnType<typeof vi.fn> } }>, globalDestroy: vi.fn() }));

vi.mock('node:http', async () => {
  const { EventEmitter } = await vi.importActual<typeof import('node:events')>('node:events');
  class FakeAgent {
    options: Record<string, unknown>;
    destroy = vi.fn();
    constructor(options: Record<string, unknown> = {}) { this.options = options; captured.agents.push(Object.assign(this, { kind: 'http' })); }
  }
  const globalAgent = { destroy: captured.globalDestroy };
  const request = (options: RequestOptions): EventEmitter & { write: ReturnType<typeof vi.fn>; end: ReturnType<typeof vi.fn>; destroy: ReturnType<typeof vi.fn> } => {
    const req = Object.assign(new EventEmitter(), { write: vi.fn(), end: vi.fn(), destroy: vi.fn() });
    captured.requests.push({ options, request: req });
    queueMicrotask(() => req.emit('error', Object.assign(new Error('peer'), { code: 'CERT_HAS_EXPIRED' })));
    return req;
  };
  return { ...await vi.importActual<typeof import('node:http')>('node:http'), Agent: FakeAgent, globalAgent, request };
});

vi.mock('node:https', async () => {
  const { EventEmitter } = await vi.importActual<typeof import('node:events')>('node:events');
  class FakeAgent {
    options: Record<string, unknown>;
    destroy = vi.fn();
    constructor(options: Record<string, unknown> = {}) { this.options = options; captured.agents.push(Object.assign(this, { kind: 'https' })); }
  }
  const request = (options: RequestOptions): EventEmitter & { write: ReturnType<typeof vi.fn>; end: ReturnType<typeof vi.fn>; destroy: ReturnType<typeof vi.fn> } => {
    const req = Object.assign(new EventEmitter(), { write: vi.fn(), end: vi.fn(), destroy: vi.fn() });
    captured.requests.push({ options, request: req });
    queueMicrotask(() => req.emit('error', Object.assign(new Error('peer'), { code: 'CERT_HAS_EXPIRED' })));
    return req;
  };
  return { ...await vi.importActual<typeof import('node:https')>('node:https'), Agent: FakeAgent, globalAgent: { destroy: captured.globalDestroy }, request };
});

import { createNodeTokenPlatform } from '../http-node.js';

const token = ApiToken.from(`shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`);
const options = (extra: Partial<NodeTokenHttpConnectOptions> = {}): NodeTokenHttpConnectOptions => ({ auth: 'token', channel: 'http', token, url: 'https://example.test', ...extra });
const init = (signal = new AbortController().signal) => ({ method: 'GET' as const, headers: {}, signal, redirect: 'error' as const, credentials: 'omit' as const });

beforeEach(() => { captured.agents.length = 0; captured.requests.length = 0; captured.globalDestroy.mockClear(); });

describe('Node TLS agent plumbing', () => {
  it('configures isolated TLS agents and sanitizes peer errors', async () => {
    const ca = Buffer.from('test CA');
    const tls = { ca };
    const platform = createNodeTokenPlatform(options({ tls }));
    ca[0] = 0;
    expect(captured.agents).toHaveLength(2);
    expect(captured.agents[0]?.options).toMatchObject({ keepAlive: true });
    expect(captured.agents[1]?.options).toMatchObject({ keepAlive: true, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'], rejectUnauthorized: true });
    expect(captured.agents[1]?.options.ca).toEqual(Buffer.from('test CA'));
    const result = platform.requestHttp(new URL('https://example.test/shamir/v1/ping'), token, init());
    expect(captured.requests[0]?.options.agent).toBe(captured.agents[1]);
    await expect(result).rejects.toMatchObject({ code: 'tls_configuration_error', phase: 'request' });
    await platform.close();
    expect(captured.agents.map(agent => agent.destroy)).toEqual([expect.any(Function), expect.any(Function)]);
    expect(captured.agents.every(agent => agent.destroy.mock.calls.length === 1)).toBe(true);
    expect(captured.globalDestroy).not.toHaveBeenCalled();
  });

  it('only bypasses verification when explicitly enabled and closes pending work cleanly', async () => {
    const platform = createNodeTokenPlatform(options({ tls: { allowInsecureTlsForDevelopment: true } }));
    expect(captured.agents[1]?.options).toMatchObject({ rejectUnauthorized: false, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'] });
    const pending = platform.requestHttp(new URL('https://example.test/shamir/v1/ping'), token, init());
    expect(captured.requests[0]?.options.agent).toBe(captured.agents[1]);
    await expect(pending).rejects.toMatchObject({ code: 'tls_configuration_error' });
    await platform.close();
    expect(captured.agents.every(agent => agent.destroy.mock.calls.length === 1)).toBe(true);
    expect(captured.globalDestroy).not.toHaveBeenCalled();
    await expect(platform.requestHttp(new URL('https://example.test/shamir/v1/ping'), token, init())).rejects.toMatchObject({ code: 'connection_closed' });
  });
});
