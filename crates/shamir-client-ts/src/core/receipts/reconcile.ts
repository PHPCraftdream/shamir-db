/**
 * Reconcile pending per-alias receipts through `waitForVisibility`.
 *
 * PLATFORM-AGNOSTIC.
 */

import { position } from '../visibility/positions.js';
import type { VisibilityState } from '../visibility/types.js';
import type { AliasReceipt, RepoObservation } from './types.js';

/** Group pending receipts by repository and issue ONE observation per
 * repository, at the highest pending position. Visible receipts are
 * skipped. Receipts with no derivable repository and pending receipts
 * without a commit position are refused rather than guessed.
 *
 * NO write is ever resent and there is no retry loop: `observe` is called
 * exactly once per distinct repository, and a transport failure
 * propagates as-is — the underlying writes are committed regardless. */
export async function reconcileReceipts(
  receipts: readonly AliasReceipt[],
  observe: (repo: string, version: bigint) => Promise<VisibilityState>,
): Promise<RepoObservation[]> {
  const targets = new Map<string, bigint>();
  for (const receipt of receipts) {
    if (receipt.visibility.state === 'visible') continue;
    if (receipt.repo === null) {
      throw new Error(`receipt for alias '${receipt.alias}' has no derivable repository; refusing to guess`);
    }
    const commit = receipt.transaction.commit_version;
    if (commit === undefined) {
      throw new Error(`pending receipt for alias '${receipt.alias}' carries no commit_version`);
    }
    const version = position(commit);
    const current = targets.get(receipt.repo);
    if (current === undefined || current < version) targets.set(receipt.repo, version);
  }
  const observations: RepoObservation[] = [];
  for (const [repo, version] of targets) {
    observations.push({ repo, version, visibility: await observe(repo, version) });
  }
  return observations;
}
