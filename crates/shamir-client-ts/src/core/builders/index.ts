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

export * from './read/filter.js';
export {
  all, field, countAll,
  aggregate, count, sum, avg, min, max,
  aggregateFn, func,
  selectExpr,
  select,
  // `expr` is NOT re-exported here — it collides with filter.ts's `expr`
  // (the `$expr` FilterValue builder). Use `select.expr(...)` or import
  // directly from './read/select.js'.
} from './read/select.js';
export * from './write/write.js';
export * from './admin/repo-admin/index.js';
export {
  currentOnly, olderThan, olderThanAge,
  FieldBuilder,
  createDb, createTable, createIndex,
  setTableSchema, addSchemaRule, removeSchemaRule, getTableSchema,
  setBufferConfig, getBufferConfig, alterBufferConfig,
  migrationStatus,
  createFunction, dropFunction, renameFunction,
  createValidator, dropValidator, renameValidator,
  bindValidator, unbindValidator, listValidators,
  createFunctionFolder,
  renameFunctionFolder,
  setRetention, purgeHistory, changesSince,
  listDatabases, listRepos, listTables, listIndexes,
  listUsers, listFunctions, listValidators_, listFunctionFolders,
  dropDb, dropRepo, dropTable, dropIndex,
  startMigration, commitMigration, rollbackMigration,
  renameRepo, renameDb,
  ddl,
  // `field` is NOT re-exported here — it collides with select.field.
  // Use `ddl.field(...)` or import directly from './ddl/index.js'.
} from './ddl/index.js';
export * from './admin/index.js';
export * from './stream/replication.js';
export * from './read/query.js';
export * from './write/call.js';
export * from './write/batch.js';
export * from './handle.js';
export * from './stream/subscribe.js';
export * from './read/cursor.js';
