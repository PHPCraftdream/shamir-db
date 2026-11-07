const SUBSCRIPTION_LOG_MARKERS = ['sub_bridge', 'subscription', 'journal', 'changefeed', 'activation', 'bridge', 'push'];

/** Server log lines about the subscription pipeline (ANSI stripped), most recent `limit` lines. */
export function subscriptionLogLines(logs: string, limit = 300): string {
  const lines = logs
    .replace(/\x1b\[[0-9;]*m/g, '')
    .split('\n')
    .filter((line) => SUBSCRIPTION_LOG_MARKERS.some((marker) => line.includes(marker)));
  return lines.slice(-limit).join('\n');
}

/** Wall-clock timeline of a test step, rendered into the failure message. */
export class Timeline {
  private readonly started = Date.now();
  private readonly marks: string[] = [];

  mark(label: string): void {
    this.marks.push(`+${Date.now() - this.started}ms ${label}`);
  }

  render(): string {
    return this.marks.join('\n');
  }
}

export type LiveOutcome<T> =
  | { kind: 'event'; value: T }
  | { kind: 'late'; value: T; afterMs: number }
  | { kind: 'lost'; waitedMs: number };

/**
 * Waits for `pending` up to `deadlineMs`; if it misses the deadline keeps waiting for `graceMs` more
 * so a LATE event (slow delivery) is told apart from a LOST one (never delivered).
 */
export async function awaitLiveEvent<T>(pending: Promise<T>, deadlineMs: number, graceMs: number): Promise<LiveOutcome<T>> {
  const started = Date.now();
  const timeoutOutcome = Symbol('timeout');
  let timer: ReturnType<typeof setTimeout> | undefined;
  const race = async (ms: number): Promise<T | typeof timeoutOutcome> => {
    try {
      return await Promise.race([pending, new Promise<typeof timeoutOutcome>((resolve) => { timer = setTimeout(() => resolve(timeoutOutcome), ms); })]);
    } finally { if (timer !== undefined) clearTimeout(timer); }
  };
  const first = await race(deadlineMs);
  if (first !== timeoutOutcome) return { kind: 'event', value: first as T };
  const second = await race(graceMs);
  if (second !== timeoutOutcome) return { kind: 'late', value: second as T, afterMs: Date.now() - started };
  return { kind: 'lost', waitedMs: Date.now() - started };
}
