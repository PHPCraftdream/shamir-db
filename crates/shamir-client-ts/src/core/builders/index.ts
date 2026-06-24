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
export {
  currentOnly, olderThan, olderThanAge,
  FieldBuilder,
  createDb, createRepo, createTable, createIndex,
  setTableSchema, addSchemaRule, removeSchemaRule, getTableSchema,
  setBufferConfig, getBufferConfig, alterBufferConfig,
  migrationStatus,
  createFunction, dropFunction, renameFunction,
  createValidator, dropValidator, renameValidator,
  bindValidator, unbindValidator, listValidators,
  createFunctionFolder,
  setRetention, purgeHistory, changesSince,
  listDatabases, listRepos, listTables, listIndexes,
  listUsers, listRoles, listFunctions, listValidators_, listFunctionFolders,
  dropDb, dropRepo, dropTable, dropIndex,
  startMigration, commitMigration, rollbackMigration,
  ddl,
  // `field` is NOT re-exported here — it collides with select.field.
  // Use `ddl.field(...)` or import directly from './ddl.js'.
} from './ddl.js';
export * from './admin.js';
export { principalId } from '../principal-id.js';
export * from './query.js';
export * from './call.js';
export * from './batch.js';
export * from './subscribe.js';
