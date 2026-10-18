import { CURRENT_QUERY_LANG_VERSION } from '../protocol/scram.js';
import { normalizeVisibility, normalizeWait, position } from './positions.js';
import type { VisibilityState, VisibilityWait } from './types.js';

export async function waitForVisibility(
  send: (request: object) => Promise<Record<string, unknown>>,
  db: string,
  repo: string,
  version: bigint,
  wait: VisibilityWait,
): Promise<VisibilityState> {
  const requested = position(version);
  const response = await send({ op: 'wait_for_visibility', query_version: CURRENT_QUERY_LANG_VERSION,
    db, repo, version: requested, visibility_wait: normalizeWait(wait) });
  if (response.kind !== 'visibility_observed' || position(response.version) !== requested) {
    throw new Error('unexpected visibility observation');
  }
  return normalizeVisibility(response.visibility as VisibilityState);
}
