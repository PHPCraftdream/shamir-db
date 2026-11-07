import { describe, expect, it, vi } from 'vitest';
import { FailureRecorder, MAX_CLIENT_EVENTS, MAX_LINE_LENGTH, serverLogTail, withFailureDiagnostics } from './failure-diagnostics.js';

describe('FailureRecorder', () => {
  it('renders timeline, notes and client events', () => {
    const recorder = new FailureRecorder();
    recorder.mark('subscribe sent');
    recorder.note('port', 4242);
    recorder.note('token label', 'revoke-1');
    recorder.event('frame sent op=put');
    recorder.event('close code=4012 reason=token_expired');
    const rendered = recorder.render();
    expect(rendered).toContain('--- timeline ---');
    expect(rendered).toMatch(/\+\d+ms subscribe sent/);
    expect(rendered).toContain('--- notes ---');
    expect(rendered).toContain('port: 4242');
    expect(rendered).toContain('token label: revoke-1');
    expect(rendered).toContain('--- client events ---');
    expect(rendered).toContain('frame sent op=put');
    expect(rendered).toContain('close code=4012 reason=token_expired');
    expect(rendered).not.toContain('older client events dropped');
  });

  it('caps the client-event ring at MAX_CLIENT_EVENTS and reports the drops', () => {
    const recorder = new FailureRecorder();
    const total = MAX_CLIENT_EVENTS + 25;
    for (let i = 0; i < total; i += 1) recorder.event(`event-${String(i).padStart(4, '0')}`);
    const rendered = recorder.render();
    expect(rendered).toContain('event-0224');
    expect(rendered).not.toContain('event-0000');
    expect(rendered).not.toContain('event-0024');
    expect(rendered).toContain('(+25 older client events dropped)');
  });

  it('truncates oversized event text at MAX_LINE_LENGTH', () => {
    const recorder = new FailureRecorder();
    recorder.event('x'.repeat(MAX_LINE_LENGTH + 100));
    const rendered = recorder.render();
    expect(rendered).toContain('(truncated)');
    expect(rendered.length).toBeLessThan(MAX_LINE_LENGTH + 200);
  });

  it('truncates oversized string notes at MAX_LINE_LENGTH', () => {
    const recorder = new FailureRecorder();
    recorder.note('frame', 'y'.repeat(MAX_LINE_LENGTH + 100));
    const rendered = recorder.render();
    expect(rendered).toContain('(truncated)');
    expect(rendered.length).toBeLessThan(MAX_LINE_LENGTH + 300);
  });

  it('renders empty sections and JSON-encodes object notes', () => {
    const recorder = new FailureRecorder();
    recorder.note('first frame', { binary: true, tokenOnly: true });
    const rendered = recorder.render();
    expect(rendered).toContain('(no marks)');
    expect(rendered).toContain('(no client events)');
    expect(rendered).toContain('first frame: {"binary":true,"tokenOnly":true}');
  });
});

describe('withFailureDiagnostics', () => {
  it('returns the body result and writes nothing on a passing body', async () => {
    const logSpy = vi.spyOn(console, 'log');
    const errorSpy = vi.spyOn(console, 'error');
    const warnSpy = vi.spyOn(console, 'warn');
    const recorder = new FailureRecorder();
    recorder.mark('inside a passing body');
    const result = await withFailureDiagnostics(recorder, () => 'server log', async () => 'ok');
    expect(result).toBe('ok');
    expect(logSpy).not.toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
    expect(warnSpy).not.toHaveBeenCalled();
    logSpy.mockRestore();
    errorSpy.mockRestore();
    warnSpy.mockRestore();
  });

  it('carries the original message, rendered sections and server log tail on failure', async () => {
    const recorder = new FailureRecorder();
    recorder.mark('subscribe sent');
    recorder.note('origin index', 1);
    recorder.event('frame received kind=event');
    const error = await withFailureDiagnostics(recorder, () => 'first log line\nlast log line', async () => {
      throw new Error('subscription event lost');
    }).then(() => undefined, (caught: unknown) => caught);
    expect(error).toBeInstanceOf(Error);
    const message = (error as Error).message;
    expect(message).toContain('subscription event lost');
    expect(message).toContain('--- diagnostics (failure-only) ---');
    expect(message).toContain('--- timeline ---');
    expect(message).toMatch(/\+\d+ms subscribe sent/);
    expect(message).toContain('--- notes ---');
    expect(message).toContain('origin index: 1');
    expect(message).toContain('--- client events ---');
    expect(message).toContain('frame received kind=event');
    expect(message).toContain('--- server log tail ---');
    expect(message).toContain('last log line');
    expect((error as { cause?: unknown }).cause).toBeInstanceOf(Error);
    expect(((error as { cause: Error }).cause).message).toBe('subscription event lost');
  });

  it('omits the server log section when no server logs are attached', async () => {
    const recorder = new FailureRecorder();
    recorder.mark('abort called');
    const error = await withFailureDiagnostics(recorder, undefined, async () => {
      throw new Error('aborted');
    }).then(() => undefined, (caught: unknown) => caught);
    const message = (error as Error).message;
    expect(message).toContain('aborted');
    expect(message).not.toContain('--- server log tail ---');
    expect(message).toContain('--- timeline ---');
  });

  it('renders thrown non-Error values', async () => {
    const recorder = new FailureRecorder();
    const error = await withFailureDiagnostics(recorder, undefined, async () => {
      throw 'plain string failure';
    }).then(() => undefined, (caught: unknown) => caught);
    expect((error as Error).message).toContain('plain string failure');
  });
});

describe('serverLogTail', () => {
  it('strips ANSI escapes and keeps the requested tail', () => {
    const logs = '\x1b[32mbound\x1b[0m\nline2\nline3';
    expect(serverLogTail(logs)).toBe('bound\nline2\nline3');
    expect(serverLogTail(logs, 2)).toBe('line2\nline3');
  });
});
