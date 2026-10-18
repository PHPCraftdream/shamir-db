/**
 * DDL builders barrel — re-exports every name the former monolithic
 * `builders/ddl.ts` exported (same names, now split across ddl-index /
 * ddl-schema / ddl-func / ddl-misc / field-builder), the repo-admin
 * passthrough, and the `ddl` aggregate namespace composed exactly as
 * before.
 */

// Imports for the `ddl` aggregate namespace below (same members, same order
// as the original `builders/ddl.ts`).
import {
  createDb,
  createTable,
  dropDb,
  dropRepo,
  dropTable,
  renameTable,
  renameRepo,
  renameDb,
  setTableSchema,
  addSchemaRule,
  removeSchemaRule,
  getTableSchema,
  describeTable,
  startMigration,
  commitMigration,
  rollbackMigration,
} from './ddl-schema.js';
import {
  currentOnly,
  olderThan,
  olderThanAge,
  setBufferConfig,
  getBufferConfig,
  alterBufferConfig,
  migrationStatus,
  setRetention,
  purgeHistory,
  changesSince,
  internerDump,
  internerTouch,
  listDatabases,
  listRepos,
  listTables,
  listIndexes,
  listUsers,
} from './ddl-misc.js';
import {
  createFunction,
  dropFunction,
  renameFunction,
  createValidator,
  dropValidator,
  renameValidator,
  bindValidator,
  unbindValidator,
  listValidators,
  createFunctionFolder,
  renameFunctionFolder,
  listFunctions,
  listValidators_,
  listFunctionFolders,
} from './ddl-func.js';
import { createIndex, dropIndex, renameIndex } from './ddl-index.js';
import { FieldBuilder, field } from './field-builder.js';
import { createRepo, alterRepo, describeRepo } from '../admin/repo-admin/index.js';

export type {
  Tokenizer,
  Metric,
  Quantization,
} from './ddl-index.js';

export {
  createIndex,
  hashIndex,
  uniqueIndex,
  sortedIndex,
  sortedWithIncludeIndex,
  ftsIndex,
  functionalIndex,
  vectorIndex,
  dropIndex,
  renameIndex,
} from './ddl-index.js';

export {
  createDb,
  createTable,
  dropDb,
  dropRepo,
  dropTable,
  renameTable,
  renameRepo,
  renameDb,
  setTableSchema,
  addSchemaRule,
  removeSchemaRule,
  getTableSchema,
  describeTable,
  startMigration,
  commitMigration,
  rollbackMigration,
} from './ddl-schema.js';

export {
  createFunction,
  dropFunction,
  renameFunction,
  createValidator,
  dropValidator,
  renameValidator,
  bindValidator,
  unbindValidator,
  listValidators,
  createFunctionFolder,
  renameFunctionFolder,
  listFunctions,
  listValidators_,
  listFunctionFolders,
} from './ddl-func.js';

export {
  currentOnly,
  olderThan,
  olderThanAge,
  setBufferConfig,
  getBufferConfig,
  alterBufferConfig,
  migrationStatus,
  setRetention,
  purgeHistory,
  changesSince,
  internerDump,
  internerTouch,
  listDatabases,
  listRepos,
  listTables,
  listIndexes,
  listUsers,
} from './ddl-misc.js';

export { FieldBuilder, field } from './field-builder.js';

export { createRepo, alterRepo, describeRepo } from '../admin/repo-admin/index.js';

/** Aggregate namespace — every DDL constructor in one object. */
export const ddl = {
  currentOnly,
  olderThan,
  olderThanAge,
  field,
  FieldBuilder,
  createDb,
  createRepo,
  alterRepo,
  describeRepo,
  createTable,
  createIndex,
  setTableSchema,
  addSchemaRule,
  removeSchemaRule,
  getTableSchema,
  describeTable,
  setBufferConfig,
  getBufferConfig,
  alterBufferConfig,
  migrationStatus,
  createFunction,
  dropFunction,
  renameFunction,
  createValidator,
  dropValidator,
  renameValidator,
  bindValidator,
  unbindValidator,
  listValidators,
  createFunctionFolder,
  renameFunctionFolder,
  setRetention,
  purgeHistory,
  changesSince,
  internerDump,
  internerTouch,
  listDatabases,
  listRepos,
  listTables,
  listIndexes,
  listUsers,
  listFunctions,
  listValidators_,
  listFunctionFolders,
  dropDb,
  dropRepo,
  dropTable,
  dropIndex,
  renameTable,
  renameRepo,
  renameDb,
  renameIndex,
  startMigration,
  commitMigration,
  rollbackMigration,
};
