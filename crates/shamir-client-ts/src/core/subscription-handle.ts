/**
 * SubscriptionHandle — async-iterable stream for a single subscription.
 *
 * Created by the batch execution layer when a subscribe op returns a sub_id.
 * Wraps a registration in the SubscriptionRouter and exposes events as an
 * AsyncIterableIterator.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { SubscriptionRouter, SubscriptionEvent } from './subscription-router.js';
import type { ShamirClient } from './client.js';
import { Batch } from './builders/batch.js';
import { unsubscribeOp } from './builders/subscribe.js';

/** Queued resolve callback for the next() promise. */
interface Waiter {
  resolve: (value: IteratorResult<SubscriptionEvent>) => void;
}

/**
 * A live subscription handle. Implements AsyncIterableIterator so it can
 * be used with `for await...of`. Calling `.unsubscribe()` sends an
 * unsubscribe op and closes the stream.
 */
export class SubscriptionHandle implements AsyncIterableIterator<SubscriptionEvent> {
  private readonly queue: SubscriptionEvent[] = [];
  private readonly waiters: Waiter[] = [];
  private done = false;

  constructor(
    readonly subId: number,
    private readonly router: SubscriptionRouter,
    private readonly client?: ShamirClient,
    private readonly db?: string,
  ) {
    this.router.register(subId, (ev) => this.push(ev));
  }

  private push(ev: SubscriptionEvent): void {
    if (this.done) return;
    if (ev.kind === 'closed') {
      this.close();
      return;
    }
    const waiter = this.waiters.shift();
    if (waiter) {
      waiter.resolve({ value: ev, done: false });
    } else {
      this.queue.push(ev);
    }
  }

  private close(): void {
    this.done = true;
    this.router.unregister(this.subId);
    for (const w of this.waiters) {
      w.resolve({ value: undefined as unknown as SubscriptionEvent, done: true });
    }
    this.waiters.length = 0;
  }

  next(): Promise<IteratorResult<SubscriptionEvent>> {
    const queued = this.queue.shift();
    if (queued) {
      return Promise.resolve({ value: queued, done: false });
    }
    if (this.done) {
      return Promise.resolve({ value: undefined as unknown as SubscriptionEvent, done: true });
    }
    return new Promise((resolve) => {
      this.waiters.push({ resolve });
    });
  }

  /**
   * Send an unsubscribe op to the server and close the stream.
   * Requires the handle to have been created with client+db context.
   */
  async unsubscribe(): Promise<void> {
    if (this.done) return;
    if (this.client && this.db) {
      await Batch.create()
        .add('_unsub', unsubscribeOp(this.subId))
        .execute(this.client, this.db);
    }
    this.close();
  }

  /** Callback-style listener as an alternative to async iteration. */
  on(handler: (ev: SubscriptionEvent) => void): () => void {
    const loop = async () => {
      for await (const ev of this) {
        handler(ev);
      }
    };
    void loop();
    return () => this.close();
  }

  return(): Promise<IteratorResult<SubscriptionEvent>> {
    this.close();
    return Promise.resolve({ value: undefined as unknown as SubscriptionEvent, done: true });
  }

  throw(): Promise<IteratorResult<SubscriptionEvent>> {
    this.close();
    return Promise.resolve({ value: undefined as unknown as SubscriptionEvent, done: true });
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<SubscriptionEvent> {
    return this;
  }
}
