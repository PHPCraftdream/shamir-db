import type { CreateRepoOp, AlterRepoOp, DescribeRepoOp, DurabilityOverride } from '../../types/ddl.js';
import { durabilityOverride } from '../durability.js';
export { validateWalRetentionBudget } from '../durability.js';

/** CREATE REPO preserves the established engine/path/tables/if_not_exists options. */
export function createRepo(
  name: string,
  opts?: { engine?: string; path?: string; tables?: string[]; if_not_exists?: boolean; durability?: DurabilityOverride },
): CreateRepoOp {
  const op: CreateRepoOp = { create_repo: name };
  if (opts?.engine !== undefined) op.engine = opts.engine;
  if (opts?.path !== undefined) op.path = opts.path;
  if (opts?.tables !== undefined && opts.tables.length > 0) op.tables = opts.tables;
  if (opts?.if_not_exists) op.if_not_exists = true;
  if (opts?.durability !== undefined) op.durability = durabilityOverride(opts.durability);
  return op;
}

export function alterRepo(name: string, durability: DurabilityOverride): AlterRepoOp {
  return { alter_repo: name, durability: durabilityOverride(durability) };
}

export function describeRepo(name: string): DescribeRepoOp {
  return { describe_repo: name };
}

export const repoAdmin = { createRepo, alterRepo, describeRepo };
