/**
 * SubscriptionRouter — demultiplexes push frames by sub_id.
 *
 * Incoming push envelopes are routed to registered handlers. Frames that
 * arrive before a handler is registered are buffered and flushed once the
 * handler appears.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { PushEnvelope, PushKind } from './types/subscribe.js';

/** Decoded subscription event delivered to handlers. */
export interface SubscriptionEvent {
  kind: PushKind;
  seq: number;
  data?: Uint8Array;
  gap_at?: number;
}

export class SubscriptionRouter {
  /** Maps sub_id to registered handler. */
  private handlers = new Map<number, (ev: SubscriptionEvent) => void>();

  /** Buffers frames that arrived before the handler was registered. */
  private earlyBuffer = new Map<number, SubscriptionEvent[]>();

  /** Register a handler for a subscription. Flushes any buffered early pushes. */
  register(subId: number, handler: (ev: SubscriptionEvent) => void): void {
    this.handlers.set(subId, handler);
    const buffered = this.earlyBuffer.get(subId);
    if (buffered) {
      this.earlyBuffer.delete(subId);
      for (const ev of buffered) handler(ev);
    }
  }

  /** Unregister a subscription handler. */
  unregister(subId: number): void {
    this.handlers.delete(subId);
    this.earlyBuffer.delete(subId);
  }

  /** Route an incoming push frame. Returns true if handled/buffered. */
  route(envelope: PushEnvelope): boolean {
    const ev: SubscriptionEvent = {
      kind: envelope.push,
      seq: envelope.seq,
      data: envelope.data,
      gap_at: envelope.gap_at,
    };
    const handler = this.handlers.get(envelope.sub);
    if (handler) {
      handler(ev);
      return true;
    }
    // Buffer early pushes (server may send events before client registers handler).
    let buf = this.earlyBuffer.get(envelope.sub);
    if (!buf) {
      buf = [];
      this.earlyBuffer.set(envelope.sub, buf);
    }
    buf.push(ev);
    return true;
  }

  /** Clear all state. */
  clear(): void {
    this.handlers.clear();
    this.earlyBuffer.clear();
  }
}
