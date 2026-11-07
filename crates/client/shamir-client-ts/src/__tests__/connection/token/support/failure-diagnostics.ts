/**
 * Failure-only diagnostics for flaky e2e tests (see
 * docs/dev-artifacts/design/flake-failure-logs.md): the recorder collects
 * evidence while the test body runs, and `withFailureDiagnostics` renders it
 * into the thrown error's message ONLY when the body fails — a passing body
 * prints and renders nothing.
 */

/** Upper bound of the client-event ring; the oldest event is dropped first. */
export const MAX_CLIENT_EVENTS = 200;
/** Per-line cap for notes and client events so one huge frame dump cannot flood the report. */
export const MAX_LINE_LENGTH = 2000;
/** Server log tail rendered into the failure message. */
export const SERVER_LOG_TAIL_LINES = 200;

/**
 * Per-test recorder: a wall-clock timeline of the test's steps (`mark`),
 * state facts that explain the failure (`note`), and a bounded ring of
 * client-side events such as frames sent/received, close codes and errors
 * (`event`). `render()` is embedded into the failure message only.
 */
export class FailureRecorder {
  private readonly started = Date.now();
  private readonly marks: string[] = [];
  private readonly notes: string[] = [];
  private readonly events: string[] = [];
  private droppedEvents = 0;

  /** Appends a wall-clock timeline entry for a test step. */
  mark(label: string): void {
    this.marks.push(`+${Date.now() - this.started}ms ${label}`);
  }

  /** Records a state fact (ports, ids, counters, the last frame seen). */
  note(key: string, value: unknown): void {
    this.notes.push(`${key}: ${formatValue(value)}`);
  }

  /** Appends a bounded client-side event; the oldest event is dropped once the ring is full. */
  event(description: string): void {
    if (this.events.length >= MAX_CLIENT_EVENTS) {
      this.events.shift();
      this.droppedEvents += 1;
    }
    this.events.push(`${Date.now() - this.started}ms ${truncateLine(description)}`);
  }

  /** Renders the collected sections; embedded into the failure message only. */
  render(): string {
    const sections = [
      `--- timeline ---\n${this.marks.length > 0 ? this.marks.join('\n') : '(no marks)'}`,
      `--- notes ---\n${this.notes.length > 0 ? this.notes.join('\n') : '(no notes)'}`,
    ];
    const dropped = this.droppedEvents > 0 ? `\n(+${this.droppedEvents} older client events dropped)` : '';
    sections.push(`--- client events ---\n${this.events.length > 0 ? this.events.join('\n') : '(no client events)'}${dropped}`);
    return sections.join('\n');
  }
}

/** Last `limit` server log lines with ANSI escapes stripped. */
export function serverLogTail(logs: string, limit = SERVER_LOG_TAIL_LINES): string {
  return logs.replace(/\x1b\[[0-9;]*m/g, '').split('\n').slice(-limit).join('\n');
}

/**
 * Runs `body`; on a throw rethrows an `Error` whose message carries the
 * original message plus the rendered recorder sections and the server log
 * tail (`serverLogs` is evaluated only on failure; pass `undefined` when the
 * test has no server). The original error stays available as `cause`.
 */
export async function withFailureDiagnostics<T>(
  recorder: FailureRecorder,
  serverLogs: (() => string) | undefined,
  body: () => Promise<T>,
): Promise<T> {
  try {
    return await body();
  } catch (error) {
    const sections = [error instanceof Error ? error.message : String(error), '--- diagnostics (failure-only) ---', recorder.render()];
    if (serverLogs !== undefined) sections.push(`--- server log tail ---\n${serverLogTail(serverLogs())}`);
    throw new Error(sections.join('\n'), { cause: error });
  }
}

function formatValue(value: unknown): string {
  if (typeof value === 'string') return truncateLine(value);
  try {
    return truncateLine(JSON.stringify(value) ?? String(value));
  } catch {
    return truncateLine(String(value));
  }
}

function truncateLine(text: string): string {
  return text.length > MAX_LINE_LENGTH ? `${text.slice(0, MAX_LINE_LENGTH)}…(truncated)` : text;
}
