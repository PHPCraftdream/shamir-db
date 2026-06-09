/**
 * Builders barrel — FLAT named re-exports of every constructor / fluent
 * builder. Consumers import names directly, with no namespace objects and no
 * `import * as`:
 *
 *   import { eq, gt, insert, createTable, chmod, Query, Batch } from '@shamir/client';
 *
 * Every exported name is unique across the filter / select / write / ddl /
 * admin / query / batch modules, so the flat surface has no collisions.
 *
 * PLATFORM-AGNOSTIC.
 */

export * from './filter.js';
export * from './select.js';
export * from './write.js';
export * from './ddl.js';
export * from './admin.js';
export * from './query.js';
export * from './call.js';
export * from './batch.js';
