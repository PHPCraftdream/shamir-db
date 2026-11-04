/**
 * Transport-neutral endpoint/link descriptions for the session layer.
 * Types only. `ws-native` and `tcp` are Node-only endpoints — no dialer
 * exists yet (S3, out of scope).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ChannelBinding } from './binding.js';
import type { Socket } from '../../support/platform.js';

export type SessionEndpoint =
  | { kind: 'ws-browser'; url: string; origin: string; tls?: { rejectUnauthorized?: boolean } }
  | { kind: 'ws-native'; url: string; tls?: { rejectUnauthorized?: boolean } }
  | { kind: 'tcp'; host: string; port: number; tls?: { rejectUnauthorized?: boolean } }
  | { kind: 'ipc'; addr: string };

export interface SessionLink {
  socket: Socket;
  binding: ChannelBinding;
}
