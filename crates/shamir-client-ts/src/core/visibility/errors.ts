import { ShamirDbError } from '../errors.js';
import { position } from './positions.js';

const codes = new Set(['as_of_visibility_timeout', 'as_of_future_version',
  'as_of_reopen_required', 'visibility_future_version', 'visibility_wait_invalid',
  'write_committed_visibility_pending']);

/** Only AsOf timeout currently has structured positions on the wire. */
export class VisibilityError extends ShamirDbError {
  readonly requested?: bigint;
  readonly visible_through?: bigint;
  readonly blocking_version?: bigint;

  constructor(response: Record<string, unknown>) {
    super(String(response.code), String(response.message ?? ''));
    this.name = 'VisibilityError';
    Object.setPrototypeOf(this, VisibilityError.prototype);
    if (response.kind === 'as_of_visibility_timeout') {
      this.requested = position(response.requested);
      this.visible_through = position(response.visible_through);
      this.blocking_version = position(response.blocking_version);
    }
  }
}

export function responseError(response: Record<string, unknown>): Error {
  try {
    return codes.has(String(response.code)) ? new VisibilityError(response)
      : new ShamirDbError(String(response.code ?? 'unknown'), String(response.message ?? ''));
  } catch (error) {
    return error instanceof Error ? error : new Error(String(error));
  }
}
